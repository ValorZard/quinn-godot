use std::collections::{HashMap, HashSet};

use game_core::{
    DEFAULT_PLAYER_ID, PlayerId, PlayerPosition, ReliableClientMessage, ReliableServerMessage,
    UnreliableClientMessage, UnreliableServerMessage,
    client::{Client, run_client},
    server::{self, Server, run_server},
};
use godot::{classes::ISprite2D, prelude::*};
use hecs::{Entity, World};

use crate::{
    async_runtime::AsyncRuntime,
    player::{self, Player},
};

pub enum NetworkState {
    ClientConnection(Client, Gd<Player>),
    ServerConnection(Server, Option<Gd<Player>>),
}

#[derive(GodotClass)]
#[class(init, singleton, base = Object)]
pub struct GameState {
    base: Base<Object>,
    pub network_state: Option<NetworkState>,
    world: World,
    remote_player_map: HashMap<PlayerId, Gd<Player>>,
    player_template: Option<Gd<PackedScene>>,
}

#[godot_api]
impl GameState {
    #[signal]
    pub fn player_joined(remote_player: Gd<Player>);

    #[func]
    pub fn start_server(&mut self, player_template: Option<Gd<PackedScene>>) -> Option<Gd<Player>> {
        if let Ok(server) = AsyncRuntime::block_on(run_server()) {
            godot_print!("server running");
            let player_ref = if let Some(template) = player_template {
                self.player_template = Some(template.clone());
                let player_ref = self.spawn_local_player(server.get_server_id());
                Some(player_ref)
            } else {
                None
            };
            self.network_state = Some(NetworkState::ServerConnection(server, player_ref.clone()));
            player_ref
        } else {
            godot_print!("failed to run server");
            None
        }
    }

