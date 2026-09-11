//! Talks to the history object over the session bus, the same way a compositor
//! key binding would.

use std::sync::mpsc;
use std::time::Duration;
use klemmbrett::{HistoryApi, HISTORY_INTERFACE, HISTORY_OBJECT_PATH};

#[test]
fn show_menu_forwards_the_pointer_coordinates() {
    let Ok(builder) = zbus::blocking::connection::Builder::session() else {
        eprintln!("skipping: no session bus");
        return;
    };

    let (tx, rx) = mpsc::channel();
    let api = HistoryApi::new(move |x, y| {
        let _ = tx.send((x, y));
    });

    let name = format!(
        "org.klemmbrett.test{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    let _server = match builder
        .name(name.as_str())
        .and_then(|b| b.serve_at(HISTORY_OBJECT_PATH, api))
        .and_then(|b| b.build())
    {
        Ok(conn) => conn,
        Err(err) => {
            eprintln!("skipping: could not claim a test bus name: {err}");
            return;
        }
    };

    let client = match zbus::blocking::Connection::session() {
        Ok(conn) => conn,
        Err(err) => {
            eprintln!("skipping: could not connect to the session bus: {err}");
            return;
        }
    };

    client
        .call_method(
            Some(name.as_str()),
            HISTORY_OBJECT_PATH,
            Some(HISTORY_INTERFACE),
            "ShowMenu",
            &(120_i32, 80_i32),
        )
        .expect("ShowMenu should succeed");

    let (x, y) = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("ShowMenu should deliver coordinates to the UI thread");
    assert_eq!((x, y), (120, 80));
}
