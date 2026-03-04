use std::collections::HashMap;

use game_core::{
    ClientMessage, DEFAULT_PLAYER_ID, PlayerId, PlayerPosition, ServerMessage,
    client::{Client, run_client},
    server::{self, Server},
};
use godot::{classes::ISprite2D, prelude::*};
use hecs::{Entity, World};

use crate::{async_runtime::AsyncRuntime, player::Player};

pub enum NetworkState {
    ClientConnection(Client, Gd<Player>),
    ServerConnection(Server),
    HostConnection(Client, Gd<Player>, Server),
    None,
}

impl Default for NetworkState {
    fn default() -> Self {
        NetworkState::None
    }
}


#[derive(GodotClass)]
#[class(init, singleton, base = Object)]
pub struct GameState {
    base: Base<Object>,
    pub network_state: NetworkState,
    world: World,
    remote_player_map: HashMap<PlayerId, Gd<Player>>,
    player_template: Option<Gd<PackedScene>>,
}

#[godot_api]
impl GameState {
    #[signal]
    pub fn player_joined(remote_player: Gd<Player>);

    pub fn start_server(&mut self) {
        self.network_state = NetworkState::ServerConnection(Server::new());
    }

    pub fn start_client(&mut self, player_template: Gd<PackedScene>) -> Option<Gd<Player>> {
        self.player_template = Some(player_template.clone());
        if let Ok((cancel_sender, server_receiver, client_sender, join_set)) =
            AsyncRuntime::block_on(run_client())
        {
            godot_print!("client running");
            let player_ref = player_template.instantiate_as::<Player>();
            self.network_state = NetworkState::ClientConnection(
                Client {
                    cancel_sender,
                    server_receiver,
                    client_sender,
                    join_set,
                    local_player_id: game_core::DEFAULT_PLAYER_ID,
                },
                player_ref.clone(),
            );
            Some(player_ref)
        } else {
            godot_print!("failed to run client");
            None
        }
    }

