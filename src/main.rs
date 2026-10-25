mod dbus;
mod systemd;
mod ui;

use clap::Parser;
use klemmbrett::{load_config, ArboardBackend, ClipboardSync, Sources};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    about = "Keep Wayland selections in sync and copy values from configurable popup sources."
)]
struct Args {
    /// HCL configuration file (defaults to $XDG_CONFIG_HOME/klemmbrett/config.hcl)
    #[arg(long)]
    config: Option<PathBuf>,

    /// Install a systemd user unit bound to graphical-session.target and enable it
    #[arg(long)]
    install_systemd: bool,
}

fn main() {
    let args = Args::parse();
    if args.install_systemd {
        systemd::install(args.config.as_deref()).unwrap_or_else(|err| {
            panic!("{err}");
        });
        return;
    }
    let config = load_config(args.config.as_deref()).unwrap_or_else(|err| {
        panic!("{err}");
    });
    let sources = Sources::from_config(&config).unwrap_or_else(|err| {
        panic!("Invalid source configuration: {err}");
    });

    let backend = ArboardBackend::new().expect("Failed to initialize the Wayland clipboard.");
    let sync = ClipboardSync::new(backend);

    ui::run(sync, sources, config.popup.label_max, config.popup.theme);
}
