//! Application configuration loaded from an XDG `.hcl` file.

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::launch::Launch;
use crate::theme::{
    parse_document_at, presentation_for_source, theme_from_document, HclDocument, HclValue,
    Location, Presentation, RawSource,
};
use crate::{
    default_db_path, Theme, DEFAULT_COALESCE_GROWING, DEFAULT_HISTORY_LIMIT, DEFAULT_LABEL_MAX,
    DEFAULT_SKIP_WHITESPACE_ONLY,
};

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub popup: PopupConfig,
    pub launch: Launch,
    pub sources: Vec<SourceConfig>,
}

#[derive(Debug, Clone)]
pub struct PopupConfig {
    pub label_max: usize,
    pub theme: Theme,
}

impl Default for PopupConfig {
    fn default() -> Self {
        Self {
            label_max: DEFAULT_LABEL_MAX,
            theme: Theme::default(),
        }
    }
}

/// What happens when a popup row from this source is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectAction {
    #[default]
    CopyToClipboard,
    XdgOpen,
}

impl SelectAction {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "copy-to-clipboard" => Some(Self::CopyToClipboard),
            "xdg-open" => Some(Self::XdgOpen),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::CopyToClipboard => "copy-to-clipboard",
            Self::XdgOpen => "xdg-open",
        }
    }

    fn supported() -> &'static str {
        "copy-to-clipboard or xdg-open"
    }

    fn default_for_type(_ty: &str) -> Self {
        Self::CopyToClipboard
    }
}

#[derive(Debug, Clone)]
pub enum SourceConfig {
    ClipboardHistory {
        name: String,
        db: Option<PathBuf>,
        history_limit: usize,
        coalesce_growing: bool,
        skip_whitespace_only: bool,
        presentation: Presentation,
        action: SelectAction,
    },
    File {
        name: String,
        path: PathBuf,
        format: FileFormat,
        presentation: Presentation,
        action: SelectAction,
    },
}

impl SourceConfig {
    pub fn name(&self) -> &str {
        match self {
            Self::ClipboardHistory { name, .. } | Self::File { name, .. } => name,
        }
    }

    pub fn presentation(&self) -> &Presentation {
        match self {
            Self::ClipboardHistory { presentation, .. } | Self::File { presentation, .. } => {
                presentation
            }
        }
    }

