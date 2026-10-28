//! Named widgets that provide records for popups.

use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use crate::config::{Config, FileFormat, MultiSelect, SelectAction, Transform, WidgetConfig};
use crate::executables::executables_on_path;
use crate::theme::Presentation;
use crate::windows::WindowList;
use crate::{default_db_path, HistoryStore};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub key: String,
    pub label: String,
    pub value: String,
}

impl Record {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        Self {
            key: value.clone(),
            label: value.clone(),
            value,
        }
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
    },
}

impl From<SerializedRecord> for Record {
    fn from(record: SerializedRecord) -> Self {
        match record {
            SerializedRecord::Value(value) => Self::new(value),
            SerializedRecord::Fields { value, key, label } => Self {
                key: key.unwrap_or_else(|| value.clone()),
                label: label.unwrap_or_else(|| value.clone()),
                value,
            },
        }
    }
}

pub struct Widgets {
    entries: Vec<NamedWidget>,
    windows: Option<WindowList>,
}

struct NamedWidget {
    name: String,
    /// `None` for a widget that only accepts records passed to `Show` or `Prompt`.
    provider: Option<Provider>,
    presentation: Presentation,
    actions: Vec<SelectAction>,
    transforms: Vec<Transform>,
    multi: Option<MultiSelect>,
}

enum Provider {
    ClipboardHistory(HistoryStore),
    File(FileSource),
    Executables,
    WaylandWindows,
}

struct FileSource {
    path: PathBuf,
    format: FileFormat,
}

impl Widgets {
    pub fn from_config(config: &Config) -> Result<Self, String> {
        let windows = config
            .widgets
            .iter()
            .any(|widget| matches!(widget, WidgetConfig::WaylandWindows { .. }))
            .then(WindowList::start);
        let mut entries = Vec::with_capacity(config.widgets.len());
        for widget in &config.widgets {
            let name = widget.name().to_owned();
            let presentation = widget.presentation().clone();
            let actions = widget.actions().to_vec();
            let transforms = widget.transforms().to_vec();
            let multi = widget.multi().cloned();
            let provider = match widget {
                WidgetConfig::ClipboardHistory {
                    db,
                    history_limit,
                    coalesce_growing,
                    skip_whitespace_only,
                    ..
                } => {
                    let path = db.clone().unwrap_or_else(default_db_path);
                    let store = HistoryStore::open(&path, *history_limit)
                        .map_err(|err| {
                            format!(
                                "widget {name:?}: failed to open history database {}: {err}",
                                path.display()
                            )
                        })?
                        .with_coalesce_growing(*coalesce_growing)
                        .with_skip_whitespace_only(*skip_whitespace_only);
                    Some(Provider::ClipboardHistory(store))
                }
                WidgetConfig::File { path, format, .. } => Some(Provider::File(FileSource {
                    path: path.clone(),
                    format: *format,
                })),
                WidgetConfig::Executables { .. } => Some(Provider::Executables),
                WidgetConfig::WaylandWindows { .. } => Some(Provider::WaylandWindows),
                WidgetConfig::OnDemand { .. } => None,
            };
            entries.push(NamedWidget {
                name,
                provider,
                presentation,
                actions,
                transforms,
                multi,
            });
        }

        let widgets = Self { entries, windows };
        // Validate file readability, format, and keys at startup. Files are
        // still reloaded when opened so changes do not require a restart.
        for entry in &widgets.entries {
            if matches!(entry.provider, Some(Provider::File(_))) {
                widgets.records(&entry.name)?;
            }
        }
        Ok(widgets)
    }

