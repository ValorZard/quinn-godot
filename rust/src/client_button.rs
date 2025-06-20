use std::{collections::HashMap, hash::Hash};

use game_core::{client::run_client, ClientMessage, PlayerId, PlayerPosition, ServerMessage, DEFAULT_PLAYER_ID};
use godot::{
    classes::{Button, IButton, Sprite2D},
    prelude::*,
};
use hecs::World;

use crate::{
    async_runtime::AsyncRuntime,
    player::{self, Player},
};

#[derive(GodotClass)]
#[class(base=Button)]
struct ClientButton {
    server_receiver: Option<async_channel::Receiver<ServerMessage>>,
    client_sender: Option<async_channel::Sender<ClientMessage>>,
    world: World,
    local_player_id: PlayerId,
    #[export]
    player_ref: Option<Gd<Player>>,
    #[export]
    remote_player_ref: Option<Gd<PackedScene>>,
    remote_player_map: HashMap<PlayerId, Gd<Player>>,
    #[export]
    remote_player_amount: i32,
    base: Base<Button>,
}

#[godot_api]
impl IButton for ClientButton {
    fn init(base: Base<Button>) -> Self {
        Self {
            server_receiver: None, // Initialize with None, will be set when the client starts
            client_sender: None,   // Initialize with None, will be set when the client starts
            player_ref: None,      // Reference to the player, if needed
            remote_player_ref: None, // Reference to the remote player, if needed
            world: World::new(),   // Initialize a new Hecs World
            local_player_id: DEFAULT_PLAYER_ID,    // Local player ID, initialized to nothing
            remote_player_map: HashMap::new(), // Map to keep track of remote players
            remote_player_amount: 0, // Initialize remote player amount to 0
            base,
        }
    }

    fn ready(&mut self) {
        godot_print!("Client button is ready!");
    }

    fn pressed(&mut self) {
        godot_print!("Client button pressed!");
        if let Ok((server_receiver, client_sender)) = AsyncRuntime::block_on(run_client()) {
            godot_print!("client running");
            self.server_receiver = Some(server_receiver);
            self.client_sender = Some(client_sender);
        } else {
            godot_print!("failed to run client");
        }
    }

    fn process(&mut self, delta: f64) {
        // This is where you can handle any client-related logic
        // For example, you might want to check for incoming messages from the server
        if let Some(receiver) = &self.server_receiver {
            while let Ok(message) = receiver.try_recv() {
                match message {
                    ServerMessage::Hello { player_id } => {
                        self.local_player_id = player_id;
                        self.world
                            .spawn((self.local_player_id.clone(), PlayerPosition { x: 0.0, y: 0.0 }));
                        if let Some(player_ref) = self.player_ref.as_mut() {
                            let mut player_ref = player_ref.bind_mut();
                            player_ref.set_player_id(self.local_player_id.clone());
                            player_ref.set_is_local(true);
                        }
                        // TODO: figure out why this never gets called
                        godot_print!("[client] local ID: {}", self.local_player_id);
                    }
                    ServerMessage::PlayerPosition(remote_player_id, player_data) => {
                        let query = self.world.query_mut::<(&PlayerId, &mut PlayerPosition)>();
                        for (_, (id, position)) in query {
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
                        for remote_player_id in player_ids {
                            if remote_player_id == self.local_player_id
                                || self.remote_player_map.contains_key(&remote_player_id)
                            {
                                continue; // Skip if it's the local player or if its already in the map
                            }
                            godot_print!("[client] Player joined with ID: {}", remote_player_id);
                            self.world
                                .spawn((remote_player_id.clone(), PlayerPosition { x: 0.0, y: 0.0 }));
                            if let Some(remote_player_ref) = &self.remote_player_ref {
                                let remote_player_scene = remote_player_ref.clone();
                                let mut remote_player =
                                    remote_player_scene.instantiate_as::<Player>();
                                self.remote_player_map
                                    .insert(remote_player_id.clone(), remote_player.clone());
                                self.to_gd().add_child(&remote_player);
                                let mut remote_player_bind = remote_player.bind_mut();
                                remote_player_bind.set_player_id(remote_player_id);
                                remote_player_bind.set_is_local(false);
                            }
                        }
                        let query = self.world.query_mut::<(&PlayerId, &mut PlayerPosition)>();
                        let query_vec = query.into_iter().collect::<Vec<_>>();
                        godot_print!("[client] Current players: {:?}", query_vec);
                    }
                    ServerMessage::PlayerLeft { player_ids } => {
                        for player_id in player_ids {
                            if player_id == self.local_player_id {
                                continue; // Skip if it's the local player
                            }

                            godot_print!("[client] Player left with ID: {}", player_id);
                            // Remove player from the world
                            let query = self.world.query_mut::<&PlayerId>();
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
        }

        if let Some(sender) = &self.client_sender {
            // Here you can send messages to the server if needed
            // For example, you might want to send a heartbeat or a status update
            if let Some(player_ref) = &self.player_ref {
                let player = player_ref.bind();
                let position = player.base().get_global_position();

                // do local player logic
                // local player logic
                let query = self.world.query_mut::<(&PlayerId, &mut PlayerPosition)>();
                for (_, (id, world_position)) in query {
                    if *id == self.local_player_id {
                        world_position.x = position.x;
                        world_position.y = position.y;
                    }
                }

                // send player position to the server
                let message = ClientMessage::PlayerPosition(PlayerPosition {
                    x: position.x,
                    y: position.y,
                });
                if let Err(e) = sender.try_send(message) {
                    godot_print!("Failed to send player position: {:?}", e);
                }
            }
        }

        self.remote_player_amount = self.remote_player_map.len() as i32;
    }
}

#[godot_api]
impl ClientButton {
    #[func]
    fn get_local_player_id(&self) -> GString {
        self.local_player_id.to_string().into()
    }
}