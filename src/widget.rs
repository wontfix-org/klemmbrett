//! Named widgets: a record source, a layout, a theme, and behavior.

use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::config::MultiSelect;
use crate::data::{display, DataPath};
use crate::executables::executables_on_path;
use crate::layout::LayoutSpec;
use crate::rank::Ranking;
use crate::secret::AgentSpec;
use crate::theme::{Location, Theme, ThemeSpec};
use crate::windows::WindowList;
use crate::HistoryStore;

#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub key: String,
    pub label: String,
    pub value: String,
    /// Nested JSON-like data for layouts, filters, and handlers. `null` when unused.
    pub data: Value,
}

impl Record {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        Self {
            key: value.clone(),
            label: value.clone(),
            value,
            data: Value::Null,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = data;
        self
    }

    /// The texts `path` names: the record's own string for `key`, `label`, or
    /// `value`, and otherwise every value it reaches inside `data`.
    pub fn texts(&self, path: &DataPath) -> Vec<String> {
        match path.record_field() {
            Some("key") => vec![self.key.clone()],
            Some("label") => vec![self.label.clone()],
            Some("value") => vec![self.value.clone()],
            _ => path.select(&self.data).into_iter().map(display).collect(),
        }
    }

    /// What `path` shows in a row; several values are joined with commas.
    pub fn text(&self, path: &DataPath) -> Option<String> {
        let texts = self.texts(path);
        (!texts.is_empty()).then(|| texts.join(", "))
    }

