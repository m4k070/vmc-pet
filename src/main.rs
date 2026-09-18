mod app;
mod cli;
mod interface;
mod multichannel_preview;
mod particle_preview;
mod persistence;
mod render;
mod shell;

use app::Pet;
use shell::{LayerWindow, LayerWindowConfig};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let (animal_code, preview) = match cli::parse(&args, app::DEFAULT_ANIMAL_CODE) {
        Ok(cli::Action::Run {
            animal_code,
            preview,
        }) => (animal_code, preview),
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
        Ok(cli::Action::ListMultichannel) => {
            match cli::format_multichannel_list() {
                Ok(list) => print!("{list}"),
                Err(error) => {
                    eprintln!("vmc-pet: {error}");
                    std::process::exit(1);
                }
            }
            return;
        }
        Ok(cli::Action::PreviewParticles {
            seed,
            zoom,
            log,
            controller,
        }) => {
            let preview = particle_preview::ParticlePreview::new(
                seed,
                zoom,
                log.as_deref().map(std::path::Path::new),
                controller,
            );
            if let Err(error) = LayerWindow::run(LayerWindowConfig::default(), Box::new(preview)) {
                eprintln!("vmc-pet: {error}");
                std::process::exit(1);
            }
            return;
        }
        Ok(cli::Action::RunMultichannel { id, preview }) => {
            let surface = match multichannel_preview::MultichannelSurface::body(&id, preview) {
                Ok(surface) => surface,
                Err(error) => {
                    eprintln!("vmc-pet: {error}");
                    eprintln!("vmc-pet: run with --list-multichannel to see the available ids");
                    std::process::exit(1);
                }
            };
            if let Err(error) = LayerWindow::run(LayerWindowConfig::default(), Box::new(surface)) {
                eprintln!("vmc-pet: {error}");
                std::process::exit(1);
            }
            return;
        }
        Ok(cli::Action::PreviewMultichannel { id }) => {
            let preview = match multichannel_preview::MultichannelSurface::display(&id) {
                Ok(preview) => preview,
                Err(error) => {
                    eprintln!("vmc-pet: {error}");
                    eprintln!("vmc-pet: run with --list-multichannel to see the available ids");
                    std::process::exit(1);
                }
            };
            if let Err(error) = LayerWindow::run(LayerWindowConfig::default(), Box::new(preview)) {
                eprintln!("vmc-pet: {error}");
                std::process::exit(1);
            }
            return;
        }
        Err(error) => {
            eprintln!("vmc-pet: {error}");
            eprint!("{}", cli::USAGE);
            std::process::exit(1);
        }
    };

    let pet = match preview {
        Some(state) => Pet::preview(&animal_code, state),
        None => Pet::new(&animal_code),
    };
    let pet = match pet {
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
