use core::net;
use std::{error::Error, f32::consts::E, sync::Arc};

use bytes::Bytes;
use hecs::World;
use tokio::sync::watch;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::{
    ClientMessage, DELIMITER, MAX_PACKET_SIZE, MessageSize, PlayerId, PlayerPosition, ServerMessage,
};
use quinn::{
    ClientConfig, Connection, Endpoint, RecvStream, SendStream, ServerConfig, VarInt,
    rustls::{self, pki_types::PrivatePkcs8KeyDer},
};
use quinn_proto::crypto::rustls::QuicClientConfig;
use rkyv::{Archived, rancor, ser};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};

use crate::server;

async fn connect_to_server(
    server_addr: SocketAddr,
) -> Result<(Endpoint, Connection), Box<dyn Error + Send + Sync + 'static>> {
    let mut endpoint = Endpoint::client(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;

    endpoint.set_default_client_config(ClientConfig::new(Arc::new(QuicClientConfig::try_from(
        rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(SkipServerVerification::new())
            .with_no_client_auth(),
    )?)));

    // connect to server
    let connection = endpoint
        .connect(server_addr, "localhost")
        .unwrap()
        .await
        .unwrap();
    println!("[client] connected: addr={}", connection.remote_address());

    Ok((endpoint, connection))
}

/// Dummy certificate verifier that treats any certificate as valid.
/// NOTE, such verification is vulnerable to MITM attacks, but convenient for testing.
#[derive(Debug)]
struct SkipServerVerification(Arc<rustls::crypto::CryptoProvider>);

impl SkipServerVerification {
    fn new() -> Arc<Self> {
        Arc::new(Self(Arc::new(rustls::crypto::ring::default_provider())))
    }
}

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

pub fn serialize_client_message(
    message: &ClientMessage,
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

async fn connect_channel_to_server(
    connection: Connection,
) -> Result<
    (
        watch::Sender<bool>,
        async_channel::Receiver<ServerMessage>,
        async_channel::Sender<ClientMessage>,
        tokio::task::JoinSet<()>,
    ),
    Box<dyn Error + Send + Sync + 'static>,
> {
    let (cancel_sender, cancel_receiver) = watch::channel(false);
    println!("[client] connecting channel to server");
    let (mut send_stream, mut recv_stream) = connection
        .accept_bi()
        .await
        .map_err(|e| format!("Failed to accept bidirectional stream: {}", e))?;
    println!("[client] accepted bidirectional stream");
    // Create a channel for sending message to the server and receiving messages from it.
    let (server_sender, server_receiver) = async_channel::unbounded::<ServerMessage>();
    // Create a channel for receiving messages from the tokio task to sync code
    let (client_sender, client_receiver) = async_channel::unbounded::<ClientMessage>();
    // return handle to to the connection tasks so we can drop it later
    let mut join_set = tokio::task::JoinSet::new();
    let cancel_rev = cancel_receiver.clone();
    join_set.spawn(async move {
        println!("[client] start receiving messages from server");
        loop {
            // break loop if the cancel receiver is set to true
            if *cancel_rev.borrow() {
                println!("[client] cancel receiver is set to true, stopping receiving messages from server");
                break;
            }

            // get message from server

            // Read the delimiter first (to see that our frame has started)
            let mut delimiter_buf = [0u8; 1];
            if let Err(e) = recv_stream.read_exact(&mut delimiter_buf).await {
                println!("[client] failed to read delimiter: {e}");
                continue;
            }
            if delimiter_buf != crate::DELIMITER {
                println!("[client] received invalid delimiter: {:?}", delimiter_buf);
                continue;
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
                //println!("bytes read: {:?}", buf);
                let server_message =
                    rkyv::from_bytes::<ServerMessage, rancor::Error>(&buf).unwrap();
                if let Err(send_error) = server_sender.send(server_message).await {
                    println!(
                        "[client] failed to send message to server receiver: {}",
                        send_error
                    );
                }
            } else {
                println!(
                    "[client] failed to receive message from server, stream id: {}",
                    recv_stream.id()
                );
            }
        }
    });

    let cancel_rev = cancel_receiver.clone();
    join_set.spawn(async move {
        println!(
            "[client] start sending messages to server, stream id: {}",
            send_stream.id()
        );
        loop {
            // break loop if the cancel receiver is set to true
            if *cancel_rev.borrow() {
                println!(
                    "[client] cancel receiver is set to true, stopping sending messages to server"
                );
                break;
            }
            // get message from sync client code
            while let Ok(message) = client_receiver.recv().await {
                let serialized_message = match serialize_client_message(&message) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        println!("[client] failed to serialize message: {}", e);
                        continue;
                    }
                };
                // then send the serialized message
                if let Ok(()) = send_stream.write_all(&serialized_message).await {
                } else {
                    println!("[client] failed to send message: {:?}", message);
                }
            }
        }
    });

    Ok((cancel_sender, server_receiver, client_sender, join_set))
}

pub async fn run_client() -> Result<
    (
        watch::Sender<bool>,
        async_channel::Receiver<ServerMessage>,
        async_channel::Sender<ClientMessage>,
        tokio::task::JoinSet<()>,
    ),
    Box<dyn Error + Send + Sync + 'static>,
> {
    let server_address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
    let (endpoint, connection) = connect_to_server(server_address).await?;
    let (cancel_sender, server_receiver, client_sender, join_set) =
        connect_channel_to_server(connection).await?;
    Ok((cancel_sender, server_receiver, client_sender, join_set))
}
