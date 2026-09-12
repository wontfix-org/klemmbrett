use crate::ui::App;
use std::collections::HashSet;
use std::rc::Rc;
use klemmbrett::{HistoryApi, HISTORY_BUS_NAME, HISTORY_OBJECT_PATH};

/// Serves `ShowMenu(source, x, y)` and forwards valid requests to GTK.
pub fn spawn(app: Rc<App>) {
    let source_names: HashSet<_> = app.source_names().into_iter().collect();
    #[allow(deprecated)]
    let (tx, rx) = glib::MainContext::channel::<(String, i32, i32)>(glib::Priority::DEFAULT);
    rx.attach(None, move |(source, x, y)| {
        app.show_source_menu(&source, x, y);
        glib::ControlFlow::Continue
    });

    std::thread::Builder::new()
        .name("klemmbrett-dbus".into())
        .spawn(move || {
            let api = HistoryApi::new(move |source, x, y| {
                if !source_names.contains(&source) {
                    return Err(format!("unknown source {source:?}"));
                }
                tx.send((source, x, y))
                    .map_err(|err| format!("failed to request popup: {err}"))
            });
            match zbus::blocking::connection::Builder::session()
                .expect("session bus")
                .name(HISTORY_BUS_NAME)
                .expect("request bus name")
                .serve_at(HISTORY_OBJECT_PATH, api)
                .expect("export history object")
                .build()
            {
                Ok(_conn) => std::thread::park(),
                Err(err) => eprintln!("Failed to claim {HISTORY_BUS_NAME} on D-Bus: {err}"),
            }
        })
        .expect("spawn D-Bus thread");
}