    pub fn action(&self) -> SelectAction {
        match self {
            Self::ClipboardHistory { action, .. } | Self::File { action, .. } => *action,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    config_home.join("klemmbrett").join("config.hcl")
}

/// Loads an explicit config, or the default config when it exists.
///
/// A missing default config intentionally produces a sync-only configuration.
pub fn load_config(explicit: Option<&Path>) -> Result<Config, String> {
    let path = explicit
        .map(PathBuf::from)
        .unwrap_or_else(default_config_path);
    if explicit.is_none() && !path.exists() {
        reject_legacy_config(&path)?;
        return Ok(Config::default());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|err| format!("failed to read config {}: {err}", path.display()))?;
    parse_config(&text, path.parent().unwrap_or_else(|| Path::new(".")))
        .map_err(|err| format!("invalid config {}: {err}", path.display()))
}

pub fn parse_config(text: &str, base_dir: &Path) -> Result<Config, String> {
    let mut doc = parse_document_at(text, base_dir)?;
    let (label_max, launch) = parse_configuration(&doc.configuration)?;
    let raw_sources = std::mem::take(&mut doc.sources);
    let mut sources = Vec::new();
    let mut names = HashSet::new();

    for raw in raw_sources {
        let source = source_from_raw(raw, &doc, base_dir)?;
        let name = source.name().to_owned();
        if name.trim().is_empty() {
            return Err("source name must not be empty".into());
        }
        if !names.insert(name.clone()) {
            return Err(format!("source name {name:?} is configured more than once"));
        }
        sources.push(source);
    }

    let theme = theme_from_document(&doc, base_dir)?;

    Ok(Config {
        popup: PopupConfig {
            label_max: label_max.max(1),
            theme,
        },
        launch,
        sources,
    })
}

fn parse_configuration(
    configuration: &BTreeMap<String, HclValue>,
) -> Result<(usize, Launch), String> {
    for key in configuration.keys() {
        if key != "label_max" && key != "launch" {
            return Err(format!(
                "unknown configuration property {key}; supported: label_max, launch"
            ));
        }
    }
    let label_max = match configuration.get("label_max") {
        None => DEFAULT_LABEL_MAX,
        Some(value) => {
            let number = value
                .as_number()
                .ok_or_else(|| "configuration label_max must be a number".to_string())?;
            if number < 1.0 {
                return Err("label_max must be at least 1".into());
            }
            number as usize
        }
    };
    let launch = match configuration.get("launch") {
        None => Launch::Auto,
        Some(value) => parse_launch(value)?,
    };
    Ok((label_max, launch))
}

fn parse_launch(value: &HclValue) -> Result<Launch, String> {
    if let Some(items) = value.as_list() {
        if items.is_empty() || items.iter().any(|item| item.trim().is_empty()) {
            return Err("launch wrapper must be a non-empty list of strings".into());
        }
        return Ok(Launch::Wrapper(items.to_vec()));
    }
    let name = value
        .as_ident()
        .ok_or_else(|| "launch must be auto, posix, systemd-run, or a command list".to_string())?;
    match name {
        "auto" => Ok(Launch::Auto),
        "posix" => Ok(Launch::Posix),
        "systemd-run" => Ok(Launch::SystemdRun),
        other => Err(format!(
            "unknown launch {other}; expected auto, posix, systemd-run, or a command list"
        )),
    }
}

fn source_from_raw(
    raw: RawSource,
    doc: &HclDocument,
    base_dir: &Path,
) -> Result<SourceConfig, String> {
    let name = raw.name;
    let presentation = source_presentation(&name, &raw.properties, doc, base_dir)?;
    let action = parse_select_action(&name, &raw.properties)?;
    let ty = raw
        .properties
        .get("type")
        .and_then(HclValue::as_ident)
        .ok_or_else(|| format!("source {name:?}: type is required"))?;
    let action = action.unwrap_or_else(|| SelectAction::default_for_type(ty));
    match ty {
        "clipboard-history" => {
            reject_unknown(
                &name,
                &raw.properties,
                &[
                    "type",
                    "db",
                    "history_limit",
                    "coalesce_growing",
                    "skip_whitespace_only",
                    "widget",
                    "theme",
                    "location",
                    "action",
                ],
            )?;
            let history_limit =
                optional_usize(&raw.properties, "history_limit")?.unwrap_or(DEFAULT_HISTORY_LIMIT);
            if history_limit == 0 {
                return Err(format!(
                    "source {name:?}: history_limit must be greater than zero"
                ));
            }
            let mut db = optional_path(&raw.properties, "db")?;
            let path = db.get_or_insert_with(default_db_path);
            resolve_relative(path, base_dir);
            Ok(SourceConfig::ClipboardHistory {
                name,
                db,
                history_limit,
                coalesce_growing: optional_bool(&raw.properties, "coalesce_growing")?
                    .unwrap_or(DEFAULT_COALESCE_GROWING),
                skip_whitespace_only: optional_bool(&raw.properties, "skip_whitespace_only")?
                    .unwrap_or(DEFAULT_SKIP_WHITESPACE_ONLY),
                presentation,
                action,
            })
        }
        "file" => {
            reject_unknown(
                &name,
                &raw.properties,
                &[
                    "type", "path", "format", "widget", "theme", "location", "action",
                ],
            )?;
            let mut path = required_path(&name, &raw.properties, "path")?;
            resolve_relative(&mut path, base_dir);
            let format = match raw
                .properties
                .get("format")
                .and_then(HclValue::as_ident)
                .ok_or_else(|| format!("source {name:?}: format is required"))?
            {
                "text" => FileFormat::Text,
                "json" => FileFormat::Json,
                "yaml" => FileFormat::Yaml,
                other => {
                    return Err(format!(
                        "source {name:?}: unknown format {other}; expected text, json, or yaml"
                    ));
                }
            };
            Ok(SourceConfig::File {
                name,
                path,
                format,
                presentation,
                action,
            })
        }
        other => Err(format!(
            "source {name:?}: unknown type {other}; expected clipboard-history or file"
        )),
    }
}

fn parse_select_action(
    source: &str,
    properties: &BTreeMap<String, HclValue>,
) -> Result<Option<SelectAction>, String> {
    match optional_name(properties, "action")? {
        None => Ok(None),
        Some(name) => SelectAction::parse(&name).map(Some).ok_or_else(|| {
            format!(
                "source {source:?}: unknown action {name}; expected {}",
                SelectAction::supported()
            )
        }),
    }
}

fn source_presentation(
    source: &str,
    properties: &BTreeMap<String, HclValue>,
    doc: &HclDocument,
    base_dir: &Path,
) -> Result<Presentation, String> {
    let theme = optional_name(properties, "theme")?;
    let widget = optional_name(properties, "widget")?;
    let location = match optional_name(properties, "location")? {
        None => None,
        Some(name) => Some(Location::parse(&name).ok_or_else(|| {
            format!(
                "source {source:?}: unknown location {name}; expected pointer, center, or a 9-point name such as north east"
            )
        })?),
    };
    presentation_for_source(
        doc,
        theme.as_deref(),
        widget.as_deref(),
        location,
        Some(source),
        properties.get("type").and_then(HclValue::as_ident),
        base_dir,
    )
    .map_err(|err| format!("source {source:?}: {err}"))
}

fn optional_name(
    properties: &BTreeMap<String, HclValue>,
    key: &str,
) -> Result<Option<String>, String> {
    match properties.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_text()
            .map(|text| Some(text.to_owned()))
            .ok_or_else(|| format!("{key} must be a name or string")),
    }
}

fn reject_unknown(
    source: &str,
    properties: &BTreeMap<String, HclValue>,
    allowed: &[&str],
) -> Result<(), String> {
    for key in properties.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!(
                "source {source:?}: unknown property {key}; supported: {}",
                allowed.join(", ")
            ));
        }
    }
    Ok(())
}

