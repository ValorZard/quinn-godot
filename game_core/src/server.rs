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
    DELIMITER, LogSender, MessageSize, PlayerId, ReliableClientMessage, ReliableServerMessage, UNIDIRECTIONAL_STREAM_LIMIT, UnreliableClientMessage, UnreliableServerMessage, log
};
use quinn::{
    Endpoint, ServerConfig, VarInt, rustls::{self, pki_types::PrivatePkcs8KeyDer}
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
    transport_config.max_concurrent_uni_streams(UNIDIRECTIONAL_STREAM_LIMIT);

    Ok((server_config, cert_der))
}

pub async fn run_server() -> Result<Server, Box<dyn Error + Send + Sync + 'static>> {
    //console_subscriber::init();
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
    let channel_map = Arc::new(Mutex::new(HashMap::<PlayerId, MessageChannels>::new()));
    let (log_sender, log_receiver) = async_channel::unbounded::<String>();
    let mut join_set = JoinSet::new();
    join_set.spawn(run_quinn_server(addr, channel_map.clone(), log_sender));

    Ok(Server {
        channel_map,
        join_set,
        log_receiver,
    })
}

#[derive(Debug)]
pub struct MessageChannels {
    pub cancel_sender: watch::Sender<bool>,
    pub reliable_receiver: async_channel::Receiver<ReliableClientMessage>,
    pub reliable_sender: async_channel::Sender<ReliableServerMessage>,
    pub unreliable_receiver: async_channel::Receiver<UnreliableClientMessage>,
    pub unreliable_sender: async_channel::Sender<UnreliableServerMessage>,
}

pub fn serialize_reliable_server_message(
    message: &ReliableServerMessage,
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

pub fn serialize_unreliable_server_message(
    message: &UnreliableServerMessage,
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

pub async fn run_reliable_server_send_stream(
    mut send_stream: quinn::SendStream,
    cancel_recv: watch::Receiver<bool>,
    reliable_server_receiver: async_channel::Receiver<ReliableServerMessage>,
    reliable_client_sender: async_channel::Sender<ReliableClientMessage>,
    player_id: PlayerId,
    log_sender: LogSender,
) {
    'sending_loop: loop {
        if *cancel_recv.borrow() {
            log(&log_sender, "[server] cancel receiver is set to true, stopping sending messages to client".into()).await;
            break 'sending_loop;
        }
        // get message from sync server code
        while let Ok(message) = reliable_server_receiver.recv().await {
            // serialize the message
            let serialized_message =
                serialize_reliable_server_message(&message).expect("Failed to serialize message");
            // then send the serialized message
            if let Err(e) = send_stream.write_all(&serialized_message).await {
                log(&log_sender, format!("[server] failed to send message: {:?}, {e}", message)).await;

                // special edge case for quitting
                if let Ok(()) = reliable_client_sender
                    .send(ReliableClientMessage::Quit {
                        player_id: player_id.clone(),
                    })
                    .await
                {
                    log(&log_sender, "[server] sent quit message to client".into()).await;
                }
            }
        }
    }
}

pub async fn run_reliable_server_recv_stream(
    mut recv_stream: quinn::RecvStream,
    cancel_recv: watch::Receiver<bool>,
    reliable_client_sender: async_channel::Sender<ReliableClientMessage>,
    log_sender: LogSender,
) {
    log(&log_sender, format!("[server] (loop) waiting for messages from client, stream id: {}", recv_stream.id())).await;
    'receive_loop: loop {
        if *cancel_recv.borrow() {
            log(&log_sender, "[server] cancel receiver is set to true, stopping receiving messages from client".into()).await;
            break 'receive_loop;
        }

        // get message from client

        // first read the delimiter
        let mut delimiter_buf = [0; 1];
        if let Err(e) = recv_stream.read_exact(&mut delimiter_buf).await {
            log(&log_sender, format!("[server] failed to read delimiter: {e}")).await;
            // If we fail to read the delimiter, it means the client has disconnected
            break 'receive_loop;
        }

        if delimiter_buf != DELIMITER {
            log(&log_sender, format!("[server] received invalid delimiter: {:?}", delimiter_buf)).await;
            continue 'receive_loop;
        }

        // Read the size of the message
        let mut size_buf: MessageSize = [0u8; 4];
        if let Err(e) = recv_stream.read_exact(&mut size_buf).await {
            log(&log_sender, format!("[server] failed to read size: {e}")).await;
            continue 'receive_loop;
        }
        // Convert the size from bytes to u32
        let size = u32::from_be_bytes(size_buf);
        // then read the actual message (only read as much as we need)
        let mut buf = vec![0u8; size as usize];
        if let Ok(()) = recv_stream.read_exact(&mut buf).await {
            // Deserialize the message
            let message = rkyv::from_bytes::<ReliableClientMessage, rancor::Error>(&buf).unwrap();
            reliable_client_sender.send(message).await.unwrap();
        }
    }
}

