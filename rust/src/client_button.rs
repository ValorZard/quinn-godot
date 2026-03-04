use std::collections::HashMap;

use game_core::{
    ClientMessage, DEFAULT_PLAYER_ID, PlayerId, PlayerPosition, ServerMessage, client::run_client,
};
use godot::{
    classes::{Button, IButton},
    prelude::*,
};
use hecs::{Entity, World};
use tokio::sync::watch;

use crate::{
    async_runtime::AsyncRuntime,
    game_state::GameState,
    player::{self, Player},
};

#[derive(GodotClass)]
#[class(base=Button)]
struct ClientButton {
    #[export]
    player_ref: Option<Gd<Player>>,
    #[export]
    remote_player_ref: Option<Gd<PackedScene>>,
    #[export]
    remote_player_amount: i32,
    base: Base<Button>,
}

#[godot_api]
impl IButton for ClientButton {
    fn init(base: Base<Button>) -> Self {
        Self {
            player_ref: None,        // Reference to the player, if needed
            remote_player_ref: None, // Reference to the remote player, if needed
            remote_player_amount: 0, // Initialize remote player amount to 0
            base,
        }
    }

    fn ready(&mut self) {
        godot_print!("Client button is ready!");
    }

    fn pressed(&mut self) {
        godot_print!("Client button pressed!");
        let mut singleton = GameState::singleton();
        let player_template = self
            .remote_player_ref
            .as_ref()
            .expect("This should be initalized by now");
        let client_player = singleton.bind_mut().start_client(player_template.clone());
        if let Some(player) = client_player {
            self.player_ref = Some(player.clone());
            let self_gd = self.to_gd();
            self_gd
                .get_parent()
                .expect("Should have parent")
                .add_child(&player);
        }
    }

    fn process(&mut self, _delta: f64) {
        let mut singleton = GameState::singleton();
        let mut singleton = singleton.bind_mut();
        singleton.poll_client();
        self.remote_player_amount = singleton.get_remote_player_amount();
    }

    fn exit_tree(&mut self) {
        godot_print!("Client button is exiting tree!");
        let mut singleton = GameState::singleton();
        let mut singleton = singleton.bind_mut();
        singleton.close_client();
    }
}

#[godot_api]
impl ClientButton {
    #[func]
    fn get_local_player_id(&self) -> GString {
        let singleton = GameState::singleton();
        let singleton = singleton.bind();
        singleton.get_local_player_id()
    }
}
