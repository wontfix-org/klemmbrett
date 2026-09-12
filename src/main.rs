mod dbus;
mod ui;

use clap::Parser;
use std::path::PathBuf;
use std::rc::Rc;
use klemmbrett::{load_config, ArboardBackend, ClipboardSync, Sources, POLL_INTERVAL};

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

    gtk::init().expect("Failed to initialize GTK.");

    let backend = ArboardBackend::new().expect("Failed to initialize the Wayland clipboard.");
    let sync = ClipboardSync::new(backend);

    let app = Rc::new(ui::App::new(sync, sources, config.popup.label_max));
    app.record_current();
    ui::install_styles();
    ui::install_tray(app.clone());
    dbus::spawn(app.clone());

    let poller = app.clone();
    glib::timeout_add_local(POLL_INTERVAL, move || {
        poller.poll();
        glib::ControlFlow::Continue
    });

    gtk::main();
}
