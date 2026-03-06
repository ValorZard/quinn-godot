use iroh::endpoint::VarInt;
use rkyv::{Archive, Deserialize, Serialize};

pub mod client;
pub mod server;

pub type PlayerId = String;
// this is the default player id, used when a player has not been assigned an id yet
pub const DEFAULT_PLAYER_ID: PlayerId = String::new();

#[derive(Archive, Deserialize, Serialize, Clone, Debug, PartialEq)]
#[rkyv(
    // This will generate a PartialEq impl between our unarchived
    // and archived types
    compare(PartialEq),
    // Derives can be passed through to the generated type:
    derive(Debug),
)]
pub enum ReliableServerMessage {
    Hello { player_id: PlayerId },
    PlayersJoined { player_ids: Vec<PlayerId> },
    PlayersLeft { player_ids: Vec<PlayerId> },
    Quit,
}

#[derive(Archive, Deserialize, Serialize, Clone, Debug, PartialEq)]
#[rkyv(
    // This will generate a PartialEq impl between our unarchived
    // and archived types
    compare(PartialEq),
    // Derives can be passed through to the generated type:
    derive(Debug),
)]
pub enum UnreliableServerMessage {
    PlayerPosition(PlayerId, PlayerPosition),
}

#[derive(Archive, Deserialize, Serialize, Clone, Debug, PartialEq)]
#[rkyv(
    // This will generate a PartialEq impl between our unarchived
    // and archived types
    compare(PartialEq),
    // Derives can be passed through to the generated type:
    derive(Debug),
)]
pub enum ReliableClientMessage {
    PlayerJoined { player_id: PlayerId },
    Quit { player_id: PlayerId },
}

#[derive(Archive, Deserialize, Serialize, Clone, Debug, PartialEq)]
#[rkyv(
    // This will generate a PartialEq impl between our unarchived
    // and archived types
    compare(PartialEq),
    // Derives can be passed through to the generated type:
    derive(Debug),
)]
pub enum UnreliableClientMessage {
    PlayerPosition(PlayerPosition),
}

pub const MAX_PACKET_SIZE: usize = 1024;

#[derive(Archive, Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
#[rkyv(
    // This will generate a PartialEq impl between our unarchived
    // and archived types
    compare(PartialEq),
    // Derives can be passed through to the generated type:
    derive(Debug),
)]
pub struct PlayerPosition {
    pub x: f32,
    pub y: f32,
}

pub const DELIMITER: [u8; 1] = *b"D";
pub type MessageSize = [u8; 4]; // convert a u32 (the size of the message) to bytes

// Be careful with this. Too many concurrent streams and the client will freeze
pub const UNIDIRECTIONAL_STREAM_LIMIT: VarInt = VarInt::from_u32(128);

async fn log(log_sender: &LogSender, msg: String) {
    let _ = log_sender.try_send(msg);
}

pub type LogSender = async_channel::Sender<String>;
pub type LogReceiver = async_channel::Receiver<String>;

// An example ALPN that we are using to communicate over the `Endpoint`
const EXAMPLE_ALPN: &[u8] = b"n0/iroh/examples/0";