    #[func]
    pub fn start_client(
        &mut self,
        server_iroh_string: GString,
        player_template: Gd<PackedScene>,
    ) -> Option<Gd<Player>> {
        godot_print!("starting client");
        if let Ok(client) = AsyncRuntime::block_on(run_client(server_iroh_string.to_string())) {
            godot_print!("client running");
            self.player_template = Some(player_template.clone());
            let player_id = client.get_local_endpoint_id();
            let player_ref = self.spawn_local_player(player_id);
            self.network_state = Some(NetworkState::ClientConnection(client, player_ref.clone()));
            Some(player_ref)
        } else {
            godot_print!("failed to run client");
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
        self.remote_player_map
            .insert(player_id.clone(), player.clone());
        self.world
            .spawn((player_id, PlayerPosition { x: 0.0, y: 0.0 }));
        player
    }

    fn spawn_remote_player(&mut self, player_id: PlayerId) {
        if self.remote_player_map.contains_key(&player_id) {
            return; // Skip if it's the local player or if its already in the map
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
        self.remote_player_map
            .insert(player_id.clone(), player.clone());
        self.world
            .spawn((player_id, PlayerPosition { x: 0.0, y: 0.0 }));
        self.signals().player_joined().emit(&player);
    }

    fn remove_player(&mut self, player_id: &PlayerId) {
        godot_print!("[client] Player left with ID: {}", player_id);

        // Remove from Godot scene tree
        if let Some(mut remote_player) = self.remote_player_map.remove(player_id.as_str()) {
            remote_player.queue_free();
        }

        // Remove player from the ECS world
        let query = self.world.query_mut::<(Entity, &PlayerId)>();
        let mut entities_to_despawn = Vec::new();
        for (entity, id) in query {
            if *id == *player_id {
                entities_to_despawn.push(entity);
            }
        }
        for entity in entities_to_despawn {
            self.world.despawn(entity).unwrap();
        }
    }

    fn update_player_with_remote_data(
        &mut self,
        player_id: &PlayerId,
        player_position: &PlayerPosition,
    ) {
        let query = self.world.query_mut::<(&PlayerId, &mut PlayerPosition)>();
        for (id, position) in query {
            if *id == *player_id {
                *position = *player_position;
                /*
                godot_print!(
                    "Updated position for player {}: ({}, {})",
                    player_id,
                    position.x,
                    position.y
                );
                */
            }
        }
        if let Some(remote_player) = self.remote_player_map.get_mut(player_id.as_str()) {
            let mut remote_player_bind = remote_player.bind_mut();
            // Set position on the underlying Godot node
            remote_player_bind
                .base_mut()
                .set_global_position(Vector2::new(player_position.x, player_position.y));
        }
    }

    #[func]
    pub fn poll_client(&mut self) {
        let mut network_state = self.network_state.take();
        if let Some(NetworkState::ClientConnection(client, player_ref)) = &mut network_state {
            // Drain log messages from async tasks
            while let Ok(log_msg) = client.log_receiver.try_recv() {
                godot_print!("{}", log_msg);
            }

            // This is where you can handle any client-related logic
            // For example, you might want to check for incoming messages from the server
            let server_reliable_receiver = client.reliable_server_receiver.clone();
            while let Ok(message) = server_reliable_receiver.try_recv() {
                godot_print!("Received message from server: {:?}", message);
                match message {
                    ReliableServerMessage::Hello { player_id } => {}
                    ReliableServerMessage::PlayersJoined { player_ids } => {
                        for remote_player_id in player_ids {
                            self.spawn_remote_player(remote_player_id);
                        }
                        let query = self.world.query_mut::<(&PlayerId, &mut PlayerPosition)>();
                        let query_vec = query.into_iter().collect::<Vec<_>>();
                        godot_print!("[client] Current players: {:?}", query_vec);
                    }
                    ReliableServerMessage::PlayersLeft { player_ids } => {
                        for player_id in player_ids {
                            self.remove_player(&player_id);
                        }
                    }
                    ReliableServerMessage::Quit => {
                        godot_print!("[client] Server requested to quit");
                    }
                }
            }

            let unreliable_server_receiver = client.unreliable_server_receiver.clone();
            while let Ok(message) = unreliable_server_receiver.try_recv() {
                match message {
                    UnreliableServerMessage::PlayerPosition(remote_player_id, player_data) => {
                        // for now, ignore if updating local player
                        if remote_player_id == client.get_local_endpoint_id() {
                            continue;
                        }
                        self.update_player_with_remote_data(&remote_player_id, &player_data);
                    }
                }
            }

            // Send local player's position to the server
            {
                let position = player_ref.bind().base().get_global_position();

                // Update local player position in the ECS world
                let query = self.world.query_mut::<(&PlayerId, &mut PlayerPosition)>();
                for (id, world_position) in query {
                    if *id == client.get_local_endpoint_id() {
                        world_position.x = position.x;
                        world_position.y = position.y;
                    }
                }

                // Send position to the server
                let message = UnreliableClientMessage::PlayerPosition(PlayerPosition {
                    x: position.x,
                    y: position.y,
                });
                if let Err(e) = client.unreliable_client_sender.try_send(message) {
                    godot_print!("Failed to send player position: {:?}", e);
                }
            }
        }

        self.network_state = network_state;
    }

    #[func]
    pub fn poll_server(&mut self) {
        // This is where you can handle any server-related logic
        // For example, you might want to check for incoming connections or messages
        let mut network_state = self.network_state.take();
        if let Some(NetworkState::ServerConnection(server, player_ref)) = &mut network_state {
            // Drain log messages from server async tasks
            while let Ok(log_msg) = server.log_receiver.try_recv() {
                godot_print!("{}", log_msg);
            }
            let player_ref = player_ref.clone();
            // Handle server logic with the channel_map
            let channel_map = server.channel_map.clone();
            let mut new_player_set = HashSet::<PlayerId>::new();
            let mut leaving_player_set = HashSet::<PlayerId>::new();
            for (player_id, channel) in channel_map.iter() {
                match channel.reliable_receiver.try_recv() {
                    Ok(message) => {
                        //godot_print!("Received message from player {}: {:?}", player_id, message);
                        // Handle the received message
                        match message {
                            ReliableClientMessage::PlayerJoined { player_id } => {
                                godot_print!("Player {} joined", player_id);
                                self.spawn_remote_player(player_id.clone());
                                new_player_set.insert(player_id.clone());
                                // send list of players to player who just joined
                                let mut player_ids: Vec<PlayerId> = channel_map.keys();
                                // Include the host player so clients know about it
                                if player_ref.is_some() {
                                    let host_id = server.get_server_id();
                                    if !player_ids.contains(&host_id) {
                                        player_ids.push(host_id);
                                    }
                                }
                                if let Some(entry) = channel_map.get(&player_id) {
                                    entry
                                        .reliable_sender
                                        .clone()
                                        .try_send(ReliableServerMessage::PlayersJoined {
                                            player_ids,
                                        })
                                        .unwrap();
                                }
                            }
                            ReliableClientMessage::Quit { player_id } => {
                                leaving_player_set.insert(player_id.clone());
                            }
                        }
                    }
                    Err(async_channel::TryRecvError::Empty) => {
                        // No messages available, continue processing
                    }
                    Err(async_channel::TryRecvError::Closed) => {
                        godot_print!("Channel for player {} closed", player_id);
                        leaving_player_set.insert(player_id.clone());
                    }
                }

                match channel.unreliable_receiver.try_recv() {
                    Ok(message) => {
                        // Handle the received message
                        match message {
                            UnreliableClientMessage::PlayerPosition(player_position) => {
                                self.update_player_with_remote_data(&player_id, &player_position);
                            }
                        }
                    }
                    Err(async_channel::TryRecvError::Empty) => {
                        // No messages available, continue processing
                    }
                    Err(async_channel::TryRecvError::Closed) => {
                        godot_print!("Unreliable channel for player {} closed", player_id);
                        leaving_player_set.insert(player_id.clone());
                    }
                }
            }

            // Send messages to clients
            let game_data = self
                .world
                .query::<(&PlayerId, &PlayerPosition)>()
                .iter()
                .map(|(id, position)| {
                    UnreliableServerMessage::PlayerPosition(id.clone(), *position)
                })
                .collect::<Vec<UnreliableServerMessage>>();

            // send game data to all players
            for (player_id, message_channels) in channel_map.iter() {
                // Get player position in the world
                let unreliable_server_sender = &message_channels.unreliable_sender;
                // send game data to each player
                for game_data_message in &game_data {
                    // Send player position to the client
                    if let Err(e) = unreliable_server_sender.try_send(game_data_message.clone()) {
                        godot_print!("Failed to send message to player {}: {}", player_id, e);
                    }
                }
            }

            // Send new player messages
            if !new_player_set.is_empty() {
                // send new player message to all players
                let new_player_message = ReliableServerMessage::PlayersJoined {
                    player_ids: new_player_set.iter().cloned().collect::<Vec<PlayerId>>(),
                };
                for (player_id, message_channels) in channel_map.iter() {
                    let reliable_server_sender = &message_channels.reliable_sender;
                    if let Err(e) = reliable_server_sender.try_send(new_player_message.clone()) {
                        godot_print!(
                            "Failed to send new player message to player {}: {}",
                            player_id,
                            e
                        );
                    }
                }
            }

            // Handle leaving players
            if !leaving_player_set.is_empty() {
                for player_id in &leaving_player_set {
                    self.remove_player(player_id);
                }
                let leaving_player_message = ReliableServerMessage::PlayersLeft {
                    player_ids: leaving_player_set
                        .iter()
                        .cloned()
                        .collect::<Vec<PlayerId>>(),
                };
                // shut down channels for leaving players and remove from channel map
                for player_id in leaving_player_set {
                    if let Some(message_channels) = channel_map.get(&player_id) {
                        message_channels.cancel_sender.send(true).unwrap();
                    }
                    server.channel_map.remove(&player_id);
                }
                // tell remaining players about leaving players
                for (player_id, message_channels) in channel_map.iter() {
                    let reliable_server_sender = &message_channels.reliable_sender;
                    if let Err(e) = reliable_server_sender.try_send(leaving_player_message.clone())
                    {
                        godot_print!(
                            "Failed to send leaving player message to player {}: {}",
                            player_id,
                            e
                        );
                    }
                }
            }

            // Handle player reference for host connection
            if let Some(player_ref) = player_ref {
                // Update host player's position in the ECS world so it gets broadcast to clients
                let position = player_ref.bind().base().get_global_position();
                let host_id = server.get_server_id();
                let query = self.world.query_mut::<(&PlayerId, &mut PlayerPosition)>();
                for (id, world_position) in query {
                    if *id == host_id {
                        world_position.x = position.x;
                        world_position.y = position.y;
                    }
                }
            }
        }
        self.network_state = network_state;
    }

    #[func]
    pub fn close_client(&mut self) {
        let mut network_state = self.network_state.take();
        if let Some(NetworkState::ClientConnection(client, _)) = &mut network_state {
            // Cancel the client if it is running
            let _ = client.cancel_sender.send(true);
            // Optionally, you can also wait for the client's tasks to finish
            AsyncRuntime::block_on(client.join_set.shutdown());
        }
    }

    #[func]
    pub fn close_server(&mut self) {
        let mut network_state = self.network_state.take();
        if let Some(NetworkState::ServerConnection(server, player_ref)) = &mut network_state {
            // Clean up resources if necessary
            if let Some(player_ref) = player_ref {
                player_ref.queue_free();
            }
            for (_player_id, message_channels) in server.channel_map.iter() {
                // shut down the tasks for each player
                message_channels.cancel_sender.send(true).unwrap();
            }
            server.channel_map.clear(); // Clear the channel map on exit

            // clean up the join set
            AsyncRuntime::block_on(server.join_set.shutdown());
        }
    }

    #[func]
    pub fn get_remote_player_amount(&self) -> i32 {
        self.remote_player_map.len() as i32
    }

    #[func]
    pub fn get_local_network_id(&self) -> GString {
        let singleton = GameState::singleton();
        let singleton = singleton.bind();
        if let Some(NetworkState::ClientConnection(client, _)) = &singleton.network_state {
            return GString::from(&client.get_local_endpoint_id());
        } else if let Some(NetworkState::ServerConnection(server, _)) = &singleton.network_state {
            return GString::from(&server.get_server_id());
        }
        GString::from(&DEFAULT_PLAYER_ID)
    }
}