    pub fn names(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|entry| entry.name.clone())
            .collect()
    }

    /// Widgets the tray can open. On-demand widgets need records from the caller.
    pub fn names_with_provider(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|entry| entry.provider.is_some())
            .map(|entry| entry.name.clone())
            .collect()
    }

    pub fn names_without_provider(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|entry| entry.provider.is_none())
            .map(|entry| entry.name.clone())
            .collect()
    }

    pub fn requires_records(&self, name: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.name == name && entry.provider.is_none())
    }

    pub fn presentation(&self, name: &str) -> Option<&Presentation> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| &entry.presentation)
    }

    pub fn actions(&self, name: &str) -> Option<&[SelectAction]> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.actions.as_slice())
    }

    pub fn transforms(&self, name: &str) -> Option<&[Transform]> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.transforms.as_slice())
    }

    pub fn multi(&self, name: &str) -> Option<&MultiSelect> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .and_then(|entry| entry.multi.as_ref())
    }

    pub fn records(&self, name: &str) -> Result<Vec<Record>, String> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| format!("unknown widget {name:?}"))?;
        let Some(provider) = &entry.provider else {
            return Err(format!("widget {name:?} has no provider; pass records"));
        };
        let records = match provider {
            Provider::ClipboardHistory(store) => store
                .entries()
                .map(|entries| entries.into_iter().map(Record::new).collect())
                .map_err(|err| format!("failed to read history: {err}"))?,
            Provider::File(file) => file.records()?,
            Provider::Executables => {
                executables_on_path(&std::env::var("PATH").unwrap_or_default())
            }
            Provider::WaylandWindows => self
                .windows
                .as_ref()
                .ok_or_else(|| "wayland window list is not running".to_string())?
                .records()?,
        };
        validate_records(name, &records)?;
        Ok(records)
    }

    pub fn activate_window(&self, key: &str) -> Result<(), String> {
        self.windows
            .as_ref()
            .ok_or_else(|| "no wayland-windows widget is configured".to_string())?
            .activate(key)
    }

    pub fn record_history(&mut self, text: &str) -> Result<(), String> {
        for entry in &mut self.entries {
            if let Some(Provider::ClipboardHistory(store)) = &mut entry.provider {
                store.record(text).map_err(|err| {
                    format!("widget {:?}: failed to record history: {err}", entry.name)
                })?;
            }
        }
        Ok(())
    }
}

impl FileSource {
    fn records(&self) -> Result<Vec<Record>, String> {
        let text = std::fs::read_to_string(&self.path)
            .map_err(|err| format!("failed to read {}: {err}", self.path.display()))?;
        parse_records(&text, self.format).map_err(|err| format!("{}: {err}", self.path.display()))
    }
}

/// Decodes text, JSON, or YAML the way a `file` widget does.
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
            let records =
                serde_json::from_str(text).map_err(|err| format!("invalid JSON: {err}"))?;
            Ok(deserialize_records(records))
        }
        FileFormat::Yaml => {
            let records =
                serde_yaml::from_str(text).map_err(|err| format!("invalid YAML: {err}"))?;
            Ok(deserialize_records(records))
        }
    }
}

/// An empty list means the caller did not supply records. A non-empty list
/// uses the same field defaults as [`parse_records`] and rejects duplicate
/// or empty keys.
pub(crate) fn records_from_maps(
    widget: &str,
    rows: Vec<BTreeMap<String, String>>,
) -> Result<Option<Vec<Record>>, String> {
    if rows.is_empty() {
        return Ok(None);
    }
    let mut records = Vec::with_capacity(rows.len());
    for (index, mut row) in rows.into_iter().enumerate() {
        let Some(value) = row.remove("value") else {
            return Err(format!(
                "widget {widget:?}: record {}: value is required",
                index + 1
            ));
        };
        let key = row.remove("key");
        let label = row.remove("label");
        records.push(Record {
            key: key.unwrap_or_else(|| value.clone()),
            label: label.unwrap_or_else(|| value.clone()),
            value,
        });
    }
    validate_records(widget, &records)?;
    Ok(Some(records))
}

