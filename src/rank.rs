//! How a widget's rows are matched against the query and ordered by past use.
//!
//! A [`Ranking`] is declared in Python and runs here on every keystroke. The
//! matcher scores how well a row fits the query; the [`Priors`] score rows by
//! how they were picked before, loaded once when the popup opens.

use std::collections::HashMap;
use std::sync::Mutex;

use nucleo_matcher::pattern::{Atom, AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use serde::Deserialize;
use serde_json::Value;

use crate::data::DataPath;
use crate::usage::{normalize_query, NamespaceUsage, UsageStore};
use crate::widget::Record;

/// How much of the rank score's logarithm `Prefer::Blend` adds to the match quality.
const BLEND: f64 = 0.5;
/// `Scheme::History` rounds match quality up to quarters, so the provider's
/// order decides between rows that match about equally well.
const HISTORY_BUCKETS: f64 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    #[default]
    Default,
    Path,
    History,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Case {
    #[default]
    Smart,
    Ignore,
    Respect,
}

fn yes() -> bool {
    true
}

/// Which rows a query keeps, and how well each one matches.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum MatchSpec {
    /// The query occurs in the text, ignoring case.
    #[default]
    Substring,
    /// The text starts with the query, ignoring case.
    Prefix,
    /// Every space-separated word occurs in some text, in any order.
    Words,
    /// fzf-style fuzzy matching; every word must match some text.
    Fuzzy {
        #[serde(default)]
        scheme: Scheme,
        #[serde(default)]
        case: Case,
        #[serde(default = "yes")]
        syntax: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Prefer {
    /// Match quality plus half the logarithm of the rank score.
    #[default]
    Blend,
    /// Best matches first; the rank breaks ties.
    Match,
    /// Highest rank first; the match only filters.
    Usage,
}

/// One ranker. Usage-based kinds read `namespace`, the widget's name by default.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Term {
    #[serde(default = "one")]
    pub weight: f64,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(flatten)]
    pub kind: TermKind,
}

fn one() -> f64 {
    1.0
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TermKind {
    /// Picks, each decayed by `half_life` seconds.
    Frecency { half_life: f64 },
    /// 1 for the latest pick, 1/2 for the one before, and so on.
    Recent,
    /// Plain number of picks.
    Count,
    /// Picks made after typing what is typed now, compared on `prefix` characters.
    Learned { prefix: usize, half_life: f64 },
    /// `boost` for the given keys.
    Pinned { keys: Vec<String>, boost: f64 },
    /// The score Python computed for each record when it was read.
    Python,
}

impl TermKind {
    fn uses_usage(&self) -> bool {
        matches!(
            self,
            TermKind::Frecency { .. }
                | TermKind::Recent
                | TermKind::Count
                | TermKind::Learned { .. }
        )
    }
}

/// A widget's matcher, rankers, and how they combine.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Ranking {
    #[serde(default, rename = "match")]
    pub matching: MatchSpec,
    #[serde(default)]
    pub rank: Vec<Term>,
    #[serde(default)]
    pub prefer: Prefer,
}

impl Ranking {
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let ranking = Ranking::deserialize(value).map_err(|err| format!("ranking: {err}"))?;
        for term in &ranking.rank {
            if !term.weight.is_finite() {
                return Err("ranking: weights must be finite numbers".into());
            }
            match &term.kind {
                TermKind::Frecency { half_life } | TermKind::Learned { half_life, .. }
                    if !(half_life.is_finite() && *half_life > 0.0) =>
                {
                    return Err("ranking: half_life must be greater than zero".into());
                }
                TermKind::Learned { prefix: 0, .. } => {
                    return Err("ranking: learned prefix must be at least 1".into());
                }
                _ => {}
            }
        }
        Ok(ranking)
    }

    /// Whether picks have to be recorded for this widget.
    pub fn uses_usage(&self) -> bool {
        self.rank.iter().any(|term| term.kind.uses_usage())
    }

    /// The namespaces picks of `widget` are recorded under.
    pub fn usage_namespaces(&self, widget: &str) -> Vec<String> {
        let mut seen = Vec::new();
        for term in self.rank.iter().filter(|term| term.kind.uses_usage()) {
            let namespace = term.namespace.clone().unwrap_or_else(|| widget.to_owned());
            if !seen.contains(&namespace) {
                seen.push(namespace);
            }
        }
        seen
    }

    pub fn has_python(&self) -> bool {
        self.rank.iter().any(|term| term.kind == TermKind::Python)
    }
}

