//! This example demonstrates how to make a QUIC connection that ignores the server certificate.
//!
//! Checkout the `README.md` for guidance.

use std::{
    collections::HashMap,
    error::Error,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};

use crate::{
    ClientMessage, DELIMITER, MAX_PACKET_SIZE, MessageSize, PlayerId, PlayerPosition, ServerMessage,
};
use bytes::Bytes;
use hecs::World;
use quinn::{
    ClientConfig, Endpoint, SendStream, ServerConfig, VarInt,
    rustls::{self, client, pki_types::PrivatePkcs8KeyDer},
};
use quinn_proto::crypto::rustls::QuicClientConfig;
use rkyv::{Archived, de, rancor, ser};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio::{sync::Mutex, task::JoinSet};

pub fn make_server_endpoint(
    bind_addr: SocketAddr,
) -> Result<(Endpoint, CertificateDer<'static>), Box<dyn Error + Send + Sync + 'static>> {
    let (server_config, server_cert) = configure_server()?;
    let endpoint = Endpoint::server(server_config, bind_addr)?;
    Ok((endpoint, server_cert))
}

/// Returns default server configuration along with its certificate.
fn configure_server()
-> Result<(ServerConfig, CertificateDer<'static>), Box<dyn Error + Send + Sync + 'static>> {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let cert_der = CertificateDer::from(cert.cert);
    let priv_key = PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der());

    let mut server_config =
        ServerConfig::with_single_cert(vec![cert_der.clone()], priv_key.into())?;
    let transport_config = Arc::get_mut(&mut server_config.transport).unwrap();
    transport_config.max_concurrent_uni_streams(0_u8.into());

    Ok((server_config, cert_der))
}

async fn run_server() -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    //console_subscriber::init();
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
    let channel_map = Arc::new(Mutex::new(HashMap::<PlayerId, MessageChannels>::new()));
    tokio::spawn(run_quinn_server(addr, channel_map.clone()));

    let mut world = World::new();

    loop {
        //  Process incoming messages from clients
        let mut map = channel_map.lock().await;

        // in case we have to remove players, we need to collect them first
        let mut players_to_remove = Vec::<PlayerId>::new();

        for (player_id, message_channels) in map.iter() {
            let client_receiver = &message_channels.receiver;
            while let Ok(message) = client_receiver.try_recv() {
                /*
                println!(
                    "[server] received message from player {}: {:?}",
                    player_id, message
                );
                */
                // Process the message
                match message {
                    ClientMessage::PlayerPosition(player_position) => {
                        // Update player position in the world
                        let query = world.query_mut::<(&PlayerId, &mut PlayerPosition)>();
                        for (_, (id, position)) in query {
                            if *id == *player_id {
                                *position = player_position;
                                /*
                                println!(
                                    "Updated position for player {}: ({}, {})",
                                    player_id, position.x, position.y
                                );
                                */
                            }
                        }
                    }
                    ClientMessage::PlayerJoined { player_id } => {
                        // Handle player joining logic
                        println!("Player {} has joined", player_id);
                        world.spawn((player_id, PlayerPosition { x: 0.0, y: 0.0 }));
                        let query = world.query_mut::<&PlayerId>();
                        let remote_player_ids: Vec<PlayerId> =
                            query.into_iter().map(|(_, id)| *id).collect();
                        let new_player_message = ServerMessage::PlayerJoined { remote_player_ids };
                        for (_, message_channels) in map.iter() {
                            let server_sender = &message_channels.sender;
                            // Send a message to all players about the new player
                            if let Err(e) = server_sender.send(new_player_message.clone()).await {
                                println!(
                                    "Failed to send player joined message to player {}: {}",
                                    player_id, e
                                );
                            }
                        }
                    }
                    ClientMessage::Quit { player_id } => {
                        // Handle player quit logic
                        println!("Player {} has quit", player_id);
                        players_to_remove.push(player_id);
                        // Remove player from the world
                        let query = world.query_mut::<&PlayerId>();
                        let mut entities_to_despawn = Vec::new();
                        for (entity, id) in query {
                            if *id == player_id {
                                entities_to_despawn.push(entity);
                            }
                        }
                        for entity in entities_to_despawn {
                            world.despawn(entity).unwrap();
                            // Send a message to all players about the player quitting
                            let player_left_message = ServerMessage::PlayerLeft { player_id };
                            for (_, message_channels) in map.iter() {
                                let server_sender = &message_channels.sender;
                                if let Err(e) =
                                    server_sender.send(player_left_message.clone()).await
                                {
                                    println!(
                                        "Failed to send player left message to player {}: {}",
                                        player_id, e
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        for player_id in &players_to_remove {
            // Remove the player from the channel map
            map.remove(player_id);
            println!("Removed player {} from channel map", player_id);
        }

        // Clear the list of players to remove for the next iteration
        players_to_remove.clear();

        // Send messages to clients
        let game_data = world
            .query::<(&PlayerId, &PlayerPosition)>()
            .iter()
            .map(|(entity, (id, position))| ServerMessage::PlayerPosition(*id, *position))
            .collect::<Vec<ServerMessage>>();

        for (player_id, message_channels) in map.iter() {
            // Get player position in the world
            let server_sender = &message_channels.sender;
            for message in &game_data {
                // Send player position to the client
                if let Err(e) = server_sender.send(message.clone()).await {
                    println!("Failed to send message to player {}: {}", player_id, e);
                }
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(15)).await;
    }

    Ok(())
}

#[derive(Debug)]
pub struct MessageChannels {
    receiver: async_channel::Receiver<ClientMessage>,
    sender: async_channel::Sender<ServerMessage>,
}

/// Runs a QUIC server bound to given address.
async fn run_quinn_server(
    addr: SocketAddr,
    channel_map: Arc<Mutex<HashMap<PlayerId, MessageChannels>>>,
) {
    let (endpoint, _server_cert) = make_server_endpoint(addr).unwrap();

    while let Some(incoming_conn) = endpoint.accept().await {
        // accept a single connection
        let conn = incoming_conn.await.unwrap();
        println!(
            "[server] connection accepted: addr={}",
            conn.remote_address()
        );
        let (mut send_stream, mut recv_stream) = conn.open_bi().await.unwrap();
        println!("[server] opened bidirectional stream");
        // Create a new player ID for this connection
        let player_id = conn.stable_id() as PlayerId;
        // channel to send from server to client
        let (server_sender, server_receiver) = async_channel::unbounded::<ServerMessage>();
        // channel to send from client to server
        let (client_sender, client_receiver) = async_channel::unbounded::<ClientMessage>();
        // Store the channels in the map
        {
            let mut map = channel_map.lock().await;
            map.insert(
                player_id,
                MessageChannels {
                    receiver: client_receiver,
                    sender: server_sender,
                },
            );
        }

        // say hello to the client for the client to accept the connection
        let hello_message = ServerMessage::Hello { player_id };
        let serialized_message = rkyv::to_bytes::<rancor::Error>(&hello_message).unwrap();
        send_stream
            .write_all(&serialized_message)
            .await
            .expect("Failed to write to send stream");

        // send message to sync code that we have new player
        client_sender
            .send(ClientMessage::PlayerJoined { player_id })
            .await
            .unwrap();

        let client_quit_sender = client_sender.clone();
        tokio::spawn(async move {
            'sending_loop: loop {
                //println!("[server] waiting for messages to send");
                // get message from sync server code
                while let Ok(message) = server_receiver.recv().await {
                    // serialize the message
                    let serialized_message = rkyv::to_bytes::<rancor::Error>(&message).unwrap();
                    // create the header with delimiter and size
                    let size: MessageSize = (serialized_message.len() as u32).to_be_bytes();
                    // attach the start delimiter to the header (this lets the client know that a new message is coming)
                    let header = [&DELIMITER[..], &size[..]].concat();
                    // prepend the header to the serialized message
                    let serialized_message = [&header, serialized_message.as_slice()].concat();
                    // then send the serialized message
                    if let Err(e) = send_stream.write_all(&serialized_message).await {
                        println!("[server] failed to send message: {:?}, {e}", message);

                        // special edge case for quitting
                        if let Ok(()) = client_quit_sender
                            .send(ClientMessage::Quit { player_id })
                            .await
                        {
                            println!("[server] sent quit message to client");
                        }
                        //break 'sending_loop;
                    }
                }
            }
        });

        tokio::spawn(async move {
            println!(
                "[server] (loop) waiting for messages from client, stream id: {}",
                recv_stream.id()
            );
            'receive_loop: loop {
                // get message from client

                // first read the delimiter
                let mut delimiter_buf = [0; 1];
                if let Err(e) = recv_stream.read_exact(&mut delimiter_buf).await {
                    println!("[server] failed to read delimiter: {e}");
                    // If we fail to read the delimiter, it means the client has disconnected
                    break 'receive_loop;
                }

                if delimiter_buf != DELIMITER {
                    println!("[server] received invalid delimiter: {:?}", delimiter_buf);
                    continue 'receive_loop;
                }

                // Read the size of the message
                let mut size_buf: MessageSize = [0u8; 4];
                if let Err(e) = recv_stream.read_exact(&mut size_buf).await {
                    println!("[client] failed to read size: {e}");
                    continue;
                }
                // Convert the size from bytes to u32
                let size = u32::from_be_bytes(size_buf);
                // then read the actual message (only read as much as we need)
                let mut buf = vec![0u8; size as usize];
                if let Ok(()) = recv_stream.read_exact(&mut buf).await {
                    //println!("[server] buffer size {}", size);
                    // Deserialize the message
                    let message = rkyv::from_bytes::<ClientMessage, rancor::Error>(&buf).unwrap();
                    //println!("[server] received message: {:?}", message);
                    client_sender.send(message).await.unwrap();
                }
                //println!("[server] buffer {:?}", buf);
            }
        });
    }
}