fn optional_bool(
    properties: &BTreeMap<String, HclValue>,
    key: &str,
) -> Result<Option<bool>, String> {
    match properties.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| format!("{key} must be true or false")),
    }
}

fn optional_usize(
    properties: &BTreeMap<String, HclValue>,
    key: &str,
) -> Result<Option<usize>, String> {
    match properties.get(key) {
        None => Ok(None),
        Some(value) => {
            let number = value
                .as_number()
                .ok_or_else(|| format!("{key} must be a number"))?;
            if number < 0.0 {
                return Err(format!("{key} must be at least 0"));
            }
            Ok(Some(number as usize))
        }
    }
}

fn optional_path(
    properties: &BTreeMap<String, HclValue>,
    key: &str,
) -> Result<Option<PathBuf>, String> {
    match properties.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_text()
            .map(|text| Some(PathBuf::from(text)))
            .ok_or_else(|| format!("{key} must be a path string")),
    }
}

fn required_path(
    source: &str,
    properties: &BTreeMap<String, HclValue>,
    key: &str,
) -> Result<PathBuf, String> {
    match optional_path(properties, key)? {
        Some(path) if !path.as_os_str().is_empty() => Ok(path),
        _ => Err(format!("source {source:?}: {key} is required")),
    }
}

fn reject_legacy_config(hcl_path: &Path) -> Result<(), String> {
    for (name, hint) in [(
        "config.toml",
        "move the settings into a .hcl file (configuration { } and source \"name\" { })",
    )] {
        let leftover = hcl_path.with_file_name(name);
        if leftover.exists() {
            return Err(format!(
                "found {}, but klemmbrett now reads config.hcl; {hint}",
                leftover.display()
            ));
        }
    }
    Ok(())
}