struct LearnedPicks {
    prefix: usize,
    /// (normalized query, key, weighted score)
    picks: Vec<(String, String, f64)>,
}

/// Rank scores for the rows of one popup, loaded when it opens.
#[derive(Default)]
pub struct Priors {
    base: HashMap<String, f64>,
    learned: Vec<LearnedPicks>,
}

impl Priors {
    /// Scores from `usage` (per namespace), Python's scores by key, and pins.
    pub fn compute(
        ranking: &Ranking,
        widget: &str,
        usage: &HashMap<String, NamespaceUsage>,
        python: &HashMap<String, f64>,
        now: i64,
    ) -> Self {
        let mut priors = Priors::default();
        let empty = NamespaceUsage::default();
        for term in &ranking.rank {
            let namespace = term.namespace.as_deref().unwrap_or(widget);
            let used = usage.get(namespace).unwrap_or(&empty);
            let weight = term.weight;
            let mut add = |key: &str, score: f64| {
                *priors.base.entry(key.to_owned()).or_default() += weight * score;
            };
            match &term.kind {
                TermKind::Frecency { half_life } => {
                    for (key, used) in &used.keys {
                        add(key, used.frecency(now, *half_life));
                    }
                }
                TermKind::Count => {
                    for (key, used) in &used.keys {
                        add(key, used.count as f64);
                    }
                }
                TermKind::Recent => {
                    let mut keys: Vec<_> = used.keys.iter().collect();
                    keys.sort_by(|a, b| b.1.last_used.cmp(&a.1.last_used).then(a.0.cmp(b.0)));
                    for (rank, (key, _)) in keys.into_iter().enumerate() {
                        add(key, 1.0 / (1.0 + rank as f64));
                    }
                }
                TermKind::Pinned { keys, boost } => {
                    for key in keys {
                        add(key, *boost);
                    }
                }
                TermKind::Python => {
                    for (key, score) in python {
                        add(key, *score);
                    }
                }
                TermKind::Learned { prefix, half_life } => {
                    priors.learned.push(LearnedPicks {
                        prefix: *prefix,
                        picks: used
                            .queries
                            .iter()
                            .map(|(query, key, used)| {
                                (
                                    query.clone(),
                                    key.clone(),
                                    weight * used.frecency(now, *half_life),
                                )
                            })
                            .collect(),
                    });
                }
            }
        }
        priors
    }

    /// Reads the namespaces `ranking` uses from `store` and computes the priors.
    /// A store that fails to read leaves usage out and logs why.
    pub fn load(
        ranking: &Ranking,
        widget: &str,
        python: &HashMap<String, f64>,
        store: Option<&Mutex<UsageStore>>,
        now: i64,
    ) -> Self {
        let mut usage = HashMap::new();
        if let Some(store) = store.filter(|_| ranking.uses_usage()) {
            let store = store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for namespace in ranking.usage_namespaces(widget) {
                match store.namespace(&namespace) {
                    Ok(used) => {
                        usage.insert(namespace, used);
                    }
                    Err(err) => crate::journal::error(&format!(
                        "failed to read usage for {namespace:?}: {err}"
                    )),
                }
            }
        }
        Self::compute(ranking, widget, &usage, python, now)
    }

