use std::str::FromStr;
use std::{error::Error, sync::Arc};

use tokio::sync::watch;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::{
    EXAMPLE_ALPN, LogReceiver, LogSender, MessageSize, PlayerId, ReliableClientMessage, ReliableServerMessage, UNIDIRECTIONAL_STREAM_LIMIT, UnreliableClientMessage, UnreliableServerMessage, log
};
use iroh::{
    Endpoint, EndpointAddr, PublicKey, RelayMode, SecretKey, endpoint
};
use iroh::endpoint::{Connection, QuicTransportConfig, RecvStream, SendStream, VarInt};
use rkyv::rancor;

pub struct Client {
    pub cancel_sender: watch::Sender<bool>,
    pub reliable_server_receiver: async_channel::Receiver<ReliableServerMessage>,
    pub reliable_client_sender: async_channel::Sender<ReliableClientMessage>,
    pub unreliable_server_receiver: async_channel::Receiver<UnreliableServerMessage>,
    pub unreliable_client_sender: async_channel::Sender<UnreliableClientMessage>,
    pub log_receiver: LogReceiver,
    pub join_set: tokio::task::JoinSet<()>,
    pub local_player_id: PlayerId,
    pub endpoint: Endpoint,
}

async fn connect_to_server(
    server_iroh_string: String,
) -> Result<(Endpoint, Connection), Box<dyn Error + Send + Sync + 'static>> {
    let mut rng = rand::rng();
    let secret_key = SecretKey::generate(&mut rng);
    
    let transport_config = QuicTransportConfig::builder()
        .max_concurrent_uni_streams(UNIDIRECTIONAL_STREAM_LIMIT)
        .build();
    
    // Build a `Endpoint`, which uses PublicKeys as endpoint identifiers, uses QUIC for directly connecting to other endpoints, and uses the relay protocol and relay servers to holepunch direct connections between endpoints when there are NATs or firewalls preventing direct connections. If no direct connection can be made, packets are relayed over the relay servers.
    let endpoint = Endpoint::builder()
        // The secret key is used to authenticate with other endpoints. The PublicKey portion of this secret key is how we identify endpoints, often referred to as the `endpoint_id` in our codebase.
        .secret_key(secret_key)
        // set the ALPN protocols this endpoint will accept on incoming connections
        .alpns(vec![EXAMPLE_ALPN.to_vec()])
        // `RelayMode::Default` means that we will use the default relay servers to holepunch and relay.
        // Use `RelayMode::Custom` to pass in a `RelayMap` with custom relay urls.
        // Use `RelayMode::Disable` to disable holepunching and relaying over HTTPS
        // If you want to experiment with relaying using your own relay server, you must pass in the same custom relay url to both the `listen` code AND the `connect` code
        .relay_mode(RelayMode::Default)
        .transport_config(transport_config)
        // you can choose a port to bind to, but passing in `0` will bind the socket to a random available port
        .bind()
        .await?;

    // connect to server
    let server_iroh_id = PublicKey::from_str(&server_iroh_string)
        .map_err(|e| format!("Failed to parse server Iroh ID: {}", e))?;
    let server_endpoint_addr = EndpointAddr::from_parts(server_iroh_id, vec![]);
    let connection = endpoint.connect(server_endpoint_addr, EXAMPLE_ALPN).await?;
    println!("[client] connected: addr={}", connection.remote_id());

    Ok((endpoint, connection))
}