fn resolve_relative(path: &mut PathBuf, base_dir: &Path) {
    if path.is_relative() {
        *path = base_dir.join(&*path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_is_sync_only_with_popup_defaults() {
        let config = parse_config("", Path::new("/config")).unwrap();
        assert!(config.sources.is_empty());
        assert_eq!(config.popup.label_max, DEFAULT_LABEL_MAX);
        assert_eq!(config.popup.theme, Theme::default());
        assert_eq!(config.launch, crate::Launch::Auto);
    }

    #[test]
    fn theme_block_merges_a_shipped_palette() {
        let config = parse_config(r#"theme "nord" {}"#, Path::new("/config")).unwrap();
        assert_eq!(
            config.popup.theme.window().background,
            crate::Rgba {
                r: 0x2e,
                g: 0x34,
                b: 0x40,
                a: 255
            }
        );
        assert_eq!(config.popup.theme.window().border.top, 1.0);
    }

    #[test]
    fn a_relative_theme_file_is_merged_over_the_default() {
        let dir = std::env::temp_dir().join(format!(
            "klemmbrett-theme-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let theme_path = dir.join("popup.hcl");
        std::fs::write(
            &theme_path,
            r#"element "selected" { background_color = hex("010203") }
"#,
        )
        .unwrap();
        let config = parse_config(r#"theme "popup.hcl" {}"#, &dir).unwrap();
        assert_eq!(config.popup.theme.element_selected().background.r, 1);
        assert_eq!(config.popup.theme.element_selected().background.g, 2);
        assert_eq!(config.popup.theme.window().border.top, 1.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_theme_file_is_rejected() {
        let error = parse_config(r#"theme "missing.hcl" {}"#, Path::new("/config")).unwrap_err();
        assert!(error.contains("failed to read theme"), "{error}");
    }

    #[test]
    fn leftover_toml_is_rejected_when_hcl_is_missing() {
        let dir = std::env::temp_dir().join(format!(
            "klemmbrett-toml-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), "unused").unwrap();
        let error = reject_legacy_config(&dir.join("config.hcl")).unwrap_err();
        assert!(
            error.contains("config.toml") && error.contains("config.hcl"),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn source_defaults_and_relative_paths_are_resolved() {
        let config = parse_config(
            r#"
                source "history" {
                    type = "clipboard-history"
                    db   = "history.sqlite"
                }
                source "snippets" {
                    type   = "file"
                    path   = "snippets.json"
                    format = "json"
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();

        match &config.sources[0] {
            SourceConfig::ClipboardHistory {
                db,
                history_limit,
                coalesce_growing,
                skip_whitespace_only,
                ..
            } => {
                assert_eq!(db.as_deref(), Some(Path::new("/config/history.sqlite")));
                assert_eq!(*history_limit, DEFAULT_HISTORY_LIMIT);
                assert!(*coalesce_growing);
                assert!(*skip_whitespace_only);
                assert_eq!(config.sources[0].action(), SelectAction::CopyToClipboard);
            }
            _ => panic!("expected history source"),
        }
        match &config.sources[1] {
            SourceConfig::File { path, .. } => {
                assert_eq!(path, Path::new("/config/snippets.json"));
                assert_eq!(config.sources[1].action(), SelectAction::CopyToClipboard);
            }
            _ => panic!("expected file source"),
        }
    }

    #[test]
    fn source_action_can_be_copy_to_clipboard() {
        let config = parse_config(
            r#"
                source "history" {
                    type   = "clipboard-history"
                    action = "copy-to-clipboard"
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(config.sources[0].action(), SelectAction::CopyToClipboard);
    }

    #[test]
    fn source_action_can_be_xdg_open() {
        let config = parse_config(
            r#"
                source "links" {
                    type   = "file"
                    path   = "links.yaml"
                    format = "yaml"
                    action = "xdg-open"
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(config.sources[0].action(), SelectAction::XdgOpen);
    }

    #[test]
    fn launch_can_be_a_preset_or_a_wrapper() {
        let config = parse_config(
            r#"configuration { launch = "posix" }"#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(config.launch, crate::Launch::Posix);
        let config = parse_config(
            r#"configuration { launch = ["uwsm", "app", "--"] }"#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(
            config.launch,
            crate::Launch::Wrapper(vec!["uwsm".into(), "app".into(), "--".into()])
        );
    }

    #[test]
    fn empty_launch_wrapper_is_rejected() {
        let error =
            parse_config(r#"configuration { launch = [] }"#, Path::new("/config")).unwrap_err();
        assert!(error.contains("non-empty"), "{error}");
    }

    #[test]
    fn unknown_source_action_is_rejected() {
        let error = parse_config(
            r#"
                source "history" {
                    type   = "clipboard-history"
                    action = "type-as-keys"
                }
            "#,
            Path::new("/config"),
        )
        .unwrap_err();
        assert!(error.contains("unknown action type-as-keys"), "{error}");
        assert!(error.contains("copy-to-clipboard"), "{error}");
        assert!(error.contains("xdg-open"), "{error}");
    }

    #[test]
    fn duplicate_source_names_are_rejected() {
        let error = parse_config(
            r#"
                source "same" {
                    type   = "file"
                    path   = "one"
                    format = "text"
                }
                source "same" {
                    type   = "file"
                    path   = "two"
                    format = "text"
                }
            "#,
            Path::new("."),
        )
        .unwrap_err();
        assert!(error.contains("configured more than once"));
    }

    #[test]
    fn unknown_configuration_and_source_properties_are_rejected() {
        let error =
            parse_config(r#"configuration { theme = "nord" }"#, Path::new(".")).unwrap_err();
        assert!(
            error.contains("unknown configuration property theme"),
            "{error}"
        );
        let error = parse_config(
            r#"
                source "history" {
                    type  = "clipboard-history"
                    extra = true
                }
            "#,
            Path::new("."),
        )
        .unwrap_err();
        assert!(error.contains("unknown property extra"), "{error}");
    }

    #[test]
    fn missing_required_file_field_names_the_field() {
        let error = parse_config(
            r#"
                source "broken" {
                    type   = "file"
                    format = "json"
                }
            "#,
            Path::new("."),
        )
        .unwrap_err();
        assert!(error.contains("path"));
    }

    #[test]
    fn skip_whitespace_only_can_be_disabled() {
        let config = parse_config(
            r#"
                source "history" {
                    type = "clipboard-history"
                    skip_whitespace_only = false
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        match &config.sources[0] {
            SourceConfig::ClipboardHistory {
                skip_whitespace_only,
                ..
            } => assert!(!*skip_whitespace_only),
            _ => panic!("expected history source"),
        }
    }

    #[test]
    fn in_file_theme_blocks_override_theme() {
        let config = parse_config(
            r#"
                theme "nord" {}
                element "selected" { background_color = hex("ff0000") }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(
            config.popup.theme.element_selected().background,
            crate::Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 255
            }
        );
        assert_eq!(
            config.popup.theme.window().background,
            crate::Rgba {
                r: 0x2e,
                g: 0x34,
                b: 0x40,
                a: 255
            }
        );
    }

    #[test]
    fn source_theme_and_widget_compose() {
        let config = parse_config(
            r#"
                theme "nord" {}
                source "history" {
                    type   = "clipboard-history"
                    widget = "filter"
                }
                source "snippets" {
                    type   = "file"
                    path   = "snippets.yaml"
                    format = "yaml"
                    widget = "menu"
                    theme  = "dracula"
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        let history = config.sources[0].presentation();
        assert_eq!(history.location, Location::Center);
        assert!(history.theme.has_inputbar());
        assert_eq!(
            history.theme.window().background,
            crate::Rgba {
                r: 0x2e,
                g: 0x34,
                b: 0x40,
                a: 255
            }
        );
        let snippets = config.sources[1].presentation();
        assert_eq!(snippets.location, Location::Pointer);
        assert!(!snippets.theme.has_inputbar());
        assert_eq!(
            snippets.theme.window().background,
            crate::Rgba {
                r: 0x28,
                g: 0x2a,
                b: 0x36,
                a: 255
            }
        );
    }

    #[test]
    fn source_location_overrides_window_and_widget() {
        let config = parse_config(
            r#"
                source "history" {
                    type     = "clipboard-history"
                    widget   = "filter"
                    location = "south east"
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(
            config.sources[0].presentation().location,
            Location::SouthEast
        );
    }

    #[test]
    fn widget_location_can_depend_on_source_type() {
        let config = parse_config(
            r#"
                widget "smart" {
                    location = source.type == "clipboard-history" ? "center" : "pointer"
                }
                source "history" {
                    type   = "clipboard-history"
                    widget = "smart"
                }
                source "snippets" {
                    type   = "file"
                    path   = "snippets.yaml"
                    format = "yaml"
                    widget = "smart"
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(config.sources[0].presentation().location, Location::Center);
        assert_eq!(config.sources[1].presentation().location, Location::Pointer);
    }

    #[test]
    fn unknown_widget_name_is_rejected() {
        let error = parse_config(
            r#"
                source "history" {
                    type   = "clipboard-history"
                    widget = "missing"
                }
            "#,
            Path::new("/config"),
        )
        .unwrap_err();
        assert!(error.contains("failed to read widget"), "{error}");
    }

    #[test]
    fn xdg_config_home_wins_over_home() {
        assert_eq!(
            config_path_from_env(Some(OsStr::new("/xdg")), Some(OsStr::new("/home/me"))),
            PathBuf::from("/xdg/klemmbrett/config.hcl")
        );
    }
}
