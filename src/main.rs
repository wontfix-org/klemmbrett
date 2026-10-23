mod dbus;
mod ui;

use clap::Parser;
use klemmbrett::{load_config, ArboardBackend, ClipboardSync, Sources};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    about = "Keep Wayland selections in sync and copy values from configurable popup sources."
)]
struct Args {
    /// TOML configuration file (defaults to $XDG_CONFIG_HOME/klemmbrett/config.toml)
    #[arg(long)]
    config: Option<PathBuf>,
}

fn main() {
    let args = Args::parse();
    let config = load_config(args.config.as_deref()).unwrap_or_else(|err| {
        panic!("{err}");
    });
    let sources = Sources::from_config(&config).unwrap_or_else(|err| {
        panic!("Invalid source configuration: {err}");
    });

    let backend = ArboardBackend::new().expect("Failed to initialize the Wayland clipboard.");
    let sync = ClipboardSync::new(backend);

    ui::run(sync, sources, config.popup.label_max);
}
