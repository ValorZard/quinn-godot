//! This example demonstrates how to make a QUIC connection that ignores the server certificate.
//!
//! Checkout the `README.md` for guidance.

use std::{
    collections::HashMap,
    error::Error,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
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

pub type ChannelMap = Arc<Mutex<HashMap<PlayerId, MessageChannels>>>;

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

pub async fn run_server() -> Result<ChannelMap, Box<dyn Error + Send + Sync + 'static>> {
    //console_subscriber::init();
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
    let channel_map = Arc::new(Mutex::new(HashMap::<PlayerId, MessageChannels>::new()));
    tokio::spawn(run_quinn_server(addr, channel_map.clone()));

    Ok(channel_map)
}

#[derive(Debug)]
pub struct MessageChannels {
    pub receiver: async_channel::Receiver<ClientMessage>,
    pub sender: async_channel::Sender<ServerMessage>,
}

/// Runs a QUIC server bound to given address.
pub async fn run_quinn_server(addr: SocketAddr, channel_map: ChannelMap) {
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
        let player_id = conn.stable_id().to_string();
        // channel to send from server to client
        let (server_sender, server_receiver) = async_channel::unbounded::<ServerMessage>();
        // channel to send from client to server
        let (client_sender, client_receiver) = async_channel::unbounded::<ClientMessage>();
        // Store the channels in the map
        {
            let mut map = channel_map.lock().unwrap();
            map.insert(
                player_id.clone(),
                MessageChannels {
                    receiver: client_receiver,
                    sender: server_sender,
                },
            );
        }

        // say hello to the client for the client to accept the connection
        let hello_message = ServerMessage::Hello {
            player_id: player_id.clone(),
        };
        let serialized_message = rkyv::to_bytes::<rancor::Error>(&hello_message).unwrap();
        send_stream
            .write_all(&serialized_message)
            .await
            .expect("Failed to write to send stream");

        // send message to sync code that we have new player
        client_sender
            .send(ClientMessage::PlayerJoined {
                player_id: player_id.clone(),
            })
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
                            .send(ClientMessage::Quit {
                                player_id: player_id.clone(),
                            })
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
