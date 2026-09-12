//! Application configuration loaded from an XDG TOML file.

use serde::Deserialize;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::{default_db_path, DEFAULT_COALESCE_GROWING, DEFAULT_HISTORY_LIMIT, DEFAULT_LABEL_MAX};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub popup: PopupConfig,
    pub sources: Vec<SourceConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct PopupConfig {
    pub label_max: usize,
}

impl Default for PopupConfig {
    fn default() -> Self {
        Self {
            label_max: DEFAULT_LABEL_MAX,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SourceConfig {
    ClipboardHistory {
        name: String,
        #[serde(default)]
        db: Option<PathBuf>,
        #[serde(default = "default_history_limit", rename = "history-limit")]
        history_limit: usize,
        #[serde(default = "default_coalesce_growing", rename = "coalesce-growing")]
        coalesce_growing: bool,
    },
    File {
        name: String,
        path: PathBuf,
        format: FileFormat,
    },
}

impl SourceConfig {
    pub fn name(&self) -> &str {
        match self {
            Self::ClipboardHistory { name, .. } | Self::File { name, .. } => name,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FileFormat {
    Text,
    Json,
    Yaml,
}

pub fn default_config_path() -> PathBuf {
    config_path_from_env(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

pub fn config_path_from_env(xdg_config_home: Option<&OsStr>, home: Option<&OsStr>) -> PathBuf {
    let config_home = xdg_config_home
        .map(PathBuf::from)
        .or_else(|| home.map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    config_home.join("klemmbrett").join("config.toml")
}

/// Loads an explicit config, or the default config when it exists.
///
/// A missing default config intentionally produces a sync-only configuration.
pub fn load_config(explicit: Option<&Path>) -> Result<Config, String> {
    let path = explicit
        .map(PathBuf::from)
        .unwrap_or_else(default_config_path);
    if explicit.is_none() && !path.exists() {
        return Ok(Config::default());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|err| format!("failed to read config {}: {err}", path.display()))?;
    parse_config(&text, path.parent().unwrap_or_else(|| Path::new(".")))
        .map_err(|err| format!("invalid config {}: {err}", path.display()))
}

pub fn parse_config(text: &str, base_dir: &Path) -> Result<Config, String> {
    let mut config: Config = toml::from_str(text).map_err(|err| err.to_string())?;
    let mut names = HashSet::new();

    for source in &mut config.sources {
        let name = source.name();
        if name.trim().is_empty() {
            return Err("source name must not be empty".into());
        }
        if !names.insert(name.to_owned()) {
            return Err(format!("source name {name:?} is configured more than once"));
        }

        match source {
            SourceConfig::ClipboardHistory {
                name,
                db,
                history_limit,
                ..
            } => {
                if *history_limit == 0 {
                    return Err(format!(
                        "source {name:?}: history-limit must be greater than zero"
                    ));
                }
                let path = db.get_or_insert_with(default_db_path);
                resolve_relative(path, base_dir);
            }
            SourceConfig::File { name, path, .. } => {
                if path.as_os_str().is_empty() {
                    return Err(format!("source {name:?}: path must not be empty"));
                }
                resolve_relative(path, base_dir);
            }
        }
    }

    config.popup.label_max = config.popup.label_max.max(1);
    Ok(config)
}

fn resolve_relative(path: &mut PathBuf, base_dir: &Path) {
    if path.is_relative() {
        *path = base_dir.join(&*path);
    }
}

fn default_history_limit() -> usize {
    DEFAULT_HISTORY_LIMIT
}

fn default_coalesce_growing() -> bool {
    DEFAULT_COALESCE_GROWING
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_is_sync_only_with_popup_defaults() {
        let config = parse_config("", Path::new("/config")).unwrap();
        assert!(config.sources.is_empty());
        assert_eq!(config.popup.label_max, DEFAULT_LABEL_MAX);
    }

    #[test]
    fn source_defaults_and_relative_paths_are_resolved() {
        let config = parse_config(
            r#"
                [[sources]]
                name = "history"
                type = "clipboard-history"
                db = "history.sqlite"

                [[sources]]
                name = "snippets"
                type = "file"
                path = "snippets.json"
                format = "json"
            "#,
            Path::new("/config"),
        )
        .unwrap();

        match &config.sources[0] {
            SourceConfig::ClipboardHistory {
                db,
                history_limit,
                coalesce_growing,
                ..
            } => {
                assert_eq!(db.as_deref(), Some(Path::new("/config/history.sqlite")));
                assert_eq!(*history_limit, DEFAULT_HISTORY_LIMIT);
                assert!(*coalesce_growing);
            }
            _ => panic!("expected history source"),
        }
        match &config.sources[1] {
            SourceConfig::File { path, .. } => {
                assert_eq!(path, Path::new("/config/snippets.json"));
            }
            _ => panic!("expected file source"),
        }
    }

    #[test]
    fn duplicate_source_names_are_rejected() {
        let error = parse_config(
            r#"
                [[sources]]
                name = "same"
                type = "file"
                path = "one"
                format = "text"

                [[sources]]
                name = "same"
                type = "file"
                path = "two"
                format = "text"
            "#,
            Path::new("."),
        )
        .unwrap_err();
        assert!(error.contains("configured more than once"));
    }

    #[test]
    fn missing_required_file_field_names_the_field() {
        let error = parse_config(
            r#"
                [[sources]]
                name = "broken"
                type = "file"
                format = "json"
            "#,
            Path::new("."),
        )
        .unwrap_err();
        assert!(error.contains("path"));
    }

    #[test]
    fn xdg_config_home_wins_over_home() {
        assert_eq!(
            config_path_from_env(Some(OsStr::new("/xdg")), Some(OsStr::new("/home/me"))),
            PathBuf::from("/xdg/klemmbrett/config.toml")
        );
    }
}
