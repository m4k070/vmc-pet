mod app;
mod body;
mod render;
mod shell;

use app::Pet;
use shell::{LayerWindow, LayerWindowConfig};

fn main() {
    let pet = match Pet::new() {
        Ok(pet) => pet,
        Err(error) => {
            eprintln!("vmc-pet: {error}");
            std::process::exit(1);
        }
    };
    if let Err(error) = LayerWindow::run(LayerWindowConfig::default(), Box::new(pet)) {
        eprintln!("vmc-pet: {error}");
        std::process::exit(1);
    }
}
