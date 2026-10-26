//! Named widgets that provide records for popups.

use serde::Deserialize;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::config::{Config, FileFormat, SelectAction, Transform, WidgetConfig};
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
    provider: Provider,
    presentation: Presentation,
    action: SelectAction,
    transforms: Vec<Transform>,
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
            let action = widget.action();
            let transforms = widget.transforms().to_vec();
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
                    Provider::ClipboardHistory(store)
                }
                WidgetConfig::File { path, format, .. } => Provider::File(FileSource {
                    path: path.clone(),
                    format: *format,
                }),
                WidgetConfig::Executables { .. } => Provider::Executables,
                WidgetConfig::WaylandWindows { .. } => Provider::WaylandWindows,
            };
            entries.push(NamedWidget {
                name,
                provider,
                presentation,
                action,
                transforms,
            });
        }

        let widgets = Self { entries, windows };
        // Validate file readability, format, and keys at startup. Files are
        // still reloaded when opened so changes do not require a restart.
        for entry in &widgets.entries {
            if matches!(entry.provider, Provider::File(_)) {
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

    pub fn presentation(&self, name: &str) -> Option<&Presentation> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| &entry.presentation)
    }

    pub fn action(&self, name: &str) -> Option<SelectAction> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.action.clone())
    }

    pub fn transforms(&self, name: &str) -> Option<&[Transform]> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.transforms.as_slice())
    }

    pub fn records(&self, name: &str) -> Result<Vec<Record>, String> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| format!("unknown widget {name:?}"))?;
        let records = match &entry.provider {
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
            if let Provider::ClipboardHistory(store) = &mut entry.provider {
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
        let records = match self.format {
            FileFormat::Text => text
                .lines()
                .map(|line| line.strip_suffix('\r').unwrap_or(line))
                .filter(|line| !line.is_empty())
                .map(Record::new)
                .collect(),
            FileFormat::Json => deserialize_records(
                serde_json::from_str(&text)
                    .map_err(|err| format!("invalid JSON in {}: {err}", self.path.display()))?,
            ),
            FileFormat::Yaml => deserialize_records(
                serde_yaml::from_str(&text)
                    .map_err(|err| format!("invalid YAML in {}: {err}", self.path.display()))?,
            ),
        };
        Ok(records)
    }
}

fn deserialize_records(records: Vec<SerializedRecord>) -> Vec<Record> {
    records.into_iter().map(Record::from).collect()
}

fn validate_records(widget_name: &str, records: &[Record]) -> Result<(), String> {
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
                    widget "items" {{
                        provider   = "file"
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
                widget "items" {
                    provider   = "file"
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
                widget "history" {
                    provider                 = "clipboard-history"
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
        let config =
            parse_config(r#"widget "run" { provider = "executables" }"#, dir.path()).unwrap();
        let previous = std::env::var_os("PATH");
        std::env::set_var("PATH", dir.path());
        let widgets = Widgets::from_config(&config).unwrap();
        let records = widgets.records("run").unwrap();
        match previous {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
        assert_eq!(
            widgets.action("run"),
            Some(SelectAction::Exec {
                command: vec!["${value}".into()],
                args: Default::default(),
            })
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
                transform "jq" {
                    args {
                        expr = ".title"
                    }
                    command = ["jq", "-r", "${expr}"]
                }
                widget "items" {
                    provider = "clipboard-history"
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
                [Transform {
                    command: vec!["jq".into(), "-r".into(), "${expr}".into()],
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
                action "notify" {
                    type = "exec"
                    args {
                        title = "klemmbrett"
                    }
                    command = ["notify-send", "${title}", "${value}"]
                }
                widget "items" {
                    provider = "file"
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
                .action("items")
                .and_then(|action| action.argv("one", "Readable", "payload")),
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
                widget "items" {
                    provider   = "file"
                    path   = "records"
                    format = "text"
                    action = "copy-to-clipboard"
                }
            "#,
            dir.path(),
        )
        .unwrap();
        let widgets = Widgets::from_config(&config).unwrap();
        assert_eq!(widgets.action("items"), Some(SelectAction::CopyToClipboard));
    }

    #[test]
    fn xdg_open_action_is_available_by_widget_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("records"), "https://example.com\n").unwrap();
        let config = parse_config(
            r#"
                widget "links" {
                    provider   = "file"
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
            widgets.action("links"),
            Some(SelectAction::Exec {
                command: vec!["xdg-open".into(), "${value}".into()],
                args: Default::default(),
            })
        );
    }

    #[test]
    fn history_is_not_created_without_a_history_widget() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("records"), "one\n").unwrap();
        let config = parse_config(
            r#"
                widget "items" {
                    provider   = "file"
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
}
