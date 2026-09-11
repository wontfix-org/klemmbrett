mod dbus;
mod ui;

use clap::Parser;
use std::path::PathBuf;
use std::rc::Rc;
use klemmbrett::{
    default_db_path, ArboardBackend, ClipboardSync, HistoryStore, DEFAULT_HISTORY_LIMIT,
    DEFAULT_LABEL_MAX, POLL_INTERVAL,
};

#[derive(Parser, Debug)]
#[command(
    about = "Keep the Wayland clipboard and primary selection in sync, with a history popup."
)]
struct Args {
    /// Maximum number of history rows stored in SQLite
    #[arg(long, default_value_t = DEFAULT_HISTORY_LIMIT)]
    history_limit: usize,

    /// Maximum characters shown for each history item in the popup
    #[arg(long, default_value_t = DEFAULT_LABEL_MAX)]
    label_max: usize,

    /// SQLite file used for history (defaults to $XDG_DATA_HOME/klemmbrett/history.sqlite)
    #[arg(long)]
    db: Option<PathBuf>,
}

fn main() {
    let args = Args::parse();
    gtk::init().expect("Failed to initialize GTK.");

    let db_path = args.db.unwrap_or_else(default_db_path);
    let history = HistoryStore::open(&db_path, args.history_limit).unwrap_or_else(|err| {
        panic!("Failed to open history database {}: {err}", db_path.display());
    });

    let backend = ArboardBackend::new().expect("Failed to initialize the Wayland clipboard.");
    let sync = ClipboardSync::new(backend);

    let app = Rc::new(ui::App::new(sync, history, args.label_max));
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
