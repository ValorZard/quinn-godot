use rkyv::{Archive, Deserialize, Serialize};

pub mod client;
pub mod server;

pub type PlayerId = u64;

#[derive(Archive, Deserialize, Serialize, Clone, Debug, PartialEq)]
#[rkyv(
    // This will generate a PartialEq impl between our unarchived
    // and archived types
    compare(PartialEq),
    // Derives can be passed through to the generated type:
    derive(Debug),
)]
pub enum ServerMessage {
    Hello { player_id: u64 },
    PlayerPosition(PlayerId, PlayerPosition),
    PlayerJoined { remote_player_ids: Vec<PlayerId> },
    PlayerLeft { player_id: PlayerId },
    Quit,
}

#[derive(Archive, Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
#[rkyv(
    // This will generate a PartialEq impl between our unarchived
    // and archived types
    compare(PartialEq),
    // Derives can be passed through to the generated type:
    derive(Debug),
)]
pub enum ClientMessage {
    PlayerJoined { player_id: PlayerId },
    PlayerPosition(PlayerPosition),
    Quit { player_id: PlayerId },
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
