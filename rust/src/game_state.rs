use std::collections::HashMap;

use game_core::{
    DEFAULT_PLAYER_ID, PlayerId, PlayerPosition, ReliableClientMessage, ReliableServerMessage,
    UnreliableClientMessage, UnreliableServerMessage,
    client::{Client, run_client},
    server::{self, Server, run_server},
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

    #[func]
    pub fn start_server(&mut self) {
        if let Ok(server) = AsyncRuntime::block_on(run_server()) {
            godot_print!("server running");
            self.network_state = NetworkState::ServerConnection(server);
        } else {
            godot_print!("failed to run server");
        }
    }

    #[func]
    pub fn start_client(&mut self, player_template: Gd<PackedScene>) -> Option<Gd<Player>> {
        godot_print!("starting client");
        if let Ok(client) = AsyncRuntime::block_on(run_client()) {
            godot_print!("client running");
            self.player_template = Some(player_template.clone());
            let player_ref = player_template.instantiate_as::<Player>();
            self.network_state = NetworkState::ClientConnection(client, player_ref.clone());
            Some(player_ref)
        } else {
            godot_print!("failed to run client");
            None
        }
    }

    #[func]
    pub fn poll_client(&mut self) {
        if let NetworkState::ClientConnection(client, player_ref) = &mut self.network_state {
            // Drain log messages from async tasks
            while let Ok(log_msg) = client.log_receiver.try_recv() {
                godot_print!("{}", log_msg);
            }

            // This is where you can handle any client-related logic
            // For example, you might want to check for incoming messages from the server
            let mut players_to_signal: Vec<Gd<Player>> = Vec::new();
            let server_reliable_receiver = client.reliable_server_receiver.clone();
            while let Ok(message) = server_reliable_receiver.try_recv() {
                godot_print!("Received message from server: {:?}", message);
                match message {
                    ReliableServerMessage::Hello { player_id } => {
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
                    ReliableServerMessage::PlayerJoined { player_ids } => {
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
                    ReliableServerMessage::PlayerLeft { player_ids } => {
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
                let message = UnreliableClientMessage::PlayerPosition(PlayerPosition {
                    x: position.x,
                    y: position.y,
                });
                if let Err(e) = client.unreliable_client_sender.try_send(message) {
                    godot_print!("Failed to send player position: {:?}", e);
                }
            }
            for player in &players_to_signal {
                self.signals().player_joined().emit(player);
            }
        }
    }

    #[func]
    pub fn poll_server(&mut self) {
        // This is where you can handle any server-related logic
        // For example, you might want to check for incoming connections or messages
        if let NetworkState::ServerConnection(server) = &mut self.network_state {
            // Drain log messages from server async tasks
            while let Ok(log_msg) = server.log_receiver.try_recv() {
                godot_print!("{}", log_msg);
            }
            // Handle server logic with the channel_map
            let mut channel_map = server.channel_map.lock().unwrap();
            let mut new_player_vec = Vec::<PlayerId>::new();
            let mut leaving_player_vec = Vec::<PlayerId>::new();
            for (player_id, channel) in channel_map.iter() {
                match channel.reliable_receiver.try_recv() {
                    Ok(message) => {
                        //godot_print!("Received message from player {}: {:?}", player_id, message);
                        // Handle the received message
                        match message {
                            ReliableClientMessage::PlayerJoined { player_id } => {
                                godot_print!("Player {} joined", player_id);
                                self.world
                                    .spawn((player_id.clone(), PlayerPosition { x: 0.0, y: 0.0 }));
                                new_player_vec.push(player_id.clone());
                                // send list of players to player who just joined
                                let player_ids: Vec<PlayerId> =
                                    channel_map.keys().cloned().collect();
                                if let Some(entry) = channel_map.get(&player_id) {
                                    entry
                                        .reliable_sender
                                        .clone()
                                        .try_send(ReliableServerMessage::PlayerJoined {
                                            player_ids,
                                        })
                                        .unwrap();
                                }
                            }
                            ReliableClientMessage::Quit { player_id } => {
                                godot_print!("Player {} left", player_id);
                                leaving_player_vec.push(player_id.clone());
                                // remove entities associated with this player
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
                    }
                    Err(async_channel::TryRecvError::Empty) => {
                        // No messages available, continue processing
                    }
                    Err(async_channel::TryRecvError::Closed) => {
                        godot_print!("Channel for player {} closed", player_id);
                        if !leaving_player_vec.contains(player_id) {
                            leaving_player_vec.push(player_id.clone());
                        }
                    }
                }

                match channel.unreliable_receiver.try_recv() {
                    Ok(message) => {
                        // Handle the received message
                        match message {
                            UnreliableClientMessage::PlayerPosition(player_position) => {
                                // Update player position in the world
                                let query =
                                    self.world.query_mut::<(&PlayerId, &mut PlayerPosition)>();
                                for (id, position) in query {
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
                        }
                    }
                    Err(async_channel::TryRecvError::Empty) => {
                        // No messages available, continue processing
                    }
                    Err(async_channel::TryRecvError::Closed) => {
                        godot_print!("Unreliable channel for player {} closed", player_id);
                        if !leaving_player_vec.contains(player_id) {
                            leaving_player_vec.push(player_id.clone());
                        }
                    }
                }
            }

            // Send messages to clients
            let game_data = self
                .world
                .query::<(&PlayerId, &PlayerPosition)>()
                .iter()
                .map(|(id, position)| UnreliableServerMessage::PlayerPosition(id.clone(), *position))
                .collect::<Vec<UnreliableServerMessage>>();

            for (player_id, message_channels) in channel_map.iter() {
                // Skip players that are leaving
                if leaving_player_vec.contains(player_id) {
                    continue;
                }
                // Get player position in the world
                let reliable_server_sender = &message_channels.reliable_sender;
                let unreliable_server_sender = &message_channels.unreliable_sender;
                // send game data to each player
                for game_data_message in &game_data {
                    // Send player position to the client
                    if let Err(e) = unreliable_server_sender.try_send(game_data_message.clone()) {
                        godot_print!("Failed to send message to player {}: {}", player_id, e);
                    }
                }
                // Send new player messages
                if !new_player_vec.is_empty() {
                    // send new player message to all players
                    let new_player_message = ReliableServerMessage::PlayerJoined {
                        player_ids: new_player_vec.clone(),
                    };
                    if let Err(e) = reliable_server_sender.try_send(new_player_message) {
                        godot_print!(
                            "Failed to send new player message to player {}: {}",
                            player_id,
                            e
                        );
                    }
                }
                // Send leaving player messages
                if !leaving_player_vec.is_empty() {
                    let leaving_player_message = ReliableServerMessage::PlayerLeft {
                        player_ids: leaving_player_vec.clone(),
                    };
                    if let Err(e) = reliable_server_sender.try_send(leaving_player_message) {
                        godot_print!(
                            "Failed to send leaving player message to player {}: {}",
                            player_id,
                            e
                        );
                    }
                }
            }

            // remove channels from players that have left
            for player_id in &leaving_player_vec {
                // Signal cancellation before removing
                if let Some(channels) = channel_map.get(player_id) {
                    let _ = channels.cancel_sender.send(true);
                }
                channel_map.remove(player_id);

                // Remove player entities from the ECS world
                let query = self.world.query_mut::<(Entity, &PlayerId)>();
                let mut entities_to_despawn = Vec::new();
                for (entity, id) in query {
                    if *id == *player_id {
                        entities_to_despawn.push(entity);
                    }
                }
                for entity in entities_to_despawn {
                    let _ = self.world.despawn(entity);
                }
            }
        }
    }

    #[func]
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
    pub fn close_server(&mut self) {
        if let NetworkState::ServerConnection(server) = &mut self.network_state {
            // Clean up resources if necessary

            let mut channel_map = server.channel_map.lock().unwrap();
            for (_player_id, message_channels) in channel_map.iter() {
                // shut down the tasks for each player
                message_channels.cancel_sender.send(true).unwrap();
            }
            channel_map.clear(); // Clear the channel map on exit

            // clean up the join set
            AsyncRuntime::block_on(server.join_set.shutdown());
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

    #[func]
    pub fn get_connection_type(&self) -> GString {
        let singleton = GameState::singleton();
        let singleton = singleton.bind();
        match &singleton.network_state {
            NetworkState::ClientConnection(_, _) => GString::from("Client"),
            NetworkState::ServerConnection(_) => GString::from("Server"),
            NetworkState::HostConnection(_, _, _) => GString::from("Host"),
            NetworkState::None => GString::from("None"),
        }
    }
}
