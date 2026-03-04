//! This example demonstrates how to make a QUIC connection that ignores the server certificate.
//!
//! Checkout the `README.md` for guidance.

use std::{
    collections::HashMap,
    error::Error,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
};

use crate::{ClientMessage, DELIMITER, MessageSize, PlayerId, ServerMessage};
use quinn::{
    Endpoint, ServerConfig,
    rustls::{self, pki_types::PrivatePkcs8KeyDer},
};
use rkyv::rancor;
use rustls::pki_types::CertificateDer;
use tokio::{sync::watch, task::JoinSet};

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
    let priv_key = PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());

    let mut server_config =
        ServerConfig::with_single_cert(vec![cert_der.clone()], priv_key.into())?;
    let transport_config = Arc::get_mut(&mut server_config.transport).unwrap();
    transport_config.max_concurrent_uni_streams(0_u8.into());

    Ok((server_config, cert_der))
}

pub async fn run_server()
-> Result<Server, Box<dyn Error + Send + Sync + 'static>> {
    //console_subscriber::init();
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
    let channel_map = Arc::new(Mutex::new(HashMap::<PlayerId, MessageChannels>::new()));
    let mut join_set = JoinSet::new();
    join_set.spawn(run_quinn_server(addr, channel_map.clone()));

    Ok(Server {
        channel_map,
        join_set,
    })
}

#[derive(Debug)]
pub struct MessageChannels {
    pub cancel_sender: watch::Sender<bool>,
    pub receiver: async_channel::Receiver<ClientMessage>,
    pub sender: async_channel::Sender<ServerMessage>,
}

pub fn serialize_server_message(
    message: &ServerMessage,
) -> Result<Vec<u8>, Box<dyn Error + Send + Sync + 'static>> {
    let serialized_message = rkyv::to_bytes::<rancor::Error>(message);
    match serialized_message {
        Ok(bytes) => {
            // create the header with delimiter and size
            let size: MessageSize = (bytes.len() as u32).to_be_bytes();
            // attach the start delimiter to the header (this lets the server know that a new message is coming)
            let header = [&crate::DELIMITER[..], &size[..]].concat();
            // prepend the header to the serialized message
            let serialized_message = [&header, bytes.as_slice()].concat();
            return Ok(serialized_message);
        }
        Err(e) => return Err(Box::new(e)),
    }
}

/// Runs a QUIC server bound to given address.
pub async fn run_quinn_server(
    addr: SocketAddr,
    channel_map: ChannelMap,
) -> tokio::task::JoinSet<()> {
    console_subscriber::init();
    let (endpoint, _server_cert) = make_server_endpoint(addr).unwrap();

    // add join set to make sure we don't leak any tasks
    let mut join_set = tokio::task::JoinSet::new();

    join_set.spawn(async move {
        // this will automatically drop when we drop the parent join_set
        // since dropping the parent join_set will stop all the tasks in it
        let mut join_set = JoinSet::new();
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
            let (cancel_sender, cancel_receiver) = watch::channel(false);
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
                        cancel_sender,
                        receiver: client_receiver,
                        sender: server_sender,
                    },
                );
            }

            // say hello to the client for the client to accept the connection
            let hello_message = ServerMessage::Hello {
                player_id: player_id.clone(),
            };
            let serialized_message = serialize_server_message(&hello_message)
                .expect("Failed to serialize hello message");
            // then send the serialized message
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

            let cancel_recv = cancel_receiver.clone();

            join_set.spawn(async move {
                'sending_loop: loop {
                    if *cancel_recv.borrow() {
                        println!("[server] cancel receiver is set to true, stopping sending messages to client");
                        break 'sending_loop;
                    }
                    // get message from sync server code
                    while let Ok(message) = server_receiver.recv().await {
                        // serialize the message
                        let serialized_message = serialize_server_message(&message)
                            .expect("Failed to serialize message");
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

            let cancel_recv = cancel_receiver.clone();

            join_set.spawn(async move {
                println!(
                    "[server] (loop) waiting for messages from client, stream id: {}",
                    recv_stream.id()
                );
                'receive_loop: loop {
                    if *cancel_recv.borrow() {
                        println!("[server] cancel receiver is set to true, stopping receiving messages from client");
                        break 'receive_loop;
                    }

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
                        let message =
                            rkyv::from_bytes::<ClientMessage, rancor::Error>(&buf).unwrap();
                        //println!("[server] received message: {:?}", message);
                        client_sender.send(message).await.unwrap();
                    }
                    //println!("[server] buffer {:?}", buf);
                }
            });
        }
    });

    join_set
}

pub struct Server {
    pub channel_map: ChannelMap,
    pub join_set: JoinSet<JoinSet<()>>,
}

