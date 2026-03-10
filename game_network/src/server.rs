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
    DELIMITER, EXAMPLE_ALPN, LogSender, MessageSize, PlayerId, ReliableClientMessage,
    ReliableServerMessage, UNIDIRECTIONAL_STREAM_LIMIT, UnreliableClientMessage,
    UnreliableServerMessage, log,
};
use iroh::{Endpoint, RelayMode, SecretKey, endpoint};
use iroh::{
    endpoint::{Connection, QuicTransportConfig, RecvStream, SendStream, VarInt},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use rkyv::rancor;
use tokio::{sync::watch, task::JoinSet};

#[derive(Debug, Clone)]
pub struct ChannelMap {
    inner: Arc<Mutex<HashMap<PlayerId, MessageChannels>>>,
}

impl ChannelMap {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::<PlayerId, MessageChannels>::new())),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (PlayerId, MessageChannels)> {
        let guard = self.inner.lock().unwrap();
        guard
            .iter()
            .map(|(player_id, channels)| (player_id.clone(), channels.clone()))
            .collect::<Vec<_>>()
            .into_iter()
    }

    pub fn get(&self, player_id: &PlayerId) -> Option<MessageChannels> {
        let guard = self.inner.lock().unwrap();
        guard.get(player_id).cloned()
    }

    pub fn insert(&self, player_id: PlayerId, channels: MessageChannels) {
        let mut guard = self.inner.lock().unwrap();
        guard.insert(player_id, channels);
    }

    pub fn remove(&self, player_id: &PlayerId) {
        let mut guard = self.inner.lock().unwrap();
        guard.remove(player_id);
    }

    pub fn keys(&self) -> Vec<PlayerId> {
        let guard = self.inner.lock().unwrap();
        guard.keys().cloned().collect()
    }

    pub fn clear(&self) {
        let mut guard = self.inner.lock().unwrap();
        guard.clear();
    }
}

pub struct Server {
    pub channel_map: ChannelMap,
    pub router: Router,
    pub log_receiver: crate::LogReceiver,
    endpoint: Endpoint,
}

impl Server {
    pub fn get_server_id(&self) -> String {
        self.endpoint.id().to_string()
    }
}

pub async fn make_server_endpoint() -> Result<Endpoint, Box<dyn Error + Send + Sync + 'static>> {
    let secret_key = SecretKey::generate(&mut rand::rng());

    // Build a `Endpoint` for the server
    let endpoint = Endpoint::builder()
        .secret_key(secret_key)
        .alpns(vec![EXAMPLE_ALPN.to_vec()])
        .bind()
        .await?;

    Ok(endpoint)
}

pub async fn run_server() -> Result<Server, Box<dyn Error + Send + Sync + 'static>> {
    //console_subscriber::init();
    let channel_map = ChannelMap::new();
    let (log_sender, log_receiver) = async_channel::unbounded::<String>();
    let endpoint = make_server_endpoint().await?;
    let router = Router::builder(endpoint.clone())
        .accept(
            EXAMPLE_ALPN,
            ServerProtocol {
                channel_map: channel_map.clone(),
                log_sender: log_sender.clone(),
            },
        )
        .spawn();

    Ok(Server {
        channel_map,
        router,
        log_receiver,
        endpoint,
    })
}

#[derive(Debug, Clone)]
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
    mut send_stream: SendStream,
    cancel_recv: watch::Receiver<bool>,
    reliable_server_receiver: async_channel::Receiver<ReliableServerMessage>,
    reliable_client_sender: async_channel::Sender<ReliableClientMessage>,
    player_id: PlayerId,
    log_sender: LogSender,
) {
    'sending_loop: loop {
        if *cancel_recv.borrow() {
            log(
                &log_sender,
                "[server] cancel receiver is set to true, stopping sending messages to client"
                    .into(),
            )
            .await;
            break 'sending_loop;
        }
        // get message from sync server code
        while let Ok(message) = reliable_server_receiver.recv().await {
            // serialize the message
            let serialized_message =
                serialize_reliable_server_message(&message).expect("Failed to serialize message");
            // then send the serialized message
            if let Err(e) = send_stream.write_all(&serialized_message).await {
                log(
                    &log_sender,
                    format!("[server] failed to send message: {:?}, {e}", message),
                )
                .await;

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
    mut recv_stream: RecvStream,
    cancel_recv: watch::Receiver<bool>,
    reliable_client_sender: async_channel::Sender<ReliableClientMessage>,
    log_sender: LogSender,
) {
    log(
        &log_sender,
        format!(
            "[server] (loop) waiting for messages from client, stream id: {}",
            recv_stream.id()
        ),
    )
    .await;
    'receive_loop: loop {
        if *cancel_recv.borrow() {
            log(
                &log_sender,
                "[server] cancel receiver is set to true, stopping receiving messages from client"
                    .into(),
            )
            .await;
            break 'receive_loop;
        }

        // get message from client

        // first read the delimiter
        let mut delimiter_buf = [0; 1];
        if let Err(e) = recv_stream.read_exact(&mut delimiter_buf).await {
            log(
                &log_sender,
                format!("[server] failed to read delimiter: {e}"),
            )
            .await;
            // If we fail to read the delimiter, it means the client has disconnected
            break 'receive_loop;
        }

        if delimiter_buf != DELIMITER {
            log(
                &log_sender,
                format!("[server] received invalid delimiter: {:?}", delimiter_buf),
            )
            .await;
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
    connection: Connection,
    cancel_recv: watch::Receiver<bool>,
    unreliable_message_sender: async_channel::Sender<UnreliableClientMessage>,
    log_sender: LogSender,
) {
    log(
        &log_sender,
        format!(
            "[server] (loop) waiting for unreliable messages from client, connection id: {}",
            connection.stable_id()
        ),
    )
    .await;
    log(
        &log_sender,
        "[server] start receiving unreliable messages from client".into(),
    )
    .await;
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
                    log(
                        &log_sender,
                        format!("[server] failed to read delimiter: {e}"),
                    )
                    .await;
                    continue;
                }
                if delimiter_buf != crate::DELIMITER {
                    log(
                        &log_sender,
                        format!("[server] received invalid delimiter: {:?}", delimiter_buf),
                    )
                    .await;
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
                        log(
                            &log_sender,
                            format!(
                                "[server] failed to send message to server receiver: {}",
                                send_error
                            ),
                        )
                        .await;
                    }
                } else {
                    log(
                        &log_sender,
                        format!(
                            "[server] failed to receive message from client, stream id: {}",
                            recv_stream.id()
                        ),
                    )
                    .await;
                }
                recv_stream.stop(VarInt::from_u32(0)).ok();
            }
            Err(e) => {
                log(
                    &log_sender,
                    format!("[server] failed to accept unidirectional stream: {}", e),
                )
                .await;
                break 'incoming_loop;
            }
        }
    }
}