    pub fn poll_client(&mut self) {
        if let NetworkState::ClientConnection(client, player_ref) = &mut self.network_state {
            // This is where you can handle any client-related logic
            // For example, you might want to check for incoming messages from the server
            let mut players_to_signal: Vec<Gd<Player>> = Vec::new();
            let server_receiver = client.server_receiver.clone();
            while let Ok(message) = server_receiver.try_recv() {
                //godot_print!("Received message from server: {:?}", message);
                match message {
                    ServerMessage::Hello { player_id } => {
                        client.local_player_id = player_id;
                        self.world.spawn((
                            client.local_player_id.clone(),
                            PlayerPosition { x: 0.0, y: 0.0 },
                        ));
                        let mut player_ref = player_ref.bind_mut();
                        player_ref.set_player_id(client.local_player_id.clone());
                        player_ref.is_local = true;
                        // TODO: figure out why this never gets called
                        godot_print!("[client] local ID: {}", client.local_player_id);
                        self.remote_player_map
                            .insert(client.local_player_id.clone(), player_ref.to_gd());
                    }
                    ServerMessage::PlayerPosition(remote_player_id, player_data) => {
                        // for now, ignore if updating local player
                        if remote_player_id == client.local_player_id {
                            continue;
                        }
                        let query = self.world.query_mut::<(&PlayerId, &mut PlayerPosition)>();
                        for (id, position) in query {
                            if *id == remote_player_id {
                                *position = player_data;
                                /*
                                godot_print!(
                                    "Updated position for player {}: ({}, {})",
                                    remote_player_id,
                                    position.x,
                                    position.y
                                );
                                */
                            }
                        }
                        if let Some(remote_player) =
                            self.remote_player_map.get_mut(&remote_player_id)
                        {
                            let mut remote_player_bind = remote_player.bind_mut();
                            // Set position on the underlying Godot node
                            remote_player_bind
                                .base_mut()
                                .set_global_position(Vector2::new(player_data.x, player_data.y));
                        }
                    }
                    ServerMessage::PlayerJoined { player_ids } => {
                        let local_player_id = client.local_player_id.clone();
                        for remote_player_id in player_ids {
                            if remote_player_id == local_player_id
                                || self.remote_player_map.contains_key(&remote_player_id)
                            {
                                continue; // Skip if it's the local player or if its already in the map
                            }
                            godot_print!("[client] Player joined with ID: {}", remote_player_id);
                            self.world.spawn((
                                remote_player_id.clone(),
                                PlayerPosition { x: 0.0, y: 0.0 },
                            ));
                            let remote_player_scene = self
                                .player_template
                                .as_ref()
                                .expect("Player template should be initialized")
                                .clone();
                            let mut remote_player = remote_player_scene.instantiate_as::<Player>();
                            self.remote_player_map
                                .insert(remote_player_id.clone(), remote_player.clone());
                            {
                                let mut remote_player_bind = remote_player.bind_mut();
                                remote_player_bind.set_player_id(remote_player_id);
                                remote_player_bind.is_local = false;
                            }
                            players_to_signal.push(remote_player);
                        }
                        let query = self.world.query_mut::<(&PlayerId, &mut PlayerPosition)>();
                        let query_vec = query.into_iter().collect::<Vec<_>>();
                        godot_print!("[client] Current players: {:?}", query_vec);
                    }
                    ServerMessage::PlayerLeft { player_ids } => {
                        for player_id in player_ids {
                            if player_id == client.local_player_id {
                                continue; // Skip if it's the local player
                            }

                            godot_print!("[client] Player left with ID: {}", player_id);

                            // Remove from Godot scene tree
                            if let Some(mut remote_player) =
                                self.remote_player_map.remove(&player_id)
                            {
                                remote_player.queue_free();
                            }

                            // Remove player from the ECS world
                            let query = self.world.query_mut::<(Entity, &PlayerId)>();
                            let mut entities_to_despawn = Vec::new();
                            for (entity, id) in query {
                                if *id == player_id {
                                    entities_to_despawn.push(entity);
                                }
                            }
                            for entity in entities_to_despawn {
                                self.world.despawn(entity).unwrap();
                            }
                        }
                    }
                    ServerMessage::Quit => {
                        godot_print!("[client] Server requested to quit");
                    }
                }
            }

            // Send local player's position to the server
            {
                let position = player_ref.bind().base().get_global_position();

                // Update local player position in the ECS world
                let query = self.world.query_mut::<(&PlayerId, &mut PlayerPosition)>();
                for (id, world_position) in query {
                    if *id == client.local_player_id {
                        world_position.x = position.x;
                        world_position.y = position.y;
                    }
                }

                // Send position to the server
                let message = ClientMessage::PlayerPosition(PlayerPosition {
                    x: position.x,
                    y: position.y,
                });
                if let Err(e) = client.client_sender.try_send(message) {
                    godot_print!("Failed to send player position: {:?}", e);
                }
            }
            for player in &players_to_signal {
                self.signals().player_joined().emit(player);
            }
        }
    }

    pub fn close_client(&mut self) {
        if let NetworkState::ClientConnection(client, _) = &mut self.network_state {
            // Cancel the client if it is running
            let _ = client.cancel_sender.send(true);
            // Optionally, you can also wait for the client's tasks to finish
            AsyncRuntime::block_on(client.join_set.shutdown());
            self.network_state = NetworkState::None;
        }
    }

    #[func]
    pub fn get_local_player_id(&self) -> GString {
        let singleton = GameState::singleton();
        let singleton = singleton.bind();
        if let NetworkState::ClientConnection(client, _) = &singleton.network_state {
            return GString::from(&client.local_player_id);
        }
        GString::from(&DEFAULT_PLAYER_ID)
    }

    #[func]
    pub fn get_remote_player_amount(&self) -> i32 {
        self.remote_player_map.len() as i32
    }
}
