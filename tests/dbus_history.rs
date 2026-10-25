//! Talks to the activate object over the session bus, the same way a compositor
//! key binding would.

use klemmbrett::{request_show_menu, ActivateApi, ACTIVATE_OBJECT_PATH};
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn show_menu_forwards_the_source_and_pointer_coordinates() {
    let Ok(builder) = zbus::blocking::connection::Builder::session() else {
        eprintln!("skipping: no session bus");
        return;
    };

    let (tx, rx) = mpsc::channel();
    let api = ActivateApi::new(move |source, x, y| {
        if source != "snippets" {
            return Err(format!("unknown source {source:?}"));
        }
        let _ = tx.send((source, x, y));
        Ok(())
    });

    let name = format!(
        "org.wontfix.klemmbrett.test{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    let _server = match builder
        .name(name.as_str())
        .and_then(|b| b.serve_at(ACTIVATE_OBJECT_PATH, api))
        .and_then(|b| b.build())
    {
        Ok(conn) => conn,
        Err(err) => {
            eprintln!("skipping: could not claim a test bus name: {err}");
            return;
        }
    };

    request_show_menu(&name, "snippets", 120, 80).expect("ShowMenu should succeed");

    let (source, x, y) = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("ShowMenu should deliver the source and coordinates to the UI thread");
    assert_eq!((source.as_str(), x, y), ("snippets", 120, 80));

    let error = request_show_menu(&name, "missing", 120, 80)
        .expect_err("an unknown source should be rejected");
    assert!(error.to_string().contains("unknown source"));
}
