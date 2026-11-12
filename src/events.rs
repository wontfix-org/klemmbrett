//! Fan-out of daemon events to Varlink `Subscribe` callers.

use serde_json::Value;
use std::collections::HashSet;
use std::sync::mpsc;
use std::sync::Mutex;

struct Subscriber {
    widgets: Option<HashSet<String>>,
    tx: mpsc::Sender<Value>,
}

static SUBSCRIBERS: Mutex<Vec<Subscriber>> = Mutex::new(Vec::new());

/// Receives every event, or only those about `widgets` when given.
pub fn subscribe(widgets: Option<HashSet<String>>) -> mpsc::Receiver<Value> {
    let (tx, rx) = mpsc::channel();
    if let Ok(mut subscribers) = SUBSCRIBERS.lock() {
        subscribers.push(Subscriber { widgets, tx });
    }
    rx
}

/// Sends `event` to every interested subscriber and forgets the ones that left.
/// An event without a widget reaches only subscribers that did not filter.
pub fn publish(event: Value, widget: Option<&str>) {
    let Ok(mut subscribers) = SUBSCRIBERS.lock() else {
        return;
    };
    subscribers.retain(|subscriber| {
        let wanted = match (&subscriber.widgets, widget) {
            (None, _) => true,
            (Some(names), Some(widget)) => names.contains(widget),
            (Some(_), None) => false,
        };
        !wanted || subscriber.tx.send(event.clone()).is_ok()
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn filtered_subscribers_only_see_their_widgets() {
        let all = subscribe(None);
        let inbox = subscribe(Some(HashSet::from(["inbox-test".to_string()])));
        publish(
            json!({"kind": "changed", "widget": "inbox-test"}),
            Some("inbox-test"),
        );
        publish(json!({"kind": "changed", "widget": "other"}), Some("other"));
        publish(json!({"kind": "notification"}), None);
        let seen: Vec<_> = inbox.try_iter().collect();
        assert_eq!(seen, [json!({"kind": "changed", "widget": "inbox-test"})]);
        assert!(all.try_iter().count() >= 3);
    }
}
