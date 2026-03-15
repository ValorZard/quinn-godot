# Godot Iroh VectorWar

This is a template project for a multiplayer game, created based on the official [Godot-Rust](https://godot-rust.github.io/book/intro/hello-world.html) guide. It serves as a starting point for developers who want to integrate Rust into their Godot projects for better performance and type safety.

The game itself is a remake of the classic GGPO Example `VectorWar`. The gameplay consists of two or more spaceships shooting at each other on a 2D plane.

The networking backend uses [iroh](https://github.com/n0-computer/iroh) for holepunching.

## Features
- Template project to get started with Godot and Rust.
- Configured to work with Godot Engine and the [Godot Rust bindings](https://github.com/godot-rust/gdext).
- Setup is based on the [Hello World](https://godot-rust.github.io/book/intro/hello-world.html) tutorial from the official Godot-Rust book.

## Requirements
- **Godot Engine** version 4.6 or later.
- **Rust** installed. You can download it from the official website: https://www.rust-lang.org/
- **Cargo** – the Rust package manager, which is included when installing Rust.

## Installation

1. Clone this repository or download the ZIP.

2. Make sure you have Godot and Rust set up correctly.

3. Navigate to the `godot` project folder and open the `project.godot` with Godot.

4. Build the Rust code:
   - In the terminal, go to the project `rust` directory and run:
     ```
     cargo build
     ```

5. Run the project from Godot.

### Visual Studio Code

If you are working with VS Code, I recommend you to use the `rust-analyzer` extension and setting the `Check: Command` to `build`. This enables you library to be compiled each time you save your files, allowing for fast changes to be applied inside the Godot Editor without having to compile them in the terminal yourself each time.

> This is only useful in Godot 4.2+ since it allows to import the changes without reloading the project. Since this template aims for 4.4+, this should not be a problem. Keep this in mind if you try a lesser version though.

## Project Structure

This project follows a very rough [Model-View-Controller](https://developer.mozilla.org/en-US/docs/Glossary/MVC) pattern, where:
- the ECS (in this case [hecs](https://github.com/Ralith/hecs) is the Model
- Godot is the view (gets user input)
- Rust code (in game_logic and game_network) is the controller.

The actual list of crates/directories is as follows:
- `rust`: The Rust directory for writing code.
- `godot`: The Godot project directory, where scenes and assets are located.
- `game_logic`: This is where a lot of the code related to GameState management is located
- `game_network`: This crate handles the actual QUIC/Iroh networking code (99% of your async code should be here)
- `README.md`: This file.
- `LICENSE` The MIT license

## Troubleshooting

- If you encounter issues with Rust not building, ensure your environment is correctly configured by following the steps in the official [Godot-Rust Book](https://godot-rust.github.io/book/intro/hello-world.html).
- For specific issues with the Godot-Rust bindings, refer to the official [GitHub repository](https://github.com/godot-rust/gdext) or consult the community forums.

## License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for more details.

The godot-rust Ferris icon was obtained from [their repository](https://github.com/godot-rust/assets) and its licence's details are explained [here](https://github.com/godot-rust/assets/blob/master/asset-licenses.md).

## Acknowledgments

- [Godot Engine](https://godotengine.org/)
- [Godot Rust](https://github.com/godot-rust/gdext) for their fantastic work on integrating Rust with Godot.
