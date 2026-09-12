mod app;
mod body;
mod cli;
mod interface;
mod render;
mod shell;

use app::Pet;
use shell::{LayerWindow, LayerWindowConfig};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let animal_code = match cli::parse(&args, app::DEFAULT_ANIMAL_CODE) {
        Ok(cli::Action::Run { animal_code }) => animal_code,
        Ok(cli::Action::Help) => {
            print!("{}", cli::USAGE);
            return;
        }
        Ok(cli::Action::ListAnimals) => {
            match cli::format_animal_list() {
                Ok(list) => print!("{list}"),
                Err(error) => {
                    eprintln!("vmc-pet: {error}");
                    std::process::exit(1);
                }
            }
            return;
        }
        Err(error) => {
            eprintln!("vmc-pet: {error}");
            eprint!("{}", cli::USAGE);
            std::process::exit(1);
        }
    };

    let pet = match Pet::new(&animal_code) {
        Ok(pet) => pet,
        Err(error) => {
            eprintln!("vmc-pet: {error}");
            eprintln!("vmc-pet: run with --list-animals to see the available codes");
            std::process::exit(1);
        }
    };
    if let Err(error) = LayerWindow::run(LayerWindowConfig::default(), Box::new(pet)) {
        eprintln!("vmc-pet: {error}");
        std::process::exit(1);
    }
}