    /// Scores for `query`: the base scores plus what was learned for it.
    fn for_query(&self, query: &str) -> HashMap<&str, f64> {
        let mut scores: HashMap<&str, f64> = self
            .base
            .iter()
            .map(|(key, score)| (key.as_str(), *score))
            .collect();
        let query = normalize_query(query);
        for learned in &self.learned {
            let typed: String = query.chars().take(learned.prefix).collect();
            if typed.is_empty() {
                continue;
            }
            for (picked, key, score) in &learned.picks {
                if picked.starts_with(&typed) || typed.starts_with(picked.as_str()) {
                    *scores.entry(key.as_str()).or_default() += score;
                }
            }
        }
        scores
    }
}

fn signed_log(score: f64) -> f64 {
    score.signum() * score.abs().ln_1p()
}

fn texts(record: &Record, search: &[DataPath]) -> Vec<String> {
    if search.is_empty() {
        vec![record.label.clone()]
    } else {
        search.iter().flat_map(|path| record.texts(path)).collect()
    }
}

/// Matches texts against one query with one matcher.
enum Compiled {
    All,
    Substring(String),
    Prefix(String),
    Words(Vec<String>),
    Fuzzy {
        atoms: Vec<Atom>,
        matcher: Box<Matcher>,
        buffer: Vec<char>,
    },
}

impl Compiled {
    fn new(spec: &MatchSpec, query: &str) -> Self {
        if query.trim().is_empty() {
            return Compiled::All;
        }
        match spec {
            MatchSpec::Substring => Compiled::Substring(query.to_lowercase()),
            MatchSpec::Prefix => Compiled::Prefix(query.to_lowercase()),
            MatchSpec::Words => {
                Compiled::Words(query.split_whitespace().map(str::to_lowercase).collect())
            }
            MatchSpec::Fuzzy {
                scheme,
                case,
                syntax,
            } => {
                let case = match case {
                    Case::Smart => CaseMatching::Smart,
                    Case::Ignore => CaseMatching::Ignore,
                    Case::Respect => CaseMatching::Respect,
                };
                let pattern = if *syntax {
                    Pattern::parse(query, case, Normalization::Smart)
                } else {
                    Pattern::new(query, case, Normalization::Smart, AtomKind::Fuzzy)
                };
                let config = match scheme {
                    Scheme::Path => Config::DEFAULT.match_paths(),
                    Scheme::Default | Scheme::History => Config::DEFAULT,
                };
                if pattern.atoms.is_empty() {
                    return Compiled::All;
                }
                Compiled::Fuzzy {
                    atoms: pattern.atoms,
                    matcher: Box::new(Matcher::new(config)),
                    buffer: Vec::new(),
                }
            }
        }
    }

    /// How well `texts` match, or `None`. Only fuzzy matching grades matches.
    fn score(&mut self, texts: &[String]) -> Option<f64> {
        let lower = || texts.iter().map(|text| text.to_lowercase());
        match self {
            Compiled::All => Some(1.0),
            Compiled::Substring(query) => lower()
                .any(|text| text.contains(query.as_str()))
                .then_some(1.0),
            Compiled::Prefix(query) => lower()
                .any(|text| text.starts_with(query.as_str()))
                .then_some(1.0),
            Compiled::Words(words) => {
                let texts: Vec<String> = lower().collect();
                words
                    .iter()
                    .all(|word| texts.iter().any(|text| text.contains(word.as_str())))
                    .then_some(1.0)
            }
            Compiled::Fuzzy {
                atoms,
                matcher,
                buffer,
            } => {
                let mut total = 0.0;
                for atom in atoms.iter() {
                    let mut scores = texts
                        .iter()
                        .map(|text| atom.score(Utf32Str::new(text, buffer), matcher));
                    if atom.negative {
                        if !scores.all(|score| score.is_some()) {
                            return None;
                        }
                    } else {
                        total += f64::from(scores.flatten().max()?);
                    }
                }
                Some(total)
            }
        }
    }

    fn graded(&self) -> bool {
        matches!(self, Compiled::Fuzzy { .. })
    }
}