async fn read_unreliable_client_message(
    connection: quinn::Connection,
    cancel_recv: watch::Receiver<bool>,
    unreliable_message_sender: async_channel::Sender<UnreliableClientMessage>,
    log_sender: LogSender,
) {
    log(&log_sender, format!("[server] (loop) waiting for unreliable messages from client, connection id: {}", connection.stable_id())).await;
    log(&log_sender, "[server] start receiving unreliable messages from client".into()).await;
    'incoming_loop: loop {
        match connection.accept_uni().await {
            Ok(mut recv_stream) => {
                // break loop if the cancel receiver is set to true
                if *cancel_recv.borrow() {
                    log(&log_sender, "[server] cancel receiver is set to true, stopping receiving messages from client".into()).await;
                    break;
                }

                // get message from client

                // Read the delimiter first (to see that our frame has started)
                let mut delimiter_buf = [0u8; 1];
                if let Err(e) = recv_stream.read_exact(&mut delimiter_buf).await {
                    log(&log_sender, format!("[server] failed to read delimiter: {e}")).await;
                    continue;
                }
                if delimiter_buf != crate::DELIMITER {
                    log(&log_sender, format!("[server] received invalid delimiter: {:?}", delimiter_buf)).await;
                    continue;
                }
                // Read the size of the message
                let mut size_buf: MessageSize = [0u8; 4];
                if let Err(e) = recv_stream.read_exact(&mut size_buf).await {
                    log(&log_sender, format!("[server] failed to read size: {e}")).await;
                    continue;
                }
                // Convert the size from bytes to u32
                let size = u32::from_be_bytes(size_buf);
                // then read the actual message (only read as much as we need)
                let mut buf = vec![0u8; size as usize];
                if let Ok(()) = recv_stream.read_exact(&mut buf).await {
                    let client_message =
                        rkyv::from_bytes::<UnreliableClientMessage, rancor::Error>(&buf).unwrap();
                    if let Err(send_error) = unreliable_message_sender.send(client_message).await {
                        log(&log_sender, format!("[server] failed to send message to server receiver: {}", send_error)).await;
                    }
                } else {
                    log(&log_sender, format!("[server] failed to receive message from client, stream id: {}", recv_stream.id())).await;
                }
                recv_stream.stop(VarInt::from_u32(0)).ok();
            }
            Err(e) => {
                log(&log_sender, format!("[server] failed to accept unidirectional stream: {}", e)).await;
                break 'incoming_loop;
            }
        }
    }
}

async fn send_unreliable_server_message(
    connection: quinn::Connection,
    cancel_recv: watch::Receiver<bool>,
    unreliable_server_receiver: async_channel::Receiver<UnreliableServerMessage>,
    log_sender: LogSender,
) {
    log(&log_sender, "[server] start sending unreliable messages to clients".into()).await;
    'outgoing_loop: loop {
        // break loop if the cancel receiver is set to true
        if *cancel_recv.borrow() {
            log(&log_sender, "[server] cancel receiver is set to true, stopping sending messages to client".into()).await;
            break;
        }
        // get message from sync server code
        while let Ok(message) = unreliable_server_receiver.recv().await {
            let serialized_message = match serialize_unreliable_server_message(&message) {
                Ok(bytes) => bytes,
                Err(e) => {
                    log(&log_sender, format!("[server] failed to serialize message: {}", e)).await;
                    continue;
                }
            };
            // then send the serialized message
            match connection.open_uni().await {
                Ok(mut send_stream) => {
                    if let Ok(()) = send_stream.write_all(&serialized_message).await {
                    } else {
                        log(&log_sender, format!("[server] failed to send message: {:?}", message)).await;
                    }
                    let _ = send_stream.finish();
                }
                Err(e) => {
                    log(&log_sender, format!("[server] failed to open unidirectional stream: {}", e)).await;
                    break 'outgoing_loop;
                }
            }
        }
    }
}

