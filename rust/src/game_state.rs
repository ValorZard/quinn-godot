use std::collections::{HashMap, HashSet};

use game_core::{
    DEFAULT_PLAYER_ID, PlayerId, PlayerPosition, ReliableClientMessage, ReliableServerMessage,
    UnreliableClientMessage, UnreliableServerMessage,
    client::{Client, run_client},
    server::{self, Server, run_server},
};
use game_logic::game_state::{DEFAULT_POSITION, GameState as GameStateInner, InputData};
use godot::{classes::ISprite2D, meta::ByValue, prelude::*};
use hecs::{Entity, World};
use tokio::time::error::Error;

use crate::{
    async_runtime::AsyncRuntime,
    player::{self, Player},
};

#[derive(Debug, Clone)]
pub struct GodotInputData {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
}

impl GodotConvert for GodotInputData {
    type Via = Dictionary<StringName, bool>;
    fn godot_shape() -> godot::meta::GodotShape {
        godot::meta::GodotShape::TypedDictionary {
            key: godot::register::property::GodotElementShape::Builtin {
                variant_type: VariantType::STRING_NAME,
            },
            value: godot::register::property::GodotElementShape::Builtin {
                variant_type: VariantType::BOOL,
            },
        }
    }
}

impl FromGodot for GodotInputData {
    fn try_from_godot(input: Self::Via) -> Result<Self, ConvertError> {
        Ok(GodotInputData {
            up: input.get("up").unwrap_or(false),
            down: input.get("down").unwrap_or(false),
            left: input.get("left").unwrap_or(false),
            right: input.get("right").unwrap_or(false),
        })
    }
}

impl ToGodot for GodotInputData {
    type Pass = ByValue;
    fn to_godot(&self) -> Self::Via {
        let mut dict = Dictionary::new();
        dict.insert("up", self.up);
        dict.insert("down", self.down);
        dict.insert("left", self.left);
        dict.insert("right", self.right);
        dict
    }
}

#[derive(GodotClass)]
#[class(init, singleton, base = Object)]
pub struct GameState {
    base: Base<Object>,
    inner: GameStateInner,
    player_template: Option<Gd<PackedScene>>,
    player_id_to_godot_map: HashMap<PlayerId, Gd<Player>>,
}

#[godot_api]
impl GameState {
    #[signal]
    pub fn player_joined(remote_player: Gd<Player>);

    #[func]
    pub fn start_server(
        &mut self,
        player_template: Gd<PackedScene>,
        is_host: bool,
    ) -> Option<Gd<Player>> {
        let entity = AsyncRuntime::block_on(self.inner.start_server(is_host));
        self.player_template = Some(player_template);
        if let Some(player_entity) = entity {
            let player_node = self.spawn_local_player(self.inner.get_local_network_id().unwrap());
            Some(player_node)
        } else {
            None
        }
    }

    #[func]
    pub fn start_client(
        &mut self,
        server_iroh_string: GString,
        player_template: Gd<PackedScene>,
    ) -> Option<Gd<Player>> {
        let entity = AsyncRuntime::block_on(self.inner.start_client(server_iroh_string.into()));
        self.player_template = Some(player_template);
        if let Some(player_entity) = entity {
            let player_node = self.spawn_local_player(self.inner.get_local_network_id().unwrap());
            Some(player_node)
        } else {
            None
        }
    }

    fn spawn_local_player(&mut self, player_id: PlayerId) -> Gd<Player> {
        let player_scene = self
            .player_template
            .as_ref()
            .expect("Player template should be initialized")
            .clone();
        let mut player = player_scene.instantiate_as::<Player>();
        {
            let mut player_bind = player.bind_mut();
            player_bind.set_player_id(player_id.clone());
            player_bind.is_local = true;
        }
        self.player_id_to_godot_map
            .insert(player_id.clone(), player.clone());
        player
    }

    fn spawn_remote_player(&mut self, player_id: PlayerId) {
        // The inner poll_client/poll_server already spawned the entity in the ECS world,
        // so just look it up instead of trying to spawn again.
        let Some(entity) = self.inner.get_entity_associated_with_player_id(&player_id) else {
            return;
        };
        if self.player_id_to_godot_map.contains_key(&player_id) {
            // Already have a Godot node for this player (e.g. local player)
            return;
        }
        let player_scene = self
            .player_template
            .as_ref()
            .expect("Player template should be initialized")
            .clone();
        let mut player = player_scene.instantiate_as::<Player>();
        {
            let mut player_bind = player.bind_mut();
            player_bind.set_player_id(player_id.clone());
            player_bind.is_local = false;
        }
        self.player_id_to_godot_map
            .insert(player_id.clone(), player.clone());
        self.signals().player_joined().emit(&player);
    }

    fn remove_player(&mut self, player_id: &PlayerId) {
        // Remove Godot node
        if let Some(mut player_node) = self.player_id_to_godot_map.remove(player_id) {
            player_node.queue_free();
        }
        // Remove from inner ECS
        self.inner.remove_player(player_id);
    }

    #[func]
    pub fn poll(&mut self) {
        if self.inner.network_state.is_none() {
            return;
        }
        // get local player position to send to the server
        let local_player_ref = self.player_id_to_godot_map.get(
            &self
                .inner
                .get_local_network_id()
                .expect("Network state should be set by now"),
        );
        let local_player_position = local_player_ref
            .and_then(|player_node| {
                let position = player_node.get_global_position();
                Some(PlayerPosition {
                    x: position.x,
                    y: position.y,
                })
            })
            .expect("There should be an initialized local player linked to the network");

        self.inner.submit_local_input(local_player_position);

        let poll_result = self.inner.poll();
        for new_player in poll_result.new_players {
            self.spawn_remote_player(new_player);
        }
        for removed_player in poll_result.leaving_players {
            self.remove_player(&removed_player);
        }
        self.sync_player_positions();
        // print messages
        for log_msg in self.inner.drain_log_buffer() {
            godot_print!("{}", log_msg);
        }
    }

    fn sync_player_positions(&mut self) {
        for (player_id, player_node) in self.player_id_to_godot_map.iter_mut() {
            if let Some(player) = self.inner.get_player_component(player_id) {
                let position = Vector2::new(player.position.x, player.position.y);
                player_node.set_global_position(position);
            }
        }
    }

    #[func]
    pub fn close_client(&mut self) {
        AsyncRuntime::block_on(self.inner.close_client());
    }

    #[func]
    pub fn close_server(&mut self) {
        self.inner.close_server();
    }

    #[func]
    pub fn get_remote_player_amount(&self) -> i32 {
        self.inner.get_remote_player_amount()
    }

    #[func]
    pub fn get_local_network_id(&self) -> GString {
        match self.inner.get_local_network_id() {
            Some(id) => (&id).into(),
            None => GString::from("No ID detected"),
        }
    }
}