/// The indices of the `entries` that match `query`, best first.
///
/// Without a query, rows are ordered by their priors, keeping the provider's
/// order among equal ones (and entirely when nothing is ranked).
pub fn order(
    entries: &[Record],
    query: &str,
    ranking: &Ranking,
    search: &[DataPath],
    priors: &Priors,
) -> Vec<usize> {
    let mut compiled = Compiled::new(&ranking.matching, query);
    let mut scored: Vec<(usize, f64)> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, record)| {
            compiled
                .score(&texts(record, search))
                .map(|score| (index, score))
        })
        .collect();
    let ranked = !ranking.rank.is_empty();
    if !compiled.graded() && !ranked {
        return scored.into_iter().map(|(index, _)| index).collect();
    }
    if compiled.graded() {
        let best = scored.iter().map(|(_, score)| *score).fold(0.0, f64::max);
        let history = matches!(
            ranking.matching,
            MatchSpec::Fuzzy {
                scheme: Scheme::History,
                ..
            }
        );
        for (_, score) in &mut scored {
            *score = if best > 0.0 { *score / best } else { 1.0 };
            if history {
                *score = (*score * HISTORY_BUCKETS).ceil() / HISTORY_BUCKETS;
            }
        }
    }
    let scores = if ranked {
        priors.for_query(query)
    } else {
        HashMap::new()
    };
    let prior = |index: usize| {
        scores
            .get(entries[index].key.as_str())
            .copied()
            .unwrap_or(0.0)
    };
    let mut keyed: Vec<(usize, f64, f64)> = scored
        .into_iter()
        .map(|(index, score)| (index, score, prior(index)))
        .collect();
    let by = |a: f64, b: f64| b.total_cmp(&a);
    keyed.sort_by(|a, b| match ranking.prefer {
        Prefer::Blend => by(a.1 + BLEND * signed_log(a.2), b.1 + BLEND * signed_log(b.2)),
        Prefer::Match => by(a.1, b.1).then(by(a.2, b.2)),
        Prefer::Usage => by(a.2, b.2).then(by(a.1, b.1)),
    });
    keyed.into_iter().map(|(index, _, _)| index).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::KeyUsage;
    use serde_json::json;

    fn records(labels: &[&str]) -> Vec<Record> {
        labels.iter().map(|label| Record::new(*label)).collect()
    }

    fn labels<'a>(entries: &'a [Record], order: &[usize]) -> Vec<&'a str> {
        order
            .iter()
            .map(|index| entries[*index].label.as_str())
            .collect()
    }

    fn ranking(value: Value) -> Ranking {
        Ranking::from_json(&value).unwrap()
    }

    fn used(count: u64, picks: &[i64]) -> KeyUsage {
        KeyUsage {
            count,
            last_used: picks.iter().copied().max().unwrap_or(0),
            picks: picks.to_vec(),
        }
    }

    const DAY: i64 = 86_400;

    #[test]
    fn the_default_keeps_substring_filtering_and_the_provider_order() {
        let entries = records(&["beta", "alphabet", "Alpha"]);
        let default = Ranking::default();
        let priors = Priors::default();
        assert_eq!(
            labels(&entries, &order(&entries, "", &default, &[], &priors)).len(),
            3
        );
        assert_eq!(
            labels(&entries, &order(&entries, "alp", &default, &[], &priors)),
            ["alphabet", "Alpha"]
        );
        assert_eq!(
            labels(&entries, &order(&entries, "bet", &default, &[], &priors)),
            ["beta", "alphabet"]
        );
    }

    #[test]
    fn prefix_and_words_filter_without_reordering() {
        let entries = records(&["git log", "log git", "logrotate"]);
        let prefix = ranking(json!({ "match": { "kind": "prefix" } }));
        let words = ranking(json!({ "match": { "kind": "words" } }));
        let none = Priors::default();
        assert_eq!(
            labels(&entries, &order(&entries, "LOG", &prefix, &[], &none)),
            ["log git", "logrotate"]
        );
        assert_eq!(
            labels(&entries, &order(&entries, "git log", &words, &[], &none)),
            ["git log", "log git"]
        );
    }

    #[test]
    fn fuzzy_matching_ranks_tighter_matches_first() {
        let entries = records(&["firewall-config", "gnome-font-viewer", "firefox"]);
        let fuzzy = ranking(json!({ "match": { "kind": "fuzzy" } }));
        let found = order(&entries, "ffx", &fuzzy, &[], &Priors::default());
        assert_eq!(labels(&entries, &found), ["firefox"]);
        let found = order(&entries, "fire", &fuzzy, &[], &Priors::default());
        assert_eq!(labels(&entries, &found), ["firewall-config", "firefox"]);
    }

    #[test]
    fn fuzzy_syntax_and_words_across_fields() {
        let entries = vec![
            Record::new("Inbox").with_data(json!({ "from": "ann" })),
            Record::new("Invoice").with_data(json!({ "from": "bob" })),
        ];
        let fuzzy = ranking(json!({ "match": { "kind": "fuzzy" } }));
        let search = [
            DataPath::parse("label").unwrap(),
            DataPath::parse("from").unwrap(),
        ];
        let none = Priors::default();
        assert_eq!(
            labels(
                &entries,
                &order(&entries, "inv bob", &fuzzy, &search, &none)
            ),
            ["Invoice"]
        );
        assert_eq!(
            labels(
                &entries,
                &order(&entries, "in !ann", &fuzzy, &search, &none)
            ),
            ["Invoice"]
        );
        assert_eq!(
            labels(&entries, &order(&entries, "^box", &fuzzy, &search, &none)),
            Vec::<&str>::new()
        );
        let literal = ranking(json!({ "match": { "kind": "fuzzy", "syntax": false } }));
        assert!(order(&entries, "!ann", &literal, &search, &none).is_empty());
    }

    #[test]
    fn the_history_scheme_keeps_the_provider_order_for_near_matches() {
        let entries = records(&["cargo test --workspace", "cargo test"]);
        let history = ranking(json!({ "match": { "kind": "fuzzy", "scheme": "history" } }));
        let found = order(&entries, "cargo test", &history, &[], &Priors::default());
        assert_eq!(
            labels(&entries, &found),
            ["cargo test --workspace", "cargo test"]
        );
    }

    #[test]
    fn frecency_orders_an_empty_query_and_decays() {
        let entries = records(&["a", "b", "c"]);
        let spec = ranking(json!({ "rank": [{ "kind": "frecency", "half_life": DAY }] }));
        let now = 100 * DAY;
        let mut usage = HashMap::new();
        let mut ns = NamespaceUsage::default();
        ns.keys.insert("b".into(), used(1, &[now]));
        ns.keys.insert(
            "c".into(),
            used(3, &[now - 10 * DAY, now - 10 * DAY, now - 10 * DAY]),
        );
        usage.insert("w".into(), ns);
        let priors = Priors::compute(&spec, "w", &usage, &HashMap::new(), now);
        assert_eq!(
            labels(&entries, &order(&entries, "", &spec, &[], &priors)),
            ["b", "c", "a"]
        );
    }

    #[test]
    fn recent_count_pinned_and_python_scores() {
        let entries = records(&["a", "b", "c", "d"]);
        let mut ns = NamespaceUsage::default();
        ns.keys.insert("a".into(), used(1, &[10]));
        ns.keys.insert("b".into(), used(5, &[5]));
        let usage = HashMap::from([("apps".to_string(), ns)]);
        let recent = ranking(json!({ "rank": [{ "kind": "recent", "namespace": "apps" }] }));
        let priors = Priors::compute(&recent, "w", &usage, &HashMap::new(), 20);
        assert_eq!(
            labels(&entries, &order(&entries, "", &recent, &[], &priors)),
            ["a", "b", "c", "d"]
        );
        let count = ranking(json!({ "rank": [{ "kind": "count", "namespace": "apps" }] }));
        let priors = Priors::compute(&count, "w", &usage, &HashMap::new(), 20);
        assert_eq!(
            labels(&entries, &order(&entries, "", &count, &[], &priors)),
            ["b", "a", "c", "d"]
        );
        let pinned = ranking(json!({ "rank": [
            { "kind": "count", "namespace": "apps" },
            { "kind": "pinned", "keys": ["d"], "boost": 100.0 },
            { "kind": "python" },
        ] }));
        let python = HashMap::from([("c".to_string(), 50.0)]);
        let priors = Priors::compute(&pinned, "w", &usage, &python, 20);
        assert_eq!(
            labels(&entries, &order(&entries, "", &pinned, &[], &priors)),
            ["d", "c", "b", "a"]
        );
    }

    #[test]
    fn learned_picks_follow_the_typed_prefix() {
        let entries = records(&["firewall", "firefox"]);
        let spec = ranking(json!({
            "match": { "kind": "fuzzy" },
            "rank": [{ "kind": "learned", "prefix": 3, "half_life": DAY }],
        }));
        let mut ns = NamespaceUsage::default();
        ns.queries
            .push(("fi".into(), "firefox".into(), used(2, &[0, 0])));
        let usage = HashMap::from([("w".to_string(), ns)]);
        let priors = Priors::compute(&spec, "w", &usage, &HashMap::new(), 0);
        assert_eq!(
            labels(&entries, &order(&entries, "", &spec, &[], &priors)),
            ["firewall", "firefox"]
        );
        assert_eq!(
            labels(&entries, &order(&entries, "f", &spec, &[], &priors)),
            ["firefox", "firewall"]
        );
        assert_eq!(
            labels(&entries, &order(&entries, "fire", &spec, &[], &priors)),
            ["firefox", "firewall"]
        );
        assert_eq!(
            labels(&entries, &order(&entries, "w", &spec, &[], &priors)),
            ["firewall"]
        );
    }

    #[test]
    fn prefer_decides_between_match_and_usage() {
        let entries = records(&["fox", "f-o-x-tool"]);
        let with = |prefer: &str, picks: u64| {
            let mut ns = NamespaceUsage::default();
            ns.keys.insert("f-o-x-tool".into(), used(picks, &[0]));
            let usage = HashMap::from([("w".to_string(), ns)]);
            let spec = ranking(json!({
                "match": { "kind": "fuzzy" },
                "rank": [{ "kind": "count" }],
                "prefer": prefer,
            }));
            let priors = Priors::compute(&spec, "w", &usage, &HashMap::new(), 0);
            labels(&entries, &order(&entries, "fox", &spec, &[], &priors))
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        assert_eq!(with("match", 100), ["fox", "f-o-x-tool"]);
        assert_eq!(with("usage", 1), ["f-o-x-tool", "fox"]);
        assert_eq!(with("blend", 0), ["fox", "f-o-x-tool"]);
        assert_eq!(with("blend", 100), ["f-o-x-tool", "fox"]);
    }

    #[test]
    fn bad_rankings_are_rejected() {
        assert!(Ranking::from_json(&json!({ "match": { "kind": "regex" } })).is_err());
        assert!(
            Ranking::from_json(&json!({ "rank": [{ "kind": "frecency", "half_life": 0 }] }))
                .is_err()
        );
        assert!(Ranking::from_json(
            &json!({ "rank": [{ "kind": "learned", "prefix": 0, "half_life": 1 }] })
        )
        .is_err());
        assert!(Ranking::from_json(&json!({ "prefer": "sometimes" })).is_err());
        let spec = ranking(json!({ "rank": [
            { "kind": "frecency", "half_life": 1, "namespace": "apps" },
            { "kind": "count" },
            { "kind": "recent", "namespace": "apps" },
            { "kind": "pinned", "keys": [], "boost": 1 },
        ] }));
        assert!(spec.uses_usage());
        assert_eq!(spec.usage_namespaces("w"), ["apps", "w"]);
        assert!(!Ranking::default().uses_usage());
    }
}
