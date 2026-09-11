use klemmbrett::{ArboardBackend, ClipboardSync, POLL_INTERVAL};

fn main() {
    println!("Starting Wayland Clipboard <-> Primary Selection Sync Tool...");

    let backend = ArboardBackend::new().expect("Failed to initialize the Wayland clipboard.");
    ClipboardSync::new(backend).run(POLL_INTERVAL);
}
