use crate::ui::App;
use std::rc::Rc;
use klemmbrett::{HistoryApi, HISTORY_BUS_NAME, HISTORY_OBJECT_PATH};

/// Serves `ShowMenu(x, y)` on the session bus and forwards requests to GTK.
pub fn spawn(app: Rc<App>) {
    #[allow(deprecated)]
    let (tx, rx) = glib::MainContext::channel(glib::Priority::DEFAULT);
    rx.attach(None, move |(x, y)| {
        app.show_history_menu(x, y);
        glib::ControlFlow::Continue
    });

    std::thread::Builder::new()
        .name("klemmbrett-dbus".into())
        .spawn(move || {
            let api = HistoryApi::new(move |x, y| {
                if let Err(err) = tx.send((x, y)) {
                    eprintln!("Failed to request the history menu: {err}");
                }
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
