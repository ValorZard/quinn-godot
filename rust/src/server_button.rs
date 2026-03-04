use game_core::{ClientMessage, PlayerId, PlayerPosition, ServerMessage, server::run_server};
use godot::{
    classes::{Button, IButton},
    prelude::*,
};
use hecs::{Entity, World};
use tokio::task::JoinSet;

use crate::{async_runtime::AsyncRuntime, game_state::GameState};

#[derive(GodotClass)]
#[class(base=Button)]
struct ServerButton {
    channel_map: Option<game_core::server::ChannelMap>,
    join_set: JoinSet<JoinSet<()>>,
    world: World,
    base: Base<Button>,
}

#[godot_api]
impl IButton for ServerButton {
    fn init(base: Base<Button>) -> Self {
        Self {
            channel_map: None,   // Initialize with None, will be set when the server starts
            world: World::new(), // Initialize a new Hecs World
            join_set: JoinSet::new(), // Initialize an empty JoinSet
            base,
        }
    }

    fn ready(&mut self) {
        godot_print!("Server button is ready!");
    }

    fn pressed(&mut self) {
        godot_print!("Server button pressed!");
        // we are going to shove this in here for now for testing purposes
        let mut singleton = GameState::singleton();
        let mut singleton = singleton.bind_mut();
        singleton.start_server();
    }

    fn process(&mut self, _delta: f64) {
        // This is where you can handle any server-related logic
        // For example, you might want to check for incoming connections or messages
        let mut singleton = GameState::singleton();
        let mut singleton = singleton.bind_mut();
        singleton.poll_server();
    }

    fn exit_tree(&mut self) {
        godot_print!("Server button is exiting the scene tree!");
        // Clean up resources if necessary
        let mut singleton = GameState::singleton();
        let mut singleton = singleton.bind_mut();
        singleton.close_server();
    }
}
