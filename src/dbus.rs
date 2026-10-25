use klemmbrett::{ActivateApi, ACTIVATE_OBJECT_PATH, HISTORY_BUS_NAME};
use std::collections::HashSet;

/// A request that must run on the GPUI thread.
#[derive(Debug)]
pub enum UiCmd {
    ShowMenu { source: String, x: i32, y: i32 },
    RecordHistory(String),
    Quit,
}

/// Serves `ShowMenu(source, x, y)` and forwards valid requests to GPUI.
pub fn spawn(source_names: HashSet<String>, tx: async_channel::Sender<UiCmd>) {
    std::thread::Builder::new()
        .name("klemmbrett-dbus".into())
        .spawn(move || {
            let api = ActivateApi::new(move |source, x, y| {
                if !source_names.contains(&source) {
                    let err = if source_names.is_empty() {
                        format!("unknown source {source:?}: no popup sources are configured")
                    } else {
                        let mut names: Vec<_> = source_names.iter().cloned().collect();
                        names.sort();
                        format!("unknown source {source:?}; known: {}", names.join(", "))
                    };
                    eprintln!("{err}");
                    return Err(err);
                }
                tx.send_blocking(UiCmd::ShowMenu { source, x, y })
                    .map_err(|err| format!("failed to request popup: {err}"))
            });
            match zbus::blocking::connection::Builder::session()
                .expect("session bus")
                .name(HISTORY_BUS_NAME)
                .expect("request bus name")
                .serve_at(ACTIVATE_OBJECT_PATH, api)
                .expect("export activate object")
                .build()
            {
                Ok(_conn) => std::thread::park(),
                Err(err) => eprintln!("Failed to claim {HISTORY_BUS_NAME} on D-Bus: {err}"),
            }
        })
        .expect("spawn D-Bus thread");
}
