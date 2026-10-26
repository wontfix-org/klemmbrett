use klemmbrett::{BusInterface, HISTORY_BUS_NAME, OBJECT_PATH};
use std::collections::HashSet;

/// A request that must run on the GPUI thread.
#[derive(Debug)]
pub enum UiCmd {
    Show {
        widget: String,
        pointer: Option<(i32, i32)>,
    },
    RecordHistory(String),
    Quit,
}

/// Serves `Show(widget, pointer)` and forwards valid requests to GPUI.
pub fn spawn(widget_names: HashSet<String>, tx: async_channel::Sender<UiCmd>) {
    std::thread::Builder::new()
        .name("klemmbrett-dbus".into())
        .spawn(move || {
            let api = BusInterface::new(move |widget, pointer| {
                if !widget_names.contains(&widget) {
                    let err = if widget_names.is_empty() {
                        format!("unknown widget {widget:?}: no popup widgets are configured")
                    } else {
                        let mut names: Vec<_> = widget_names.iter().cloned().collect();
                        names.sort();
                        format!("unknown widget {widget:?}; known: {}", names.join(", "))
                    };
                    eprintln!("{err}");
                    return Err(err);
                }
                tx.send_blocking(UiCmd::Show { widget, pointer })
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
