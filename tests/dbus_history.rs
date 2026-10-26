//! Talks to the klemmbrett object over the session bus, the same way a
//! compositor key binding would.

use klemmbrett::{request_show, BusInterface, OBJECT_PATH};
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn show_forwards_the_source_and_pointer_coordinates() {
    let Ok(builder) = zbus::blocking::connection::Builder::session() else {
        eprintln!("skipping: no session bus");
        return;
    };

    let (tx, rx) = mpsc::channel();
    let api = BusInterface::new(move |source, pointer| {
        if source != "snippets" {
            return Err(format!("unknown source {source:?}"));
        }
        let _ = tx.send((source, pointer));
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
        .and_then(|b| b.serve_at(OBJECT_PATH, api))
        .and_then(|b| b.build())
    {
        Ok(conn) => conn,
        Err(err) => {
            eprintln!("skipping: could not claim a test bus name: {err}");
            return;
        }
    };

    request_show(&name, "snippets", Some((120, 80))).expect("Show should succeed");

    let (source, pointer) = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("Show should deliver the source and coordinates to the UI thread");
    assert_eq!((source.as_str(), pointer), ("snippets", Some((120, 80))));

    request_show(&name, "snippets", None).expect("Show with no pointer should succeed");
    let (source, pointer) = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("Show should deliver an empty pointer array");
    assert_eq!((source.as_str(), pointer), ("snippets", None));

    let error = request_show(&name, "missing", Some((120, 80)))
        .expect_err("an unknown source should be rejected");
    assert!(error.to_string().contains("unknown source"));
}