fn deserialize_records(records: Vec<SerializedRecord>) -> Vec<Record> {
    records.into_iter().map(Record::from).collect()
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
    use crate::config::parse_config;

    fn file_source(format: &str, contents: &str) -> (tempfile::TempDir, Widgets) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("records"), contents).unwrap();
        let config = parse_config(
            &format!(
                r#"
                    widget "items" "file" {{
                        path   = "records"
                        format = "{format}"
                    }}
                "#
            ),
            dir.path(),
        )
        .unwrap();
        let widgets = Widgets::from_config(&config).unwrap();
        (dir, widgets)
    }

    #[test]
    fn text_files_are_one_value_per_nonempty_line() {
        let (_dir, widgets) = file_source("text", "one\n\ntwo\n");
        assert_eq!(
            widgets.records("items").unwrap(),
            vec![Record::new("one"), Record::new("two")]
        );
    }

    #[test]
    fn json_supports_strings_and_full_records() {
        let (_dir, widgets) = file_source(
            "json",
            r#"["plain", {"key":"id", "label":"Readable", "value":"copied"}]"#,
        );
        assert_eq!(
            widgets.records("items").unwrap(),
            vec![
                Record::new("plain"),
                Record {
                    key: "id".into(),
                    label: "Readable".into(),
                    value: "copied".into(),
                }
            ]
        );
    }

    #[test]
    fn yaml_defaults_key_and_label_to_value() {
        let (_dir, widgets) = file_source("yaml", "- value: copied\n");
        assert_eq!(
            widgets.records("items").unwrap(),
            vec![Record::new("copied")]
        );
    }

    #[test]
    fn duplicate_keys_are_rejected_with_widget_context() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("records"),
            r#"[{"key":"same","value":"one"},{"key":"same","value":"two"}]"#,
        )
        .unwrap();
        let config = parse_config(
            r#"
                widget "items" "file" {
                    path   = "records"
                    format = "json"
                }
            "#,
            dir.path(),
        )
        .unwrap();
        let error = Widgets::from_config(&config).err().unwrap();
        assert!(error.contains("widget \"items\""));
        assert!(error.contains("duplicate record key"));
    }

    #[test]
    fn file_widgets_are_reloaded() {
        let (dir, widgets) = file_source("text", "old\n");
        std::fs::write(dir.path().join("records"), "new\n").unwrap();
        assert_eq!(widgets.records("items").unwrap(), vec![Record::new("new")]);
    }

    #[test]
    fn whitespace_history_respects_widget_config() {
        let dir = tempfile::tempdir().unwrap();
        let config = parse_config(
            r#"
                widget "history" "clipboard-history" {
                    db                   = "history.sqlite"
                    skip_whitespace_only = false
                }
            "#,
            dir.path(),
        )
        .unwrap();
        let mut widgets = Widgets::from_config(&config).unwrap();
        widgets.record_history("   \n").unwrap();
        assert_eq!(
            widgets.records("history").unwrap(),
            vec![Record::new("   \n")]
        );
    }

    #[test]
    fn executables_widget_uses_the_process_path() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("kb-path-test-bin");
        std::fs::write(&bin, "#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let config = parse_config(r#"widget "run" "executables" {}"#, dir.path()).unwrap();
        let previous = std::env::var_os("PATH");
        std::env::set_var("PATH", dir.path());
        let widgets = Widgets::from_config(&config).unwrap();
        let records = widgets.records("run").unwrap();
        match previous {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
        assert_eq!(
            widgets.actions("run"),
            Some(
                [SelectAction::Launch {
                    command: vec!["${value}".into()],
                    args: Default::default(),
                }]
                .as_slice()
            )
        );
        assert!(
            records
                .iter()
                .any(|record| record.key == "kb-path-test-bin"),
            "{records:?}"
        );
    }

    #[test]
    fn configured_transforms_are_available_by_widget_name() {
        let dir = tempfile::tempdir().unwrap();
        let config = parse_config(
            r#"
                transform "jq" "exec" {
                    args {
                        expr = ".title"
                    }
                    command = ["jq", "-r", "${expr}"]
                }
                widget "items" "clipboard-history" {
                    db       = "history.sqlite"
                    transform "jq" {
                        expr = ".data"
                    }
                }
            "#,
            dir.path(),
        )
        .unwrap();
        let widgets = Widgets::from_config(&config).unwrap();
        assert_eq!(
            widgets.transforms("items"),
            Some(
                [Transform::Exec {
                    command: vec!["jq".into(), "-r".into(), "${expr}".into()],
                    stdin: "${value}".into(),
                    args: std::collections::BTreeMap::from([("expr".into(), ".data".into())]),
                }]
                .as_slice()
            )
        );
    }

    #[test]
    fn configured_action_args_are_available_by_widget_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("records"), "one\n").unwrap();
        let config = parse_config(
            r#"
                action "notify" "exec" {
                    args {
                        title = "klemmbrett"
                    }
                    command = ["notify-send", "${title}", "${value}"]
                }
                widget "items" "file" {
                    path     = "records"
                    format   = "text"
                    action "notify" {
                        title = "${label}"
                    }
                }
            "#,
            dir.path(),
        )
        .unwrap();
        let widgets = Widgets::from_config(&config).unwrap();
        assert_eq!(
            widgets
                .actions("items")
                .and_then(|actions| actions.first()?.argv("one", "Readable", "payload")),
            Some(vec![
                "notify-send".into(),
                "Readable".into(),
                "payload".into()
            ])
        );
    }

    #[test]
    fn configured_action_is_available_by_widget_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("records"), "one\n").unwrap();
        let config = parse_config(
            r#"
                widget "items" "file" {
                    path   = "records"
                    format = "text"
                    action = "copy-to-clipboard"
                }
            "#,
            dir.path(),
        )
        .unwrap();
        let widgets = Widgets::from_config(&config).unwrap();
        assert_eq!(
            widgets.actions("items"),
            Some([SelectAction::CopyToClipboard].as_slice())
        );
    }

    #[test]
    fn xdg_open_action_is_available_by_widget_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("records"), "https://example.com\n").unwrap();
        let config = parse_config(
            r#"
                widget "links" "file" {
                    path   = "records"
                    format = "text"
                    action = "xdg-open"
                }
            "#,
            dir.path(),
        )
        .unwrap();
        let widgets = Widgets::from_config(&config).unwrap();
        assert_eq!(
            widgets.actions("links"),
            Some(
                [SelectAction::Launch {
                    command: vec!["xdg-open".into(), "${value}".into()],
                    args: Default::default(),
                }]
                .as_slice()
            )
        );
    }

    #[test]
    fn widget_without_a_provider_rejects_its_own_records() {
        let dir = tempfile::tempdir().unwrap();
        let config = parse_config(
            r#"
                widget "pick" {
                    action = "xdg-open"
                }
                widget "items" "file" {
                    path   = "records"
                    format = "text"
                }
            "#,
            dir.path(),
        )
        .unwrap();
        std::fs::write(dir.path().join("records"), "one\n").unwrap();
        let widgets = Widgets::from_config(&config).unwrap();
        assert_eq!(widgets.names_without_provider(), vec!["pick".to_string()]);
        assert_eq!(widgets.names_with_provider(), vec!["items".to_string()]);
        assert!(widgets.requires_records("pick"));
        assert!(!widgets.requires_records("items"));
        let error = widgets.records("pick").unwrap_err();
        assert!(error.contains("has no provider"), "{error}");
        assert!(error.contains("pass records"), "{error}");
        assert_eq!(
            widgets.actions("pick"),
            Some(
                [SelectAction::Launch {
                    command: vec!["xdg-open".into(), "${value}".into()],
                    args: Default::default(),
                }]
                .as_slice()
            )
        );
    }

    #[test]
    fn history_is_not_created_without_a_history_widget() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("records"), "one\n").unwrap();
        let config = parse_config(
            r#"
                widget "items" "file" {
                    path   = "records"
                    format = "text"
                }
            "#,
            dir.path(),
        )
        .unwrap();
        let mut widgets = Widgets::from_config(&config).unwrap();
        widgets.record_history("ignored").unwrap();
        assert!(!dir.path().join("history.sqlite").exists());
    }

    #[test]
    fn parse_records_shares_text_json_and_yaml_rules() {
        assert_eq!(
            parse_records("one\n\ntwo\n", FileFormat::Text).unwrap(),
            vec![Record::new("one"), Record::new("two")]
        );
        assert_eq!(
            parse_records(
                r#"["plain", {"label":"Readable", "value":"copied", "extra":"ignored"}]"#,
                FileFormat::Json,
            )
            .unwrap(),
            vec![
                Record::new("plain"),
                Record {
                    key: "copied".into(),
                    label: "Readable".into(),
                    value: "copied".into(),
                },
            ]
        );
        assert_eq!(
            parse_records("- value: copied\n", FileFormat::Yaml).unwrap(),
            vec![Record::new("copied")]
        );
    }

    #[test]
    fn dbus_maps_default_missing_fields_and_reject_bad_keys() {
        let mut partial = BTreeMap::new();
        partial.insert("value".into(), "copied".into());
        partial.insert("extra".into(), "ignored".into());
        assert_eq!(
            records_from_maps("items", vec![partial]).unwrap(),
            Some(vec![Record::new("copied")])
        );

        let mut missing = BTreeMap::new();
        missing.insert("key".into(), "id".into());
        let error = records_from_maps("items", vec![missing]).unwrap_err();
        assert!(error.contains("value is required"), "{error}");

        let mut first = BTreeMap::new();
        first.insert("key".into(), "same".into());
        first.insert("value".into(), "one".into());
        let mut second = BTreeMap::new();
        second.insert("key".into(), "same".into());
        second.insert("value".into(), "two".into());
        let error = records_from_maps("items", vec![first, second]).unwrap_err();
        assert!(error.contains("duplicate record key"), "{error}");
        assert!(records_from_maps("items", Vec::new()).unwrap().is_none());
    }
}
