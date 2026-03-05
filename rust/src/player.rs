use game_core::DEFAULT_PLAYER_ID;
use godot::classes::{Input, Sprite2D};
use godot::prelude::*;

#[derive(GodotClass)]
#[class(base=Sprite2D)]
pub struct Player {
    speed: f32,
    angular_speed: f64,
    pub player_id: game_core::PlayerId,
    #[export]
    pub is_local: bool,

    base: Base<Sprite2D>,
}

use godot::classes::ISprite2D;

#[godot_api]
impl ISprite2D for Player {
    fn init(base: Base<Sprite2D>) -> Self {
        godot_print!("Hello, world!"); // Prints to the Godot console

        Self {
            speed: 400.0,
            angular_speed: std::f64::consts::PI,
            player_id: DEFAULT_PLAYER_ID, // Default player ID, can be set later
            is_local: false,              // Default to not being local, can be set later
            base,
        }
    }

    fn ready(&mut self) {}

    fn physics_process(&mut self, delta: f32) {
        // GDScript code:
        //
        // rotation += angular_speed * delta
        // var velocity = Vector2.UP.rotated(rotation) * speed
        // position += velocity * delta

        if self.is_local {
            let mut velocity = Vector2::new(0.0, 0.0);

            // Note: exact=false by default, in Rust we have to provide it explicitly
            let input = Input::singleton();
            if input.is_action_pressed("move_right") {
                velocity += Vector2::RIGHT;
            }
            if input.is_action_pressed("move_left") {
                velocity += Vector2::LEFT;
            }
            if input.is_action_pressed("move_down") {
                velocity += Vector2::DOWN;
            }
            if input.is_action_pressed("move_up") {
                velocity += Vector2::UP;
            }

            if velocity.length() > 0.0 {
                velocity = velocity.normalized() * self.speed;
            }

            let change = velocity * delta;
            let position = self.base().get_global_position() + change;
            self.base_mut().set_global_position(position);
        }

        // or verbose:
        // let this = self.base_mut();
        // this.set_position(
        //     this.position() + velocity * delta as f32
        // );
    }
}

#[godot_api]
impl Player {
    #[func]
    pub fn set_player_id(&mut self, player_id: game_core::PlayerId) {
        self.player_id = player_id;
    }

    #[func]
    fn increase_speed(&mut self, amount: f32) {
        self.speed += amount;
        self.base_mut().emit_signal("speed_increased", &[]);
    }

    #[func]
    fn get_player_id(&self) -> GString {
        GString::from(&self.player_id)
    }

    #[signal]
    fn speed_increased();
}
