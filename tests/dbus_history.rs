//! Talks to the klemmbrett object over the session bus, the same way a
//! compositor key binding would.

use klemmbrett::{
    request_prompt, request_show, BusInterface, Record, ShowRequest, INTERFACE, OBJECT_PATH,
};
use std::collections::BTreeMap;
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn show_forwards_the_widget_pointer_and_records() {
    let Ok(builder) = zbus::blocking::connection::Builder::session() else {
        eprintln!("skipping: no session bus");
        return;
    };

    let (tx, rx) = mpsc::channel();
    let api = BusInterface::new(move |request: ShowRequest| {
        if request.widget != "snippets" {
            return Err(format!("unknown widget {:?}", request.widget));
        }
        if let Some(reply) = request.reply {
            let values: Vec<String> = request
                .records
                .iter()
                .flatten()
                .map(|record| record.value.clone())
                .collect();
            let values = if values.is_empty() {
                vec!["chosen".into()]
            } else {
                values
            };
            let _ = reply.send(Ok(values));
        }
        let _ = tx.send((request.widget, request.pointer, request.records));
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

    request_show(&name, "snippets", Some((120, 80)), &[]).expect("Show should succeed");

    let (widget, pointer, records) = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("Show should deliver the widget and coordinates to the UI thread");
    assert_eq!(
        (widget.as_str(), pointer, records),
        ("snippets", Some((120, 80)), None)
    );

    request_show(&name, "snippets", None, &[]).expect("Show with no pointer should succeed");
    let (widget, pointer, records) = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("Show should deliver an empty pointer array");
    assert_eq!(
        (widget.as_str(), pointer, records),
        ("snippets", None, None)
    );

    let override_rows = vec![Record {
        key: "id".into(),
        label: "Readable".into(),
        value: "copied".into(),
    }];
    request_show(&name, "snippets", None, &override_rows)
        .expect("Show with records should succeed");
    let (widget, pointer, records) = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("Show should deliver the override records");
    assert_eq!(
        (widget.as_str(), pointer, records),
        ("snippets", None, Some(override_rows.clone()))
    );

    let error = request_show(&name, "missing", Some((120, 80)), &[])
        .expect_err("an unknown widget should be rejected");
    assert!(error.to_string().contains("unknown widget"));

    let values = request_prompt(&name, "snippets", None, &override_rows)
        .expect("Prompt should return the selected values");
    assert_eq!(values, vec!["copied".to_string()]);
    let (_widget, _pointer, records) = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("Prompt should deliver the same request shape as Show");
    assert_eq!(records, Some(override_rows));

    let values = request_prompt(&name, "snippets", None, &[])
        .expect("Prompt without records should still return a selection");
    assert_eq!(values, vec!["chosen".to_string()]);

    let mut missing_value = BTreeMap::new();
    missing_value.insert("key".to_string(), "id".to_string());
    let conn = zbus::blocking::Connection::session().expect("session bus");
    let error = conn
        .call_method(
            Some(name.as_str()),
            OBJECT_PATH,
            Some(INTERFACE),
            "Show",
            &("snippets", Vec::<(i32, i32)>::new(), vec![missing_value]),
        )
        .expect_err("a record without value should be rejected");
    assert!(error.to_string().contains("value is required"), "{error}");
}