    /// `{"key", "label", "value", "data"}`, the shape Varlink callers see.
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("key".into(), Value::String(self.key.clone()));
        map.insert("label".into(), Value::String(self.label.clone()));
        map.insert("value".into(), Value::String(self.value.clone()));
        if !self.data.is_null() {
            map.insert("data".into(), self.data.clone());
        }
        Value::Object(map)
    }

    /// The Varlink record shape: `value` is required, `key` and `label`
    /// default to it, and `data` is optional. Other fields are rejected.
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let Value::Object(map) = value else {
            return Err("expected an object with a value".into());
        };
        let text = |name: &str| -> Result<Option<String>, String> {
            match map.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(text)) => Ok(Some(text.clone())),
                Some(other) => Err(format!("{name}: expected a string, got {other}")),
            }
        };
        if let Some(unknown) = map
            .keys()
            .find(|name| !matches!(name.as_str(), "key" | "label" | "value" | "data"))
        {
            return Err(format!(
                "unknown field {unknown:?}; use key, label, value, and data"
            ));
        }
        let value = text("value")?.ok_or_else(|| "value is required".to_string())?;
        Ok(Self {
            key: text("key")?.unwrap_or_else(|| value.clone()),
            label: text("label")?.unwrap_or_else(|| value.clone()),
            data: map.get("data").cloned().unwrap_or(Value::Null),
            value,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SerializedRecord {
    Value(String),
    Fields {
        value: String,
        #[serde(default)]
        key: Option<String>,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        data: Option<Value>,
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

impl TryFrom<SerializedRecord> for Record {
    type Error = String;

    /// Keys other than `key`, `label`, and `value` become `data`, unless
    /// `data` is given explicitly.
    fn try_from(record: SerializedRecord) -> Result<Self, String> {
        Ok(match record {
            SerializedRecord::Value(value) => Self::new(value),
            SerializedRecord::Fields {
                value,
                key,
                label,
                data,
                extra,
            } => {
                let data = match (data, extra.is_empty()) {
                    (Some(_), false) => {
                        let names: Vec<_> = extra.keys().cloned().collect();
                        return Err(format!(
                            "record {value:?} has both data and extra fields ({}); use one",
                            names.join(", ")
                        ));
                    }
                    (Some(data), true) => data,
                    (None, false) => Value::Object(extra),
                    (None, true) => Value::Null,
                };
                Self {
                    key: key.unwrap_or_else(|| value.clone()),
                    label: label.unwrap_or_else(|| value.clone()),
                    value,
                    data,
                }
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    Text,
    Json,
    Yaml,
}

impl FileFormat {
    pub fn parse(name: &str) -> Result<Self, String> {
        match name {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            "yaml" => Ok(Self::Yaml),
            other => Err(format!(
                "unknown format {other}; expected text, json, or yaml"
            )),
        }
    }

    /// `.json` and `.yaml`/`.yml` by extension, otherwise text.
    pub fn for_path(path: &Path) -> Self {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("json") => Self::Json,
            Some("yaml" | "yml") => Self::Yaml,
            _ => Self::Text,
        }
    }
}

/// Where a widget's rows come from.
///
/// `records` may block: it runs off the UI thread each time the widget opens.
pub trait RecordSource: Send + Sync {
    fn records(&self) -> Result<Vec<Record>, String>;

    /// The records with a rank score for each, for `rk.score`. Sources
    /// that compute none return an empty list.
    fn scored_records(&self) -> Result<(Vec<Record>, Vec<f64>), String> {
        Ok((self.records()?, Vec::new()))
    }

    /// Called with every new clipboard text.
    fn record_clipboard(&self, _text: &str) -> Result<(), String> {
        Ok(())
    }

    /// Called once when the daemon starts, before any `records`.
    fn start(&self) {}
}

pub struct ClipboardHistorySource {
    store: Mutex<HistoryStore>,
}

impl ClipboardHistorySource {
    pub fn open(
        path: &Path,
        limit: usize,
        coalesce_growing: bool,
        skip_whitespace_only: bool,
    ) -> Result<Self, String> {
        if limit == 0 {
            return Err("history limit must be greater than zero".into());
        }
        let store = HistoryStore::open(path, limit)
            .map_err(|err| format!("failed to open history database {}: {err}", path.display()))?
            .with_coalesce_growing(coalesce_growing)
            .with_skip_whitespace_only(skip_whitespace_only);
        Ok(Self {
            store: Mutex::new(store),
        })
    }

    fn store(&self) -> Result<std::sync::MutexGuard<'_, HistoryStore>, String> {
        self.store
            .lock()
            .map_err(|_| "history lock was poisoned".to_string())
    }
}

impl RecordSource for ClipboardHistorySource {
    /// `key` is the SQLite row id, so the same text can appear twice.
    fn records(&self) -> Result<Vec<Record>, String> {
        let rows = self
            .store()?
            .rows()
            .map_err(|err| format!("failed to read history: {err}"))?;
        Ok(rows
            .into_iter()
            .map(|row| Record {
                key: row.id.to_string(),
                label: row.text.clone(),
                value: row.text,
                data: Value::Null,
            })
            .collect())
    }

    fn record_clipboard(&self, text: &str) -> Result<(), String> {
        self.store()?
            .record(text)
            .map_err(|err| format!("failed to record history: {err}"))
    }
}

/// Reloaded every time the widget opens, so edits need no restart.
pub struct FileSource {
    path: PathBuf,
    format: FileFormat,
}

impl FileSource {
    /// Reads the file once so a missing or malformed file fails at startup.
    pub fn open(path: PathBuf, format: FileFormat) -> Result<Self, String> {
        let source = Self { path, format };
        let records = source.records()?;
        validate_records(&source.path.display().to_string(), &records)?;
        Ok(source)
    }
}

impl RecordSource for FileSource {
    fn records(&self) -> Result<Vec<Record>, String> {
        let text = std::fs::read_to_string(&self.path)
            .map_err(|err| format!("failed to read {}: {err}", self.path.display()))?;
        parse_records(&text, self.format).map_err(|err| format!("{}: {err}", self.path.display()))
    }
}

/// Executables on `PATH`; the first basename wins.
pub struct ExecutablesSource;

impl RecordSource for ExecutablesSource {
    fn records(&self) -> Result<Vec<Record>, String> {
        Ok(executables_on_path(
            &std::env::var("PATH").unwrap_or_default(),
        ))
    }
}

/// Open toplevels via `zwlr_foreign_toplevel_manager_v1`. `value` is the key
/// that `activate_window` accepts.
pub struct WaylandWindowsSource;

impl RecordSource for WaylandWindowsSource {
    fn records(&self) -> Result<Vec<Record>, String> {
        WindowList::shared().records()
    }

    fn start(&self) {
        WindowList::shared();
    }
}

/// Fixed records, mostly for tests and on-the-fly widgets.
pub struct StaticSource(pub Vec<Record>);

impl RecordSource for StaticSource {
    fn records(&self) -> Result<Vec<Record>, String> {
        Ok(self.0.clone())
    }
}

/// What a widget does with rows. Calls come from the UI thread and must not block.
pub trait Handlers: Send + Sync {
    /// The rows a user picked, in the order they were marked.
    fn select(&self, widget: &str, records: Vec<Record>);
    /// The keyboard selection moved to a different row.
    fn highlight(&self, widget: &str, record: Record);
    /// New clipboard text, for sources that are not native.
    fn clipboard(&self, _text: &str) {}
    /// A notification arrived. `true` means the handlers will decide whether
    /// it is shown; `false` shows it right away.
    fn notification(&self, _notification: &crate::notifications::Notification) -> bool {
        false
    }
    /// An RPC method the configuration answers, such as `StashList`. Blocks
    /// until the answer is ready, so it must not be called from the UI thread.
    fn call(&self, method: &str, _parameters: Value) -> Result<Value, CallError> {
        Err(CallError::failed(format!("{method} is not available")))
    }
    /// Which widget shows a secret question of the agent `widget`, from the
    /// question's metadata (never its answer). `None` keeps the agent's own.
    /// Blocks, so it must not be called from the UI thread.
    fn route_secret(&self, _widget: &str, _question: Value) -> Option<String> {
        None
    }
}

/// A Varlink error: the unqualified error name and its parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct CallError {
    pub error: String,
    pub parameters: Value,
}

impl CallError {
    pub fn new(error: impl Into<String>, parameters: Value) -> Self {
        Self {
            error: error.into(),
            parameters,
        }
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self::new("Failed", serde_json::json!({ "message": message.into() }))
    }
}

/// Drops every event. Used when nothing is configured.
pub struct NoHandlers;

impl Handlers for NoHandlers {
    fn select(&self, _widget: &str, _records: Vec<Record>) {}
    fn highlight(&self, _widget: &str, _record: Record) {}
}

#[derive(Clone)]
pub struct WidgetDef {
    pub name: String,
    /// `None` for a widget that only accepts records passed to `Show` or `Prompt`.
    pub source: Option<Arc<dyn RecordSource>>,
    pub layout: LayoutSpec,
    /// Replaces the file-wide palette for this widget only.
    pub theme: Option<ThemeSpec>,
    pub location: Option<Location>,
    pub multi: Option<MultiSelect>,
    /// Whether highlight events are worth sending to the handlers.
    pub highlight: bool,
    /// What the filter matches. Empty means the label.
    pub search: Vec<DataPath>,
    /// How typing matches rows and how past picks order them.
    pub ranking: Ranking,
    /// Set when the widget answers secret questions instead of showing rows.
    pub agent: Option<AgentSpec>,
}

impl WidgetDef {
    pub fn new(name: impl Into<String>, source: Option<Arc<dyn RecordSource>>) -> Self {
        Self {
            name: name.into(),
            source,
            layout: LayoutSpec::Default,
            theme: None,
            location: None,
            multi: None,
            highlight: false,
            search: Vec::new(),
            ranking: Ranking::default(),
            agent: None,
        }
    }
}

pub struct Widgets {
    defs: Vec<WidgetDef>,
    themes: Vec<Theme>,
    palette: ThemeSpec,
    overrides: ThemeSpec,
    default_theme: Theme,
}

impl Widgets {
    /// `palette` is the file-wide theme, `overrides` is applied over every
    /// widget's palette and survives `set_palette`.
    pub fn new(
        defs: Vec<WidgetDef>,
        palette: ThemeSpec,
        overrides: ThemeSpec,
    ) -> Result<Self, String> {
        let mut names = HashSet::new();
        for def in &defs {
            if def.name.trim().is_empty() {
                return Err("widget name must not be empty".into());
            }
            if !names.insert(def.name.as_str()) {
                return Err(format!(
                    "widget name {:?} is configured more than once",
                    def.name
                ));
            }
        }
        let mut widgets = Self {
            defs,
            themes: Vec::new(),
            palette,
            overrides,
            default_theme: Theme::default(),
        };
        widgets.restyle();
        Ok(widgets)
    }

    pub fn empty() -> Self {
        Self::new(Vec::new(), ThemeSpec::default(), ThemeSpec::default()).expect("no widgets")
    }

    fn restyle(&mut self) {
        self.default_theme = Theme::resolve(&[&self.palette, &self.overrides]);
        self.themes = self
            .defs
            .iter()
            .map(|def| match &def.theme {
                Some(own) => Theme::resolve(&[own, &self.overrides]),
                None => self.default_theme.clone(),
            })
            .collect();
    }

    /// Replaces the file-wide palette. Widgets with their own theme keep it.
    pub fn set_palette(&mut self, palette: ThemeSpec) -> &Theme {
        self.palette = palette;
        self.restyle();
        &self.default_theme
    }

    pub fn default_theme(&self) -> &Theme {
        &self.default_theme
    }

    pub fn get(&self, name: &str) -> Option<&WidgetDef> {
        self.defs.iter().find(|def| def.name == name)
    }

    pub fn theme(&self, name: &str) -> Option<&Theme> {
        self.defs
            .iter()
            .position(|def| def.name == name)
            .map(|index| &self.themes[index])
    }

    pub fn names(&self) -> Vec<String> {
        self.defs.iter().map(|def| def.name.clone()).collect()
    }

    pub fn defs(&self) -> &[WidgetDef] {
        &self.defs
    }

    /// Whether any widget ranks by past picks, so they have to be recorded.
    pub fn uses_usage(&self) -> bool {
        self.defs.iter().any(|def| def.ranking.uses_usage())
    }

    /// Widgets the tray can open. On-demand widgets need records from the caller.
    pub fn names_with_provider(&self) -> Vec<String> {
        self.defs
            .iter()
            .filter(|def| def.source.is_some())
            .map(|def| def.name.clone())
            .collect()
    }

    pub fn names_without_provider(&self) -> Vec<String> {
        self.defs
            .iter()
            .filter(|def| def.source.is_none())
            .map(|def| def.name.clone())
            .collect()
    }

    pub fn requires_records(&self, name: &str) -> bool {
        self.get(name).is_some_and(|def| def.source.is_none())
    }

    pub fn multi(&self, name: &str) -> Option<&MultiSelect> {
        self.get(name).and_then(|def| def.multi.as_ref())
    }

    pub fn records(&self, name: &str) -> Result<Vec<Record>, String> {
        let def = self
            .get(name)
            .ok_or_else(|| format!("unknown widget {name:?}"))?;
        fetch_records(def)
    }

    /// Starts every source once, even when several widgets share it.
    pub fn start(&self) {
        for source in self.unique_sources() {
            source.start();
        }
    }

    /// Records clipboard text in every source once, even when several widgets share it.
    pub fn record_history(&self, text: &str) -> Result<(), String> {
        for source in self.unique_sources() {
            source.record_clipboard(text)?;
        }
        Ok(())
    }

    fn unique_sources(&self) -> Vec<&Arc<dyn RecordSource>> {
        let mut seen = HashSet::new();
        self.defs
            .iter()
            .filter_map(|def| def.source.as_ref())
            .filter(|source| seen.insert(Arc::as_ptr(source) as *const () as usize))
            .collect()
    }
}

/// Loads and validates a widget's rows. Errors name the widget.
pub fn fetch_records(def: &WidgetDef) -> Result<Vec<Record>, String> {
    let Some(source) = &def.source else {
        return Err(format!(
            "widget {:?} has no provider; pass records",
            def.name
        ));
    };
    let records = source
        .records()
        .map_err(|err| format!("widget {:?}: {err}", def.name))?;
    validate_records(&def.name, &records)?;
    Ok(records)
}

/// The widget's records and the rank scores its source computed, by key.
pub fn fetch_scored(def: &WidgetDef) -> Result<(Vec<Record>, HashMap<String, f64>), String> {
    let Some(source) = &def.source else {
        return Err(format!(
            "widget {:?} has no provider; pass records",
            def.name
        ));
    };
    let (records, scores) = source
        .scored_records()
        .map_err(|err| format!("widget {:?}: {err}", def.name))?;
    validate_records(&def.name, &records)?;
    if !scores.is_empty() && scores.len() != records.len() {
        return Err(format!(
            "widget {:?}: {} scores for {} records",
            def.name,
            scores.len(),
            records.len()
        ));
    }
    let scores = records
        .iter()
        .zip(scores)
        .map(|(record, score)| (record.key.clone(), score))
        .collect();
    Ok((records, scores))
}

/// Decodes text, JSON, or YAML the way a `File` provider does.
///
/// JSON and YAML are an array of strings or objects. A string is the value.
/// An object requires `value`; omitted `key` and `label` become that value.
/// Text is one non-empty line per value.
pub fn parse_records(text: &str, format: FileFormat) -> Result<Vec<Record>, String> {
    match format {
        FileFormat::Text => Ok(text
            .lines()
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .filter(|line| !line.is_empty())
            .map(Record::new)
            .collect()),
        FileFormat::Json => {
            let records: Vec<SerializedRecord> =
                serde_json::from_str(text).map_err(|err| format!("invalid JSON: {err}"))?;
            records.into_iter().map(Record::try_from).collect()
        }
        FileFormat::Yaml => {
            let records: Vec<SerializedRecord> =
                serde_yaml::from_str(text).map_err(|err| format!("invalid YAML: {err}"))?;
            records.into_iter().map(Record::try_from).collect()
        }
    }
}

/// Decodes records a caller passed, naming the widget and row on errors.
/// Duplicate or empty keys are rejected.
pub fn records_from_json(widget: &str, rows: &[Value]) -> Result<Vec<Record>, String> {
    let records = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            Record::from_json(row)
                .map_err(|err| format!("widget {widget:?}: record {}: {err}", index + 1))
        })
        .collect::<Result<Vec<_>, _>>()?;
    validate_records(widget, &records)?;
    Ok(records)
}

pub fn validate_records(widget_name: &str, records: &[Record]) -> Result<(), String> {
    let mut keys = HashSet::new();
    for (index, record) in records.iter().enumerate() {
        if record.key.is_empty() {
            return Err(format!(
                "widget {widget_name:?}: record {} has an empty key",
                index + 1
            ));
        }
        if !keys.insert(&record.key) {
            return Err(format!(
                "widget {widget_name:?}: duplicate record key {:?}",
                record.key
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Props, Rgba};
    use serde_json::json;

    fn file_widget(format: FileFormat, contents: &str) -> (tempfile::TempDir, Widgets) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("records");
        std::fs::write(&path, contents).unwrap();
        let source = FileSource::open(path, format).unwrap();
        let widgets = Widgets::new(
            vec![WidgetDef::new("items", Some(Arc::new(source)))],
            ThemeSpec::default(),
            ThemeSpec::default(),
        )
        .unwrap();
        (dir, widgets)
    }

    #[test]
    fn text_files_are_one_value_per_nonempty_line() {
        let (_dir, widgets) = file_widget(FileFormat::Text, "one\n\ntwo\n");
        assert_eq!(
            widgets.records("items").unwrap(),
            vec![Record::new("one"), Record::new("two")]
        );
    }

    #[test]
    fn file_widgets_are_reloaded() {
        let (dir, widgets) = file_widget(FileFormat::Text, "old\n");
        std::fs::write(dir.path().join("records"), "new\n").unwrap();
        assert_eq!(widgets.records("items").unwrap(), vec![Record::new("new")]);
    }

    #[test]
    fn a_broken_file_fails_when_opened() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("records.json");
        std::fs::write(
            &path,
            r#"[{"key":"same","value":"one"},{"key":"same","value":"two"}]"#,
        )
        .unwrap();
        let error = FileSource::open(path.clone(), FileFormat::Json)
            .err()
            .unwrap();
        assert!(error.contains("duplicate record key"), "{error}");
        let error = FileSource::open(dir.path().join("missing"), FileFormat::Text)
            .err()
            .unwrap();
        assert!(error.contains("failed to read"), "{error}");
    }

    #[test]
    fn format_follows_the_extension() {
        assert_eq!(FileFormat::for_path(Path::new("a.json")), FileFormat::Json);
        assert_eq!(FileFormat::for_path(Path::new("a.yml")), FileFormat::Yaml);
        assert_eq!(FileFormat::for_path(Path::new("a.yaml")), FileFormat::Yaml);
        assert_eq!(FileFormat::for_path(Path::new("a.txt")), FileFormat::Text);
    }

    #[test]
    fn history_records_once_per_shared_source() {
        let dir = tempfile::tempdir().unwrap();
        let source: Arc<dyn RecordSource> = Arc::new(
            ClipboardHistorySource::open(&dir.path().join("h.sqlite"), 10, true, true).unwrap(),
        );
        let widgets = Widgets::new(
            vec![
                WidgetDef::new("a", Some(source.clone())),
                WidgetDef::new("b", Some(source)),
            ],
            ThemeSpec::default(),
            ThemeSpec::default(),
        )
        .unwrap();
        widgets.record_history("one").unwrap();
        widgets.record_history("two").unwrap();
        let keys: Vec<_> = widgets
            .records("a")
            .unwrap()
            .into_iter()
            .map(|record| record.value)
            .collect();
        assert_eq!(keys, vec!["two", "one"]);
    }

    #[test]
    fn whitespace_history_respects_the_setting() {
        let dir = tempfile::tempdir().unwrap();
        let source =
            ClipboardHistorySource::open(&dir.path().join("h.sqlite"), 10, true, false).unwrap();
        source.record_clipboard("   \n").unwrap();
        assert_eq!(
            source.records().unwrap(),
            vec![Record {
                key: "1".into(),
                label: "   \n".into(),
                value: "   \n".into(),
                data: Value::Null,
            }]
        );
    }

    #[test]
    fn widgets_without_a_source_need_records() {
        let widgets = Widgets::new(
            vec![
                WidgetDef::new("pick", None),
                WidgetDef::new(
                    "items",
                    Some(Arc::new(StaticSource(vec![Record::new("x")]))),
                ),
            ],
            ThemeSpec::default(),
            ThemeSpec::default(),
        )
        .unwrap();
        assert_eq!(widgets.names_without_provider(), vec!["pick".to_string()]);
        assert_eq!(widgets.names_with_provider(), vec!["items".to_string()]);
        assert!(widgets.requires_records("pick"));
        assert!(!widgets.requires_records("items"));
        let error = widgets.records("pick").unwrap_err();
        assert!(error.contains("has no provider"), "{error}");
    }

    #[test]
    fn duplicate_and_empty_names_are_rejected() {
        let error = Widgets::new(
            vec![WidgetDef::new("same", None), WidgetDef::new("same", None)],
            ThemeSpec::default(),
            ThemeSpec::default(),
        )
        .err()
        .unwrap();
        assert!(error.contains("more than once"), "{error}");
        let error = Widgets::new(
            vec![WidgetDef::new(" ", None)],
            ThemeSpec::default(),
            ThemeSpec::default(),
        )
        .err()
        .unwrap();
        assert!(error.contains("must not be empty"), "{error}");
    }

    #[test]
    fn set_palette_keeps_widget_themes_and_overrides() {
        let palette = |rgb| ThemeSpec {
            global: Props {
                background: Some(Rgba::hex(rgb)),
                ..Props::default()
            },
            ..ThemeSpec::default()
        };
        let overrides = ThemeSpec {
            global: Props {
                text_color: Some(Rgba::hex(0x010101)),
                ..Props::default()
            },
            ..ThemeSpec::default()
        };
        let mut pinned = WidgetDef::new("pinned", None);
        pinned.theme = Some(palette(0x282a36));
        let mut widgets = Widgets::new(
            vec![WidgetDef::new("pick", None), pinned],
            palette(0x2e3440),
            overrides,
        )
        .unwrap();
        let theme = widgets.set_palette(palette(0x282828)).clone();
        assert_eq!(theme.window().background, Rgba::hex(0x282828));
        let pick = widgets.theme("pick").unwrap();
        assert_eq!(pick.window().background, Rgba::hex(0x282828));
        assert_eq!(pick.window().text_color, Rgba::hex(0x010101));
        let pinned = widgets.theme("pinned").unwrap();
        assert_eq!(pinned.window().background, Rgba::hex(0x282a36));
        assert_eq!(pinned.window().text_color, Rgba::hex(0x010101));
    }

    #[test]
    fn parse_records_shares_text_json_and_yaml_rules() {
        assert_eq!(
            parse_records("one\n\ntwo\n", FileFormat::Text).unwrap(),
            vec![Record::new("one"), Record::new("two")]
        );
        assert_eq!(
            parse_records(
                r#"["plain", {"label":"Readable", "value":"copied"}]"#,
                FileFormat::Json,
            )
            .unwrap(),
            vec![
                Record::new("plain"),
                Record {
                    key: "copied".into(),
                    label: "Readable".into(),
                    value: "copied".into(),
                    data: Value::Null,
                },
            ]
        );
        assert_eq!(
            parse_records("- value: copied\n", FileFormat::Yaml).unwrap(),
            vec![Record::new("copied")]
        );
    }

    #[test]
    fn extra_keys_or_data_become_nested_data() {
        assert_eq!(
            parse_records(
                r#"[{"value":"v", "sender":{"name":"Ada"}, "tags":["a"]}]"#,
                FileFormat::Json
            )
            .unwrap(),
            vec![Record::new("v").with_data(json!({"sender": {"name": "Ada"}, "tags": ["a"]}))]
        );
        assert_eq!(
            parse_records("- value: v\n  data: [1, 2]\n", FileFormat::Yaml).unwrap(),
            vec![Record::new("v").with_data(json!([1, 2]))]
        );
        let error =
            parse_records(r#"[{"value":"v", "data":1, "extra":2}]"#, FileFormat::Json).unwrap_err();
        assert!(
            error.contains("both data and extra fields (extra)"),
            "{error}"
        );
    }

    #[test]
    fn json_records_default_missing_fields_and_reject_bad_keys() {
        let record = Record::new("copied").with_data(json!({"n": 1}));
        assert_eq!(Record::from_json(&record.to_json()).unwrap(), record);
        assert_eq!(
            records_from_json("items", &[json!({"value": "copied"})]).unwrap(),
            vec![Record::new("copied")]
        );
        let error = records_from_json("items", &[json!({"key": "id"})]).unwrap_err();
        assert!(error.contains("record 1: value is required"), "{error}");
        let error = records_from_json("items", &[json!({"value": "v", "extra": 1})]).unwrap_err();
        assert!(error.contains("unknown field \"extra\""), "{error}");
        let error = records_from_json("items", &[json!({"value": 3})]).unwrap_err();
        assert!(error.contains("value: expected a string"), "{error}");
        let error = records_from_json(
            "items",
            &[
                json!({"key": "same", "value": "one"}),
                json!({"key": "same", "value": "two"}),
            ],
        )
        .unwrap_err();
        assert!(error.contains("duplicate record key"), "{error}");
    }

    #[test]
    fn record_texts_name_own_fields_or_data_paths() {
        let record = Record::new("v").with_data(json!({"tags": ["a", "b"], "n": 2}));
        let path = |text| DataPath::parse(text).unwrap();
        assert_eq!(record.text(&path("value")).as_deref(), Some("v"));
        assert_eq!(record.text(&path("tags[*]")).as_deref(), Some("a, b"));
        assert_eq!(record.text(&path("n")).as_deref(), Some("2"));
        assert_eq!(record.text(&path("missing")), None);
    }
}
