use game_core::{client::run_client, ClientMessage, PlayerPosition, ServerMessage};
use godot::{classes::{Button, IButton}, prelude::*};

use crate::{async_runtime::AsyncRuntime, player::{self, Player}};

#[derive(GodotClass)]
#[class(base=Button)]
struct ClientButton {
    server_receiver: Option<async_channel::Receiver<ServerMessage>>,
    client_sender: Option<async_channel::Sender<ClientMessage>>,
    #[export]
    player_ref: Option<Gd<Player>>,
    base: Base<Button>,
}

#[godot_api]
impl IButton for ClientButton {
    fn init(base: Base<Button>) -> Self {
        Self {
            server_receiver: None, // Initialize with None, will be set when the client starts
            client_sender: None, // Initialize with None, will be set when the client starts
            player_ref: None, // Reference to the player, if needed
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
            match receiver.try_recv() {
                Ok(message) => {
                    godot_print!("Received message from server: {:?}", message);
                    // Handle the received message
                }
                Err(async_channel::TryRecvError::Empty) => {
                    // No messages available, continue processing
                }
                Err(async_channel::TryRecvError::Closed) => {
                    godot_print!("Server channel closed");
                    self.server_receiver = None; // Reset the receiver if the channel is closed
                }
            }
        }


        if let Some(sender) = &self.client_sender {
            // Here you can send messages to the server if needed
            // For example, you might want to send a heartbeat or a status update
            if let Some(player_ref) = &self.player_ref {
                let player = player_ref.bind();
                let position = player.base().get_position();
                let message = ClientMessage::PlayerPosition(PlayerPosition {
                    x: position.x,
                    y: position.y,
                });
                if let Err(e) = sender.try_send(message) {
                    godot_print!("Failed to send player position: {:?}", e);
                }
            }
        }
    }
}