pub fn serialize_reliable_client_message(
    message: &ReliableClientMessage,
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

pub fn serialize_unreliable_client_message(
    message: &UnreliableClientMessage,
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

async fn read_reliable_server_message(
    mut recv_stream: RecvStream,
    cancel_rev: watch::Receiver<bool>,
    server_message_sender: async_channel::Sender<ReliableServerMessage>,
    log_sender: LogSender,
) {
    log(
        &log_sender,
        "[client] start receiving messages from server".into(),
    )
    .await;
    loop {
        // break loop if the cancel receiver is set to true
        if *cancel_rev.borrow() {
            log(
                &log_sender,
                "[client] cancel receiver is set to true, stopping receiving messages from server"
                    .into(),
            )
            .await;
            break;
        }

        // get message from server

        // Read the delimiter first (to see that our frame has started)
        let mut delimiter_buf = [0u8; 1];
        if let Err(e) = recv_stream.read_exact(&mut delimiter_buf).await {
            log(
                &log_sender,
                format!("[client] failed to read delimiter: {e}"),
            )
            .await;
            continue;
        }
        if delimiter_buf != crate::DELIMITER {
            log(
                &log_sender,
                format!("[client] received invalid delimiter: {:?}", delimiter_buf),
            )
            .await;
            continue;
        }
        // Read the size of the message
        let mut size_buf: MessageSize = [0u8; 4];
        if let Err(e) = recv_stream.read_exact(&mut size_buf).await {
            log(&log_sender, format!("[client] failed to read size: {e}")).await;
            continue;
        }
        // Convert the size from bytes to u32
        let size = u32::from_be_bytes(size_buf);
        // then read the actual message (only read as much as we need)
        let mut buf = vec![0u8; size as usize];
        if let Ok(()) = recv_stream.read_exact(&mut buf).await {
            let server_message =
                rkyv::from_bytes::<ReliableServerMessage, rancor::Error>(&buf).unwrap();
            if let Err(send_error) = server_message_sender.send(server_message).await {
                log(
                    &log_sender,
                    format!(
                        "[client] failed to send message to server receiver: {}",
                        send_error
                    ),
                )
                .await;
            }
        } else {
            log(
                &log_sender,
                format!(
                    "[client] failed to receive message from server, stream id: {}",
                    recv_stream.id()
                ),
            )
            .await;
        }
    }
}

async fn send_reliable_client_message(
    mut send_stream: SendStream,
    cancel_recv: watch::Receiver<bool>,
    client_message_receiver: async_channel::Receiver<ReliableClientMessage>,
    log_sender: LogSender,
) {
    log(
        &log_sender,
        format!(
            "[client] start sending messages to server, stream id: {}",
            send_stream.id()
        ),
    )
    .await;
    loop {
        // break loop if the cancel receiver is set to true
        if *cancel_recv.borrow() {
            log(
                &log_sender,
                "[client] cancel receiver is set to true, stopping sending messages to server"
                    .into(),
            )
            .await;
            break;
        }
        // get message from sync client code
        while let Ok(message) = client_message_receiver.recv().await {
            let serialized_message = match serialize_reliable_client_message(&message) {
                Ok(bytes) => bytes,
                Err(e) => {
                    log(
                        &log_sender,
                        format!("[client] failed to serialize message: {}", e),
                    )
                    .await;
                    continue;
                }
            };
            // then send the serialized message
            if let Ok(()) = send_stream.write_all(&serialized_message).await {
            } else {
                log(
                    &log_sender,
                    format!("[client] failed to send message: {:?}", message),
                )
                .await;
            }
        }
    }
}

async fn read_unreliable_server_message(
    connection: Connection,
    cancel_recv: watch::Receiver<bool>,
    server_message_sender: async_channel::Sender<UnreliableServerMessage>,
    log_sender: LogSender,
) {
    log(
        &log_sender,
        "[client] start receiving unreliable messages from server".into(),
    )
    .await;
    'incoming_loop: loop {
        match connection.accept_uni().await {
            Ok(mut recv_stream) => {
                // break loop if the cancel receiver is set to true
                if *cancel_recv.borrow() {
                    log(&log_sender, "[client] cancel receiver is set to true, stopping receiving messages from server".into()).await;
                    break;
                }

                // get message from server

                // Read the delimiter first (to see that our frame has started)
                let mut delimiter_buf = [0u8; 1];
                if let Err(e) = recv_stream.read_exact(&mut delimiter_buf).await {
                    log(
                        &log_sender,
                        format!("[client] failed to read delimiter: {e}"),
                    )
                    .await;
                    continue;
                }
                if delimiter_buf != crate::DELIMITER {
                    log(
                        &log_sender,
                        format!("[client] received invalid delimiter: {:?}", delimiter_buf),
                    )
                    .await;
                    continue;
                }
                // Read the size of the message
                let mut size_buf: MessageSize = [0u8; 4];
                if let Err(e) = recv_stream.read_exact(&mut size_buf).await {
                    log(&log_sender, format!("[client] failed to read size: {e}")).await;
                    continue;
                }
                // Convert the size from bytes to u32
                let size = u32::from_be_bytes(size_buf);
                // then read the actual message (only read as much as we need)
                let mut buf = vec![0u8; size as usize];
                if let Ok(()) = recv_stream.read_exact(&mut buf).await {
                    let server_message =
                        rkyv::from_bytes::<UnreliableServerMessage, rancor::Error>(&buf).unwrap();
                    if let Err(send_error) = server_message_sender.send(server_message).await {
                        log(
                            &log_sender,
                            format!(
                                "[client] failed to send message to server receiver: {}",
                                send_error
                            ),
                        )
                        .await;
                    }
                } else {
                    log(
                        &log_sender,
                        format!(
                            "[client] failed to receive message from server, stream id: {}",
                            recv_stream.id()
                        ),
                    )
                    .await;
                }
                // Discard the stream after reading the message
                recv_stream.stop(VarInt::from_u32(0)).ok();
            }
            Err(e) => {
                log(
                    &log_sender,
                    format!("[client] failed to accept unidirectional stream: {}", e),
                )
                .await;
                break 'incoming_loop;
            }
        }
    }
}

async fn send_unreliable_client_message(
    connection: Connection,
    cancel_recv: watch::Receiver<bool>,
    client_message_receiver: async_channel::Receiver<UnreliableClientMessage>,
    log_sender: LogSender,
) {
    log(
        &log_sender,
        "[client] start sending unreliable messages to server".into(),
    )
    .await;
    'outgoing_loop: loop {
        // break loop if the cancel receiver is set to true
        if *cancel_recv.borrow() {
            log(
                &log_sender,
                "[client] cancel receiver is set to true, stopping sending messages to server"
                    .into(),
            )
            .await;
            break;
        }
        // get message from sync client code
        while let Ok(message) = client_message_receiver.recv().await {
            let serialized_message = match serialize_unreliable_client_message(&message) {
                Ok(bytes) => bytes,
                Err(e) => {
                    log(
                        &log_sender,
                        format!("[client] failed to serialize message: {}", e),
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
                            format!("[client] failed to send message: {:?}", message),
                        )
                        .await;
                    }
                    let _ = send_stream.finish();
                }
                Err(e) => {
                    log(
                        &log_sender,
                        format!("[client] failed to open unidirectional stream: {}", e),
                    )
                    .await;
                    break 'outgoing_loop;
                }
            }
        }
    }
}

