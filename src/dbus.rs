use klemmbrett::{BusInterface, HISTORY_BUS_NAME, OBJECT_PATH};
use std::collections::HashSet;

/// A request that must run on the GPUI thread.
#[derive(Debug)]
pub enum UiCmd {
    Show {
        source: String,
        pointer: Option<(i32, i32)>,
    },
    RecordHistory(String),
    Quit,
}

/// Serves `Show(source, pointer)` and forwards valid requests to GPUI.
pub fn spawn(source_names: HashSet<String>, tx: async_channel::Sender<UiCmd>) {
    std::thread::Builder::new()
        .name("klemmbrett-dbus".into())
        .spawn(move || {
            let api = BusInterface::new(move |source, pointer| {
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
                tx.send_blocking(UiCmd::Show { source, pointer })
                    .map_err(|err| format!("failed to request popup: {err}"))
            });
            match zbus::blocking::connection::Builder::session()
                .expect("session bus")
                .name(HISTORY_BUS_NAME)
                .expect("request bus name")
                .serve_at(OBJECT_PATH, api)
                .expect("export klemmbrett object")
                .build()
            {
                Ok(_conn) => std::thread::park(),
                Err(err) => eprintln!("Failed to claim {HISTORY_BUS_NAME} on D-Bus: {err}"),
            }
        })
        .expect("spawn D-Bus thread");
}
