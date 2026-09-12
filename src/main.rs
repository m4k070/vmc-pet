mod app;
mod shell;

use app::Pet;
use shell::{LayerWindow, LayerWindowConfig};

fn main() {
    let config = LayerWindowConfig::default();
    if let Err(error) = LayerWindow::run(config, Box::new(Pet::new())) {
        eprintln!("vmc-pet: {error}");
        std::process::exit(1);
    }
}