async fn connect_client_to_server(
    endpoint: Endpoint,
    connection: Connection,
) -> Result<Client, Box<dyn Error + Send + Sync + 'static>> {
    let (cancel_sender, cancel_receiver) = watch::channel(false);
    println!("[client] connecting channel to server");
    let (mut send_stream, mut recv_stream) = connection
        .accept_bi()
        .await
        .map_err(|e| format!("Failed to accept bidirectional stream: {}", e))?;
    println!("[client] accepted bidirectional stream");
    // Create a channel for sending message to the server and receiving messages from it.
    let (reliable_server_sender, reliable_server_receiver) =
        async_channel::unbounded::<ReliableServerMessage>();
    // Create a channel for receiving messages from the tokio task to sync code
    let (reliable_client_sender, reliable_client_receiver) =
        async_channel::unbounded::<ReliableClientMessage>();

    let (unreliable_server_sender, unreliable_server_receiver) =
        async_channel::unbounded::<UnreliableServerMessage>();
    let (unreliable_client_sender, unreliable_client_receiver) =
        async_channel::unbounded::<UnreliableClientMessage>();

    // Create a log channel for forwarding messages to the Godot side
    let (log_sender, log_receiver) = async_channel::unbounded::<String>();

    // return handle to to the connection tasks so we can drop it later
    let mut join_set = tokio::task::JoinSet::new();
    let cancel_rev = cancel_receiver.clone();
    join_set.spawn(read_reliable_server_message(
        recv_stream,
        cancel_rev,
        reliable_server_sender,
        log_sender.clone(),
    ));

    let cancel_rev = cancel_receiver.clone();
    join_set.spawn(read_unreliable_server_message(
        connection.clone(),
        cancel_rev,
        unreliable_server_sender,
        log_sender.clone(),
    ));

    let cancel_rev = cancel_receiver.clone();
    join_set.spawn(send_reliable_client_message(
        send_stream,
        cancel_rev,
        reliable_client_receiver,
        log_sender.clone(),
    ));

    let cancel_rev = cancel_receiver.clone();
    join_set.spawn(send_unreliable_client_message(
        connection,
        cancel_rev,
        unreliable_client_receiver,
        log_sender,
    ));

    Ok(Client {
        cancel_sender,
        reliable_server_receiver,
        reliable_client_sender,
        unreliable_server_receiver,
        unreliable_client_sender,
        log_receiver,
        join_set,
        local_player_id: PlayerId::default(),
        endpoint
    })
}

pub async fn run_client(server_iroh_string: String) -> Result<Client, Box<dyn Error + Send + Sync + 'static>> {
    console_subscriber::init();
    let (endpoint, connection) = connect_to_server(server_iroh_string).await?;
    let client = connect_client_to_server(endpoint, connection).await?;
    Ok(client)
}