async fn send_unreliable_server_message(
    connection: Connection,
    cancel_recv: watch::Receiver<bool>,
    unreliable_server_receiver: async_channel::Receiver<UnreliableServerMessage>,
    log_sender: LogSender,
) {
    log(
        &log_sender,
        "[server] start sending unreliable messages to clients".into(),
    )
    .await;
    'outgoing_loop: loop {
        // break loop if the cancel receiver is set to true
        if *cancel_recv.borrow() {
            log(
                &log_sender,
                "[server] cancel receiver is set to true, stopping sending messages to client"
                    .into(),
            )
            .await;
            break;
        }
        // get message from sync server code
        while let Ok(message) = unreliable_server_receiver.recv().await {
            let serialized_message = match serialize_unreliable_server_message(&message) {
                Ok(bytes) => bytes,
                Err(e) => {
                    log(
                        &log_sender,
                        format!("[server] failed to serialize message: {}", e),
                    )
                    .await;
                    continue;
                }
            };
            // then send the serialized message
            match connection.open_uni().await {
                Ok(mut send_stream) => {
                    if let Ok(()) = send_stream.write_all(&serialized_message).await {
                    } else {
                        log(
                            &log_sender,
                            format!("[server] failed to send message: {:?}", message),
                        )
                        .await;
                    }
                    let _ = send_stream.finish();
                }
                Err(e) => {
                    log(
                        &log_sender,
                        format!("[server] failed to open unidirectional stream: {}", e),
                    )
                    .await;
                    break 'outgoing_loop;
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
struct ServerProtocol {
    channel_map: ChannelMap,
    log_sender: LogSender,
}

impl ProtocolHandler for ServerProtocol {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        log(
            &self.log_sender,
            format!("[server] connection accepted: addr={}", conn.remote_id()),
        )
        .await;
        let mut join_set = JoinSet::new();
        let (mut send_stream, mut recv_stream) = conn.open_bi().await.unwrap();
        log(
            &self.log_sender,
            "[server] opened bidirectional stream".into(),
        )
        .await;
        // Get the remote ID for this connection
        let player_id = conn.remote_id().to_string();
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
        self.channel_map.insert(
            player_id.clone(),
            MessageChannels {
                cancel_sender,
                reliable_receiver: reliable_client_receiver,
                reliable_sender: reliable_server_sender,
                unreliable_receiver: unreliable_client_receiver,
                unreliable_sender: unreliable_server_sender,
            },
        );

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
            self.log_sender.clone(),
        ));

        let cancel_recv = cancel_receiver.clone();

        join_set.spawn(run_reliable_server_recv_stream(
            recv_stream,
            cancel_recv,
            reliable_client_sender.clone(),
            self.log_sender.clone(),
        ));

        let cancel_recv = cancel_receiver.clone();
        join_set.spawn(read_unreliable_client_message(
            conn.clone(),
            cancel_recv,
            unreliable_client_sender,
            self.log_sender.clone(),
        ));

        let cancel_recv = cancel_receiver.clone();
        join_set.spawn(send_unreliable_server_message(
            conn.clone(),
            cancel_recv,
            unreliable_server_receiver,
            self.log_sender.clone(),
        ));

        conn.closed().await;

        log(
            &self.log_sender,
            format!("[server] connection closed: addr={}", conn.remote_id()),
        )
        .await;
        Ok(())
    }
}
