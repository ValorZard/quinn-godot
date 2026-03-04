use godot::prelude::*;

struct RustExtension;

#[gdextension]
unsafe impl ExtensionLibrary for RustExtension {}

mod async_runtime;
mod client_button;
mod game_state;
mod player;
mod server_button;