/// Runs a QUIC server bound to given address.
pub async fn run_quinn_server(
    addr: SocketAddr,
    channel_map: ChannelMap,
    log_sender: LogSender,
) -> tokio::task::JoinSet<()> {
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
            log(&log_sender, format!("[server] connection accepted: addr={}", conn.remote_address())).await;
            let (mut send_stream, mut recv_stream) = conn.open_bi().await.unwrap();
            log(&log_sender, "[server] opened bidirectional stream".into()).await;
            // Create a new player ID for this connection
            let player_id = conn.stable_id().to_string();
            let (cancel_sender, cancel_receiver) = watch::channel(false);
            // channel to send from server to client
            let (reliable_server_sender, reliable_server_receiver) =
                async_channel::unbounded::<ReliableServerMessage>();
            // channel to send from client to server
            let (reliable_client_sender, reliable_client_receiver) =
                async_channel::unbounded::<ReliableClientMessage>();
            // channel for unreliable messages from client to server
            let (unreliable_server_sender, unreliable_server_receiver) =
                async_channel::unbounded::<UnreliableServerMessage>();
            let (unreliable_client_sender, unreliable_client_receiver) =
                async_channel::unbounded::<UnreliableClientMessage>();
            // Store the channels in the map
            {
                let mut map = channel_map.lock().unwrap();
                map.insert(
                    player_id.clone(),
                    MessageChannels {
                        cancel_sender,
                        reliable_receiver: reliable_client_receiver,
                        reliable_sender: reliable_server_sender,
                        unreliable_receiver: unreliable_client_receiver,
                        unreliable_sender: unreliable_server_sender,
                    },
                );
            }

            // say hello to the client for the client to accept the connection
            let hello_message = ReliableServerMessage::Hello {
                player_id: player_id.clone(),
            };
            let serialized_message = serialize_reliable_server_message(&hello_message)
                .expect("Failed to serialize hello message");
            // then send the serialized message
            send_stream
                .write_all(&serialized_message)
                .await
                .expect("Failed to write to send stream");

            // send message to sync code that we have new player
            reliable_client_sender
                .send(ReliableClientMessage::PlayerJoined {
                    player_id: player_id.clone(),
                })
                .await
                .unwrap();

            let client_quit_sender = reliable_client_sender.clone();

            let cancel_recv = cancel_receiver.clone();

            join_set.spawn(run_reliable_server_send_stream(
                send_stream,
                cancel_recv,
                reliable_server_receiver,
                client_quit_sender,
                player_id.clone(),
                log_sender.clone(),
            ));

            let cancel_recv = cancel_receiver.clone();

            join_set.spawn(run_reliable_server_recv_stream(
                recv_stream,
                cancel_recv,
                reliable_client_sender.clone(),
                log_sender.clone(),
            ));

            let cancel_recv = cancel_receiver.clone();
            join_set.spawn(read_unreliable_client_message(
                conn.clone(),
                cancel_recv,
                unreliable_client_sender,
                log_sender.clone(),
            ));

            let cancel_recv = cancel_receiver.clone();
            join_set.spawn(send_unreliable_server_message(
                conn.clone(),
                cancel_recv,
                unreliable_server_receiver,
                log_sender.clone(),
            ));
        }
    });

    join_set
}

pub struct Server {
    pub channel_map: ChannelMap,
    pub join_set: JoinSet<JoinSet<()>>,
    pub log_receiver: crate::LogReceiver,
}
