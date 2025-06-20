use game_core::{ClientMessage, PlayerId, PlayerPosition, ServerMessage, server::run_server};
use godot::{
    classes::{Button, IButton},
    prelude::*,
};
use hecs::World;

use crate::async_runtime::AsyncRuntime;

#[derive(GodotClass)]
#[class(base=Button)]
struct ServerButton {
    channel_map: Option<game_core::server::ChannelMap>,
    world: World,
    base: Base<Button>,
}

#[godot_api]
impl IButton for ServerButton {
    fn init(base: Base<Button>) -> Self {
        Self {
            channel_map: None,   // Initialize with None, will be set when the server starts
            world: World::new(), // Initialize a new Hecs World
            base,
        }
    }

    fn ready(&mut self) {
        godot_print!("Server button is ready!");
    }

    fn pressed(&mut self) {
        godot_print!("Server button pressed!");
        // we are going to shove this in here for now for testing purposes
        if let Ok(channel_map) = AsyncRuntime::block_on(run_server()) {
            godot_print!("server running");
            self.channel_map = Some(channel_map);
        } else {
            godot_print!("failed to run server");
        }
    }

    fn process(&mut self, delta: f64) {
        // This is where you can handle any server-related logic
        // For example, you might want to check for incoming connections or messages
        if let Some(channel_map) = &self.channel_map {
            // Handle server logic with the channel_map
            let mut channel_map = channel_map.lock().unwrap();
            let mut new_player_vec = Vec::<PlayerId>::new();
            let mut leaving_player_vec = Vec::<PlayerId>::new();
            for (player_id, channel) in channel_map.iter() {
                match channel.receiver.try_recv() {
                    Ok(message) => {
                        //godot_print!("Received message from player {}: {:?}", player_id, message);
                        // Handle the received message
                        match message {
                            ClientMessage::PlayerPosition(player_position) => {
                                // Update player position in the world
                                let query =
                                    self.world.query_mut::<(&PlayerId, &mut PlayerPosition)>();
                                for (_, (id, position)) in query {
                                    if *id == *player_id {
                                        *position = player_position;
                                        /*
                                        godot_print!(
                                            "Player {} position: {:?}",
                                            player_id,
                                            player_position
                                        );
                                        */
                                    }
                                }
                            }
                            ClientMessage::PlayerJoined { player_id } => {
                                godot_print!("Player {} joined", player_id);
                                self.world
                                    .spawn((player_id.clone(), PlayerPosition { x: 0.0, y: 0.0 }));
                                new_player_vec.push(player_id.clone());
                                // send list of players to player who just joined
                                let player_ids: Vec<PlayerId> =
                                    channel_map.keys().cloned().collect();
                                if let Some(entry) = channel_map.get(&player_id) {
                                    entry
                                        .sender
                                        .clone()
                                        .try_send(ServerMessage::PlayerJoined { player_ids })
                                        .unwrap();
                                }
                            }
                            ClientMessage::Quit { player_id } => {
                                godot_print!("Player {} left", player_id);
                                leaving_player_vec.push(player_id.clone());
                                // remove entities associated with this player
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
                    }
                    Err(async_channel::TryRecvError::Empty) => {
                        // No messages available, continue processing
                    }
                    Err(async_channel::TryRecvError::Closed) => {
                        godot_print!("Channel for player {} closed", player_id);
                        // Handle the closed channel if necessary
                    }
                }
            }

            // Send messages to clients
            let game_data = self
                .world
                .query::<(&PlayerId, &PlayerPosition)>()
                .iter()
                .map(|(entity, (id, position))| ServerMessage::PlayerPosition(id.clone(), *position))
                .collect::<Vec<ServerMessage>>();

            for (player_id, message_channels) in channel_map.iter() {
                // Get player position in the world
                let server_sender = &message_channels.sender;
                // send game data to each player
                for game_data_message in &game_data {
                    // Send player position to the client
                    if let Err(e) = server_sender.try_send(game_data_message.clone()) {
                        godot_print!("Failed to send message to player {}: {}", player_id, e);
                    }
                }
                // Send new player messages
                if !new_player_vec.is_empty() {
                    // send new player message to all players
                    let new_player_message = ServerMessage::PlayerJoined {
                        player_ids: new_player_vec.clone(),
                    };
                    if let Err(e) = server_sender.try_send(new_player_message) {
                        godot_print!(
                            "Failed to send new player message to player {}: {}",
                            player_id, e
                        );
                    }
                }
                // Send leaving player messages
                if !leaving_player_vec.is_empty() {
                    let leaving_player_message = ServerMessage::PlayerLeft {
                        player_ids: new_player_vec.clone(),
                    };
                    if let Err(e) = server_sender.try_send(leaving_player_message) {
                        godot_print!(
                            "Failed to send new player message to player {}: {}",
                            player_id, e
                        );
                    }
                }
            }

            // remove channels from players that have left
            for player_id in &leaving_player_vec {
                channel_map.remove(player_id);
            }
        }
    }
}
