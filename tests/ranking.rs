//! Picks recorded in the usage store reorder a widget's rows, from the
//! ranking the Python config sends to the order the popup shows.

use klemmbrett::rank::{self, Priors};
use klemmbrett::{Ranking, Record, UsageStore};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Mutex;

const DAY: i64 = 86_400;

/// What `kb.widget("run", providers.Executables())` sends.
fn executables_default() -> Ranking {
    Ranking::from_json(&json!({
        "match": { "kind": "fuzzy", "scheme": "default", "case": "smart", "syntax": true },
        "rank": [
            { "kind": "frecency", "weight": 1.0, "half_life": 259200.0 },
            { "kind": "learned", "weight": 1.0, "half_life": 1209600.0, "prefix": 3 },
        ],
        "prefer": "blend",
    }))
    .unwrap()
}

fn shown(
    entries: &[Record],
    query: &str,
    ranking: &Ranking,
    store: &Mutex<UsageStore>,
    now: i64,
) -> Vec<String> {
    let priors = Priors::load(ranking, "run", &HashMap::new(), Some(store), now);
    rank::order(entries, query, ranking, &[], &priors)
        .into_iter()
        .map(|index| entries[index].key.clone())
        .collect()
}

#[test]
fn picks_move_rows_up_for_the_empty_query_and_their_prefix() {
    let entries: Vec<Record> = ["firewall-config", "firefox", "fish", "foot"]
        .into_iter()
        .map(Record::new)
        .collect();
    let ranking = executables_default();
    let store = Mutex::new(UsageStore::open_in_memory().unwrap());
    let now = 1000 * DAY;

    assert_eq!(
        shown(&entries, "", &ranking, &store, now),
        ["firewall-config", "firefox", "fish", "foot"]
    );
    assert_eq!(
        shown(&entries, "fi", &ranking, &store, now)[0],
        "firewall-config"
    );

    for at in [now - DAY, now - 60] {
        store
            .lock()
            .unwrap()
            .record("run", "fish", "Fi", at)
            .unwrap();
    }
    assert_eq!(shown(&entries, "", &ranking, &store, now)[0], "fish");
    assert_eq!(shown(&entries, "fi", &ranking, &store, now)[0], "fish");
    assert_eq!(shown(&entries, "fis", &ranking, &store, now), ["fish"]);
    assert_eq!(shown(&entries, "foo", &ranking, &store, now), ["foot"]);

    store.lock().unwrap().clear("run").unwrap();
    assert_eq!(
        shown(&entries, "", &ranking, &store, now)[0],
        "firewall-config"
    );
}

#[test]
fn widgets_without_usage_terms_ignore_the_store() {
    let entries: Vec<Record> = ["b", "a"].into_iter().map(Record::new).collect();
    let ranking = Ranking::from_json(
        &json!({ "match": { "kind": "substring" }, "rank": [], "prefer": "blend" }),
    )
    .unwrap();
    let store = Mutex::new(UsageStore::open_in_memory().unwrap());
    store.lock().unwrap().record("run", "a", "", 0).unwrap();
    assert!(!ranking.uses_usage());
    assert_eq!(shown(&entries, "", &ranking, &store, 1), ["b", "a"]);
}
