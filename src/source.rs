//! Named sources that provide records for popups.

use serde::Deserialize;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::config::{Config, FileFormat, SelectAction, SourceConfig};
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

pub struct Sources {
    entries: Vec<NamedSource>,
    windows: Option<WindowList>,
}

struct NamedSource {
    name: String,
    source: Source,
    presentation: Presentation,
    action: SelectAction,
}

enum Source {
    ClipboardHistory(HistoryStore),
    File(FileSource),
    Executables,
    WaylandWindows,
}

struct FileSource {
    path: PathBuf,
    format: FileFormat,
}

impl Sources {
    pub fn from_config(config: &Config) -> Result<Self, String> {
        let windows = config
            .sources
            .iter()
            .any(|source| matches!(source, SourceConfig::WaylandWindows { .. }))
            .then(WindowList::start);
        let mut entries = Vec::with_capacity(config.sources.len());
        for source in &config.sources {
            let name = source.name().to_owned();
            let presentation = source.presentation().clone();
            let action = source.action();
            let source = match source {
                SourceConfig::ClipboardHistory {
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
                                "source {name:?}: failed to open history database {}: {err}",
                                path.display()
                            )
                        })?
                        .with_coalesce_growing(*coalesce_growing)
                        .with_skip_whitespace_only(*skip_whitespace_only);
                    Source::ClipboardHistory(store)
                }
                SourceConfig::File { path, format, .. } => Source::File(FileSource {
                    path: path.clone(),
                    format: *format,
                }),
                SourceConfig::Executables { .. } => Source::Executables,
                SourceConfig::WaylandWindows { .. } => Source::WaylandWindows,
            };
            entries.push(NamedSource {
                name,
                source,
                presentation,
                action,
            });
        }

        let sources = Self { entries, windows };
        // Validate file readability, format, and keys at startup. Files are
        // still reloaded when opened so changes do not require a restart.
        for entry in &sources.entries {
            if matches!(entry.source, Source::File(_)) {
                sources.records(&entry.name)?;
            }
        }
        Ok(sources)
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

    pub fn records(&self, name: &str) -> Result<Vec<Record>, String> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| format!("unknown source {name:?}"))?;
        let records = match &entry.source {
            Source::ClipboardHistory(store) => store
                .entries()
                .map(|entries| entries.into_iter().map(Record::new).collect())
                .map_err(|err| format!("failed to read history: {err}"))?,
            Source::File(source) => source.records()?,
            Source::Executables => executables_on_path(&std::env::var("PATH").unwrap_or_default()),
            Source::WaylandWindows => self
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
            .ok_or_else(|| "no wayland-windows source is configured".to_string())?
            .activate(key)
    }

    pub fn record_history(&mut self, text: &str) -> Result<(), String> {
        for entry in &mut self.entries {
            if let Source::ClipboardHistory(store) = &mut entry.source {
                store.record(text).map_err(|err| {
                    format!("source {:?}: failed to record history: {err}", entry.name)
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

fn validate_records(source_name: &str, records: &[Record]) -> Result<(), String> {
    let mut keys = HashSet::new();
    for (index, record) in records.iter().enumerate() {
        if record.key.is_empty() {
            return Err(format!(
                "source {source_name:?}: record {} has an empty key",
                index + 1
            ));
        }
        if !keys.insert(&record.key) {
            return Err(format!(
                "source {source_name:?}: duplicate record key {:?}",
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

    fn file_source(format: &str, contents: &str) -> (tempfile::TempDir, Sources) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("records"), contents).unwrap();
        let config = parse_config(
            &format!(
                r#"
                    source "items" {{
                        type   = "file"
                        path   = "records"
                        format = "{format}"
                    }}
                "#
            ),
            dir.path(),
        )
        .unwrap();
        let sources = Sources::from_config(&config).unwrap();
        (dir, sources)
    }

    #[test]
    fn text_files_are_one_value_per_nonempty_line() {
        let (_dir, sources) = file_source("text", "one\n\ntwo\n");
        assert_eq!(
            sources.records("items").unwrap(),
            vec![Record::new("one"), Record::new("two")]
        );
    }

    #[test]
    fn json_supports_strings_and_full_records() {
        let (_dir, sources) = file_source(
            "json",
            r#"["plain", {"key":"id", "label":"Readable", "value":"copied"}]"#,
        );
        assert_eq!(
            sources.records("items").unwrap(),
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
        let (_dir, sources) = file_source("yaml", "- value: copied\n");
        assert_eq!(
            sources.records("items").unwrap(),
            vec![Record::new("copied")]
        );
    }

    #[test]
    fn duplicate_keys_are_rejected_with_source_context() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("records"),
            r#"[{"key":"same","value":"one"},{"key":"same","value":"two"}]"#,
        )
        .unwrap();
        let config = parse_config(
            r#"
                source "items" {
                    type   = "file"
                    path   = "records"
                    format = "json"
                }
            "#,
            dir.path(),
        )
        .unwrap();
        let error = Sources::from_config(&config).err().unwrap();
        assert!(error.contains("source \"items\""));
        assert!(error.contains("duplicate record key"));
    }

    #[test]
    fn file_sources_are_reloaded() {
        let (dir, sources) = file_source("text", "old\n");
        std::fs::write(dir.path().join("records"), "new\n").unwrap();
        assert_eq!(sources.records("items").unwrap(), vec![Record::new("new")]);
    }

    #[test]
    fn whitespace_history_respects_source_config() {
        let dir = tempfile::tempdir().unwrap();
        let config = parse_config(
            r#"
                source "history" {
                    type                 = "clipboard-history"
                    db                   = "history.sqlite"
                    skip_whitespace_only = false
                }
            "#,
            dir.path(),
        )
        .unwrap();
        let mut sources = Sources::from_config(&config).unwrap();
        sources.record_history("   \n").unwrap();
        assert_eq!(
            sources.records("history").unwrap(),
            vec![Record::new("   \n")]
        );
    }

    #[test]
    fn executables_source_uses_the_process_path() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("kb-path-test-bin");
        std::fs::write(&bin, "#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let config = parse_config(r#"source "run" { type = "executables" }"#, dir.path()).unwrap();
        let previous = std::env::var_os("PATH");
        std::env::set_var("PATH", dir.path());
        let sources = Sources::from_config(&config).unwrap();
        let records = sources.records("run").unwrap();
        match previous {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
        assert_eq!(
            sources.action("run"),
            Some(SelectAction::Exec {
                command: vec!["${value}".into()]
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
    fn configured_action_is_available_by_source_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("records"), "one\n").unwrap();
        let config = parse_config(
            r#"
                source "items" {
                    type   = "file"
                    path   = "records"
                    format = "text"
                    action = "copy-to-clipboard"
                }
            "#,
            dir.path(),
        )
        .unwrap();
        let sources = Sources::from_config(&config).unwrap();
        assert_eq!(sources.action("items"), Some(SelectAction::CopyToClipboard));
    }

    #[test]
    fn xdg_open_action_is_available_by_source_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("records"), "https://example.com\n").unwrap();
        let config = parse_config(
            r#"
                source "links" {
                    type   = "file"
                    path   = "records"
                    format = "text"
                    action = "xdg-open"
                }
            "#,
            dir.path(),
        )
        .unwrap();
        let sources = Sources::from_config(&config).unwrap();
        assert_eq!(
            sources.action("links"),
            Some(SelectAction::Exec {
                command: vec!["xdg-open".into(), "${value}".into()]
            })
        );
    }

    #[test]
    fn history_is_not_created_without_a_history_source() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("records"), "one\n").unwrap();
        let config = parse_config(
            r#"
                source "items" {
                    type   = "file"
                    path   = "records"
                    format = "text"
                }
            "#,
            dir.path(),
        )
        .unwrap();
        let mut sources = Sources::from_config(&config).unwrap();
        sources.record_history("ignored").unwrap();
        assert!(!dir.path().join("history.sqlite").exists());
    }
}
