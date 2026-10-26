//! Application configuration loaded from an XDG `.hcl` file.

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::launch::Launch;
use crate::theme::{
    parse_document_at, presentation_for_widget, theme_from_document, HclDocument, HclValue,
    Location, Presentation, RawAction, RawActionInvocation, RawTransform, RawTransformInvocation,
    RawWidget,
};
use crate::{
    default_db_path, Theme, DEFAULT_COALESCE_GROWING, DEFAULT_HISTORY_LIMIT, DEFAULT_LABEL_MAX,
    DEFAULT_SKIP_WHITESPACE_ONLY,
};

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub popup: PopupConfig,
    pub launch: Launch,
    pub widgets: Vec<WidgetConfig>,
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

/// What happens when a popup row from this widget is chosen.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SelectAction {
    #[default]
    CopyToClipboard,
    Launch {
        command: Vec<String>,
        args: BTreeMap<String, String>,
    },
    Exec {
        command: Vec<String>,
        stdin: String,
        args: BTreeMap<String, String>,
    },
    ActivateWindow,
}

impl SelectAction {
    fn builtin(name: &str) -> Option<Self> {
        match name {
            "copy-to-clipboard" => Some(Self::CopyToClipboard),
            "launch" => Some(Self::default_launch()),
            "exec" => Some(Self::default_exec()),
            "activate-window" => Some(Self::ActivateWindow),
            _ => None,
        }
    }

    fn is_reserved(name: &str) -> bool {
        matches!(
            name,
            "copy-to-clipboard" | "launch" | "exec" | "activate-window"
        )
    }

    fn default_launch() -> Self {
        Self::Launch {
            command: vec!["${value}".into()],
            args: BTreeMap::new(),
        }
    }

    fn default_exec() -> Self {
        Self::Exec {
            command: vec!["${value}".into()],
            stdin: String::new(),
            args: BTreeMap::new(),
        }
    }

    fn shipped_xdg_open() -> Self {
        Self::Launch {
            command: vec!["xdg-open".into(), "${value}".into()],
            args: BTreeMap::new(),
        }
    }

    fn default_for_provider(ty: &str) -> Self {
        match ty {
            "executables" => Self::default_launch(),
            "wayland-windows" => Self::ActivateWindow,
            _ => Self::CopyToClipboard,
        }
    }

    pub fn argv(&self, key: &str, label: &str, value: &str) -> Option<Vec<String>> {
        match self {
            Self::Launch { command, args } | Self::Exec { command, args, .. } => {
                let extras = interpolate_arg_map(args, key, label, value);
                Some(interpolate_command_with(
                    command, key, label, value, &extras,
                ))
            }
            _ => None,
        }
    }

    pub fn stdin(&self, key: &str, label: &str, value: &str) -> Option<String> {
        match self {
            Self::Exec { stdin, args, .. } => {
                let extras = interpolate_arg_map(args, key, label, value);
                Some(interpolate_text(stdin, key, label, value, &extras))
            }
            _ => None,
        }
    }
}

/// A named rewrite applied to the selected value before the widget action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transform {
    Exec {
        command: Vec<String>,
        stdin: String,
        args: BTreeMap<String, String>,
    },
    Regex {
        pattern: String,
        replace: String,
        on_miss: Miss,
        args: BTreeMap<String, String>,
    },
}

impl Transform {
    pub fn args(&self) -> &BTreeMap<String, String> {
        match self {
            Self::Exec { args, .. } | Self::Regex { args, .. } => args,
        }
    }

    fn with_args(&self, args: BTreeMap<String, String>) -> Self {
        match self {
            Self::Exec { command, stdin, .. } => Self::Exec {
                command: command.clone(),
                stdin: stdin.clone(),
                args,
            },
            Self::Regex {
                pattern,
                replace,
                on_miss,
                ..
            } => Self::Regex {
                pattern: pattern.clone(),
                replace: replace.clone(),
                on_miss: *on_miss,
                args,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Miss {
    Keep,
    Fail,
}

#[derive(Debug, Clone)]
pub enum WidgetConfig {
    ClipboardHistory {
        name: String,
        db: Option<PathBuf>,
        history_limit: usize,
        coalesce_growing: bool,
        skip_whitespace_only: bool,
        presentation: Presentation,
        actions: Vec<SelectAction>,
        transforms: Vec<Transform>,
    },
    File {
        name: String,
        path: PathBuf,
        format: FileFormat,
        presentation: Presentation,
        actions: Vec<SelectAction>,
        transforms: Vec<Transform>,
    },
    Executables {
        name: String,
        presentation: Presentation,
        actions: Vec<SelectAction>,
        transforms: Vec<Transform>,
    },
    WaylandWindows {
        name: String,
        presentation: Presentation,
        actions: Vec<SelectAction>,
        transforms: Vec<Transform>,
    },
}

impl WidgetConfig {
    pub fn name(&self) -> &str {
        match self {
            Self::ClipboardHistory { name, .. }
            | Self::File { name, .. }
            | Self::Executables { name, .. }
            | Self::WaylandWindows { name, .. } => name,
        }
    }

    pub fn presentation(&self) -> &Presentation {
        match self {
            Self::ClipboardHistory { presentation, .. }
            | Self::File { presentation, .. }
            | Self::Executables { presentation, .. }
            | Self::WaylandWindows { presentation, .. } => presentation,
        }
    }

    pub fn actions(&self) -> &[SelectAction] {
        match self {
            Self::ClipboardHistory { actions, .. }
            | Self::File { actions, .. }
            | Self::Executables { actions, .. }
            | Self::WaylandWindows { actions, .. } => actions,
        }
    }

    pub fn transforms(&self) -> &[Transform] {
        match self {
            Self::ClipboardHistory { transforms, .. }
            | Self::File { transforms, .. }
            | Self::Executables { transforms, .. }
            | Self::WaylandWindows { transforms, .. } => transforms,
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
    let actions = collect_actions(&doc)?;
    let named_transforms = collect_transforms(&doc)?;
    let raw_widgets = std::mem::take(&mut doc.widgets);
    let mut widgets = Vec::new();
    let mut names = HashSet::new();

    for raw in raw_widgets {
        let widget = widget_from_raw(raw, &doc, base_dir, &actions, &named_transforms)?;
        let name = widget.name().to_owned();
        if name.trim().is_empty() {
            return Err("widget name must not be empty".into());
        }
        if !names.insert(name.clone()) {
            return Err(format!("widget name {name:?} is configured more than once"));
        }
        widgets.push(widget);
    }

    let theme = theme_from_document(&doc, base_dir)?;

    Ok(Config {
        popup: PopupConfig {
            label_max: label_max.max(1),
            theme,
        },
        launch,
        widgets,
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

fn widget_from_raw(
    raw: RawWidget,
    doc: &HclDocument,
    base_dir: &Path,
    actions: &BTreeMap<String, SelectAction>,
    named_transforms: &BTreeMap<String, Transform>,
) -> Result<WidgetConfig, String> {
    let name = raw.name;
    let ty = raw.provider.as_str();
    let presentation = widget_presentation(&name, ty, &raw.properties, doc, base_dir)?;
    let actions = parse_select_actions(&name, &raw.properties, &raw.actions, actions)?;
    let transforms = resolve_transforms(&name, &raw.transforms, named_transforms)?;
    let actions = if actions.is_empty() {
        vec![SelectAction::default_for_provider(ty)]
    } else {
        actions
    };
    match ty {
        "clipboard-history" => {
            reject_unknown(
                "widget",
                &name,
                &raw.properties,
                &[
                    "db",
                    "history_limit",
                    "coalesce_growing",
                    "skip_whitespace_only",
                    "layout",
                    "theme",
                    "location",
                    "action",
                ],
            )?;
            let history_limit =
                optional_usize(&raw.properties, "history_limit")?.unwrap_or(DEFAULT_HISTORY_LIMIT);
            if history_limit == 0 {
                return Err(format!(
                    "widget {name:?}: history_limit must be greater than zero"
                ));
            }
            let mut db = optional_path(&raw.properties, "db")?;
            let path = db.get_or_insert_with(default_db_path);
            resolve_relative(path, base_dir);
            Ok(WidgetConfig::ClipboardHistory {
                name,
                db,
                history_limit,
                coalesce_growing: optional_bool(&raw.properties, "coalesce_growing")?
                    .unwrap_or(DEFAULT_COALESCE_GROWING),
                skip_whitespace_only: optional_bool(&raw.properties, "skip_whitespace_only")?
                    .unwrap_or(DEFAULT_SKIP_WHITESPACE_ONLY),
                presentation,
                actions,
                transforms,
            })
        }
        "file" => {
            reject_unknown(
                "widget",
                &name,
                &raw.properties,
                &["path", "format", "layout", "theme", "location", "action"],
            )?;
            let mut path = required_path(&name, &raw.properties, "path")?;
            resolve_relative(&mut path, base_dir);
            let format = match raw
                .properties
                .get("format")
                .and_then(HclValue::as_ident)
                .ok_or_else(|| format!("widget {name:?}: format is required"))?
            {
                "text" => FileFormat::Text,
                "json" => FileFormat::Json,
                "yaml" => FileFormat::Yaml,
                other => {
                    return Err(format!(
                        "widget {name:?}: unknown format {other}; expected text, json, or yaml"
                    ));
                }
            };
            Ok(WidgetConfig::File {
                name,
                path,
                format,
                presentation,
                actions,
                transforms,
            })
        }
        "executables" => {
            reject_unknown(
                "widget",
                &name,
                &raw.properties,
                &["layout", "theme", "location", "action"],
            )?;
            Ok(WidgetConfig::Executables {
                name,
                presentation,
                actions,
                transforms,
            })
        }
        "wayland-windows" => {
            reject_unknown(
                "widget",
                &name,
                &raw.properties,
                &["layout", "theme", "location", "action"],
            )?;
            Ok(WidgetConfig::WaylandWindows {
                name,
                presentation,
                actions,
                transforms,
            })
        }
        other => Err(format!(
            "widget {name:?}: unknown provider {other}; expected clipboard-history, file, executables, or wayland-windows"
        )),
    }
}

fn parse_select_actions(
    widget: &str,
    properties: &BTreeMap<String, HclValue>,
    invocations: &[RawActionInvocation],
    actions: &BTreeMap<String, SelectAction>,
) -> Result<Vec<SelectAction>, String> {
    match (optional_name(properties, "action")?, invocations) {
        (Some(_), [_, ..]) => Err(format!(
            "widget {widget:?}: action cannot be set as both a name and a block"
        )),
        (None, []) => Ok(Vec::new()),
        (Some(name), []) => resolve_action_name(&name, actions)
            .map(|action| vec![action])
            .map_err(|err| format!("widget {widget:?}: {err}")),
        (None, invocations) => invocations
            .iter()
            .map(|invocation| resolve_action_invocation(widget, invocation, actions))
            .collect(),
    }
}

fn collect_actions(doc: &HclDocument) -> Result<BTreeMap<String, SelectAction>, String> {
    let mut actions = BTreeMap::new();
    for (name, raw) in &doc.actions {
        if SelectAction::is_reserved(name) {
            return Err(format!(
                "action {name:?} is a built-in and cannot be redefined"
            ));
        }
        actions.insert(name.clone(), action_from_raw(name, raw)?);
    }
    actions
        .entry("xdg-open".into())
        .or_insert_with(SelectAction::shipped_xdg_open);
    Ok(actions)
}

fn action_from_raw(name: &str, raw: &RawAction) -> Result<SelectAction, String> {
    match raw.ty.as_str() {
        "copy-to-clipboard" => {
            reject_unknown("action", name, &raw.properties, &[])?;
            reject_action_args(name, raw)?;
            Ok(SelectAction::CopyToClipboard)
        }
        "activate-window" => {
            reject_unknown("action", name, &raw.properties, &[])?;
            reject_action_args(name, raw)?;
            Ok(SelectAction::ActivateWindow)
        }
        "launch" => {
            reject_unknown("action", name, &raw.properties, &["command"])?;
            let args = declared_arg_defaults("action", name, &raw.args)?;
            for default in args.values() {
                validate_placeholders("action", name, default, &[])?;
            }
            let extra: Vec<&str> = args.keys().map(String::as_str).collect();
            Ok(SelectAction::Launch {
                command: action_command(name, raw, &extra)?,
                args,
            })
        }
        "exec" => {
            reject_unknown(
                "action",
                name,
                &raw.properties,
                &["command", "stdin"],
            )?;
            let args = declared_arg_defaults("action", name, &raw.args)?;
            for default in args.values() {
                validate_placeholders("action", name, default, &[])?;
            }
            let extra: Vec<&str> = args.keys().map(String::as_str).collect();
            Ok(SelectAction::Exec {
                command: action_command(name, raw, &extra)?,
                stdin: optional_stdin("action", name, &raw.properties, &extra, "")?,
                args,
            })
        }
        other => Err(format!(
            "action {name:?}: unknown type {other}; expected copy-to-clipboard, launch, exec, or activate-window"
        )),
    }
}

fn action_command(name: &str, raw: &RawAction, extra: &[&str]) -> Result<Vec<String>, String> {
    match raw.properties.get("command") {
        None => Ok(vec!["${value}".into()]),
        Some(value) => {
            let items = value
                .as_list()
                .ok_or_else(|| format!("action {name:?}: command must be a list of strings"))?;
            if items.is_empty() || items.iter().any(|item| item.trim().is_empty()) {
                return Err(format!(
                    "action {name:?}: command must be a non-empty list of strings"
                ));
            }
            for item in items {
                validate_placeholders("action", name, item, extra)?;
            }
            Ok(items.to_vec())
        }
    }
}

fn collect_transforms(doc: &HclDocument) -> Result<BTreeMap<String, Transform>, String> {
    let mut transforms = BTreeMap::new();
    for (name, raw) in &doc.transforms {
        transforms.insert(name.clone(), named_transform_from_raw(name, raw)?);
    }
    Ok(transforms)
}

fn named_transform_from_raw(name: &str, raw: &RawTransform) -> Result<Transform, String> {
    let args = declared_arg_defaults("transform", name, &raw.args)?;
    let extra: Vec<&str> = args.keys().map(String::as_str).collect();
    match raw.ty.as_str() {
        "exec" => {
            reject_unknown("transform", name, &raw.properties, &["command", "stdin"])?;
            let command = raw
                .properties
                .get("command")
                .and_then(HclValue::as_list)
                .ok_or_else(|| format!("transform {name:?}: command is required"))?;
            if command.is_empty() || command.iter().any(|item| item.trim().is_empty()) {
                return Err(format!(
                    "transform {name:?}: command must be a non-empty list of strings"
                ));
            }
            for item in command {
                validate_placeholders("transform", name, item, &extra)?;
            }
            let stdin = optional_stdin("transform", name, &raw.properties, &extra, "${value}")?;
            Ok(Transform::Exec {
                command: command.to_vec(),
                stdin,
                args,
            })
        }
        "regex" => {
            reject_unknown(
                "transform",
                name,
                &raw.properties,
                &["pattern", "replace", "on_miss"],
            )?;
            let pattern = raw
                .properties
                .get("pattern")
                .and_then(HclValue::as_text)
                .ok_or_else(|| format!("transform {name:?}: pattern is required"))?;
            let replace = raw
                .properties
                .get("replace")
                .and_then(HclValue::as_text)
                .ok_or_else(|| format!("transform {name:?}: replace is required"))?;
            if pattern.is_empty() {
                return Err(format!("transform {name:?}: pattern must not be empty"));
            }
            validate_unclosed_placeholders("transform", name, pattern)?;
            validate_unclosed_placeholders("transform", name, replace)?;
            let on_miss = match raw.properties.get("on_miss").and_then(HclValue::as_ident) {
                None => Miss::Keep,
                Some("keep") => Miss::Keep,
                Some("fail") => Miss::Fail,
                Some(other) => {
                    return Err(format!(
                        "transform {name:?}: unknown on_miss {other}; expected keep or fail"
                    ));
                }
            };
            Ok(Transform::Regex {
                pattern: pattern.to_owned(),
                replace: replace.to_owned(),
                on_miss,
                args,
            })
        }
        other => Err(format!(
            "transform {name:?}: unknown type {other}; expected exec or regex"
        )),
    }
}

fn reject_action_args(name: &str, raw: &RawAction) -> Result<(), String> {
    if raw.args.is_empty() {
        return Ok(());
    }
    Err(format!(
        "action {name:?}: args are only supported for launch or exec"
    ))
}

fn optional_stdin(
    kind: &str,
    name: &str,
    properties: &BTreeMap<String, HclValue>,
    extra: &[&str],
    default: &str,
) -> Result<String, String> {
    match properties.get("stdin") {
        None => Ok(default.to_owned()),
        Some(value) => {
            let text = value
                .as_text()
                .ok_or_else(|| format!("{kind} {name:?}: stdin must be a string"))?;
            validate_placeholders(kind, name, text, extra)?;
            Ok(text.to_owned())
        }
    }
}

fn declared_arg_defaults(
    kind: &str,
    name: &str,
    raw: &BTreeMap<String, HclValue>,
) -> Result<BTreeMap<String, String>, String> {
    let mut args = BTreeMap::new();
    for (arg, value) in raw {
        if arg.trim().is_empty() {
            return Err(format!("{kind} {name:?}: argument name must not be empty"));
        }
        if matches!(arg.as_str(), "value" | "key" | "label") {
            return Err(format!(
                "{kind} {name:?}: argument {arg} is reserved; use a different name"
            ));
        }
        let default = value
            .as_text()
            .ok_or_else(|| format!("{kind} {name:?}: argument {arg} default must be a string"))?;
        args.insert(arg.clone(), default.to_owned());
    }
    Ok(args)
}

fn resolve_action_invocation(
    widget: &str,
    invocation: &RawActionInvocation,
    actions: &BTreeMap<String, SelectAction>,
) -> Result<SelectAction, String> {
    let mut action = resolve_action_name(&invocation.name, actions)
        .map_err(|err| format!("widget {widget:?}: {err}"))?;
    match &mut action {
        SelectAction::Launch { args, .. } | SelectAction::Exec { args, .. } => {
            *args = overlay_action_args(widget, &invocation.name, args, &invocation.properties)?;
        }
        _ if invocation.properties.is_empty() => {}
        _ => {
            let extra = invocation
                .properties
                .keys()
                .next()
                .expect("non-empty argument map");
            return Err(format!(
                "widget {widget:?}: action {:?}: unknown argument {extra}",
                invocation.name
            ));
        }
    }
    Ok(action)
}

fn overlay_action_args(
    widget: &str,
    action: &str,
    defaults: &BTreeMap<String, String>,
    properties: &BTreeMap<String, HclValue>,
) -> Result<BTreeMap<String, String>, String> {
    let mut args = defaults.clone();
    for (key, value) in properties {
        if !defaults.contains_key(key) {
            return Err(format!(
                "widget {widget:?}: action {action:?}: unknown argument {key}"
            ));
        }
        let text = value.as_text().ok_or_else(|| {
            format!("widget {widget:?}: action {action:?}: {key} must be a string")
        })?;
        validate_placeholders("widget", widget, text, &[])?;
        args.insert(key.clone(), text.to_owned());
    }
    Ok(args)
}

fn resolve_transforms(
    widget: &str,
    invocations: &[RawTransformInvocation],
    named: &BTreeMap<String, Transform>,
) -> Result<Vec<Transform>, String> {
    let mut transforms = Vec::with_capacity(invocations.len());
    for invocation in invocations {
        let spec = named.get(&invocation.name).ok_or_else(|| {
            format!(
                "widget {widget:?}: unknown transform {}; expected {}",
                invocation.name,
                supported_transforms(named)
            )
        })?;
        let mut args = spec.args().clone();
        for (key, value) in &invocation.properties {
            if !spec.args().contains_key(key) {
                return Err(format!(
                    "widget {widget:?}: transform {:?}: unknown argument {key}",
                    invocation.name
                ));
            }
            let text = value.as_text().ok_or_else(|| {
                format!(
                    "widget {widget:?}: transform {:?}: {key} must be a string",
                    invocation.name
                )
            })?;
            args.insert(key.clone(), text.to_owned());
        }
        transforms.push(spec.with_args(args));
    }
    Ok(transforms)
}

fn supported_transforms(transforms: &BTreeMap<String, Transform>) -> String {
    let names: Vec<&str> = transforms.keys().map(String::as_str).collect();
    match names.as_slice() {
        [] => "no transforms are configured".into(),
        [only] => (*only).to_owned(),
        [left, right] => format!("{left} or {right}"),
        [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
    }
}

fn resolve_action_name(
    name: &str,
    actions: &BTreeMap<String, SelectAction>,
) -> Result<SelectAction, String> {
    if let Some(action) = actions.get(name) {
        return Ok(action.clone());
    }
    SelectAction::builtin(name).ok_or_else(|| {
        format!(
            "unknown action {name}; expected {}",
            supported_actions(actions)
        )
    })
}

fn supported_actions(actions: &BTreeMap<String, SelectAction>) -> String {
    let mut names: Vec<&str> = vec!["copy-to-clipboard", "launch", "exec", "activate-window"];
    for name in actions.keys() {
        if !names.contains(&name.as_str()) {
            names.push(name);
        }
    }
    match names.as_slice() {
        [] => String::new(),
        [only] => (*only).to_owned(),
        [left, right] => format!("{left} or {right}"),
        [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
    }
}

fn validate_unclosed_placeholders(kind: &str, name: &str, input: &str) -> Result<(), String> {
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(format!(
                "{kind} {name:?}: unclosed placeholder in {input:?}"
            ));
        };
        rest = &after[end + 1..];
    }
    Ok(())
}

fn validate_placeholders(
    kind: &str,
    name: &str,
    input: &str,
    extra: &[&str],
) -> Result<(), String> {
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(format!(
                "{kind} {name:?}: unclosed placeholder in {input:?}"
            ));
        };
        let token = &after[..end];
        if !matches!(token, "value" | "key" | "label") && !extra.contains(&token) {
            let mut expected = vec!["${value}".into(), "${key}".into(), "${label}".into()];
            expected.extend(extra.iter().map(|arg| format!("${{{arg}}}")));
            return Err(format!(
                "{kind} {name:?}: unknown placeholder ${{{token}}}; expected {}",
                match expected.as_slice() {
                    [only] => only.clone(),
                    [left, right] => format!("{left} or {right}"),
                    [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
                    [] => String::new(),
                }
            ));
        }
        rest = &after[end + 1..];
    }
    Ok(())
}

fn interpolate_arg_map(
    args: &BTreeMap<String, String>,
    key: &str,
    label: &str,
    value: &str,
) -> BTreeMap<String, String> {
    args.iter()
        .map(|(name, template)| {
            (
                name.clone(),
                interpolate_arg(template, key, label, value, &BTreeMap::new()),
            )
        })
        .collect()
}

pub fn interpolate_text(
    input: &str,
    key: &str,
    label: &str,
    value: &str,
    extras: &BTreeMap<String, String>,
) -> String {
    interpolate_arg(input, key, label, value, extras)
}

pub fn interpolate_command_with(
    command: &[String],
    key: &str,
    label: &str,
    value: &str,
    extras: &BTreeMap<String, String>,
) -> Vec<String> {
    command
        .iter()
        .map(|part| interpolate_arg(part, key, label, value, extras))
        .collect()
}

fn interpolate_arg(
    input: &str,
    key: &str,
    label: &str,
    value: &str,
    extras: &BTreeMap<String, String>,
) -> String {
    let mut out = String::new();
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let token = &after[..end];
        let replacement = match token {
            "value" => value,
            "key" => key,
            "label" => label,
            other => match extras.get(other) {
                Some(text) => text.as_str(),
                None => {
                    out.push_str(&rest[start..start + 2 + end + 1]);
                    rest = &after[end + 1..];
                    continue;
                }
            },
        };
        out.push_str(replacement);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn widget_presentation(
    widget: &str,
    provider: &str,
    properties: &BTreeMap<String, HclValue>,
    doc: &HclDocument,
    base_dir: &Path,
) -> Result<Presentation, String> {
    let theme = optional_name(properties, "theme")?;
    let layout = optional_name(properties, "layout")?;
    let location = match optional_name(properties, "location")? {
        None => None,
        Some(name) => Some(Location::parse(&name).ok_or_else(|| {
            format!(
                "widget {widget:?}: unknown location {name}; expected pointer, center, or a 9-point name such as north east"
            )
        })?),
    };
    presentation_for_widget(
        doc,
        theme.as_deref(),
        layout.as_deref(),
        location,
        Some(widget),
        Some(provider),
        base_dir,
    )
    .map_err(|err| format!("widget {widget:?}: {err}"))
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
    kind: &str,
    name: &str,
    properties: &BTreeMap<String, HclValue>,
    allowed: &[&str],
) -> Result<(), String> {
    for key in properties.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!(
                "{kind} {name:?}: unknown property {key}; supported: {}",
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
    widget: &str,
    properties: &BTreeMap<String, HclValue>,
    key: &str,
) -> Result<PathBuf, String> {
    match optional_path(properties, key)? {
        Some(path) if !path.as_os_str().is_empty() => Ok(path),
        _ => Err(format!("widget {widget:?}: {key} is required")),
    }
}

fn reject_legacy_config(hcl_path: &Path) -> Result<(), String> {
    for (name, hint) in [(
        "config.toml",
        "move the settings into a .hcl file (configuration { } and widget \"name\" { })",
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
        assert!(config.widgets.is_empty());
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
    fn widget_defaults_and_relative_paths_are_resolved() {
        let config = parse_config(
            r#"
                widget "history" "clipboard-history" {
                    db   = "history.sqlite"
                }
                widget "snippets" "file" {
                    path   = "snippets.json"
                    format = "json"
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();

        match &config.widgets[0] {
            WidgetConfig::ClipboardHistory {
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
                assert_eq!(
                    config.widgets[0].actions()[0],
                    SelectAction::CopyToClipboard
                );
            }
            _ => panic!("expected history widget"),
        }
        match &config.widgets[1] {
            WidgetConfig::File { path, .. } => {
                assert_eq!(path, Path::new("/config/snippets.json"));
                assert_eq!(
                    config.widgets[1].actions()[0],
                    SelectAction::CopyToClipboard
                );
            }
            _ => panic!("expected file widget"),
        }
    }

    #[test]
    fn widget_action_can_be_copy_to_clipboard() {
        let config = parse_config(
            r#"
                widget "history" "clipboard-history" {
                    action = "copy-to-clipboard"
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(
            config.widgets[0].actions()[0],
            SelectAction::CopyToClipboard
        );
    }

    #[test]
    fn widget_action_can_be_xdg_open() {
        let config = parse_config(
            r#"
                widget "links" "file" {
                    path   = "links.yaml"
                    format = "yaml"
                    action = "xdg-open"
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(
            config.widgets[0].actions()[0],
            SelectAction::shipped_xdg_open()
        );
    }

    #[test]
    fn executables_and_windows_default_to_their_actions() {
        let config = parse_config(
            r#"
                widget "run" "executables" {}
                widget "windows" "wayland-windows" {}
            "#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(
            config.widgets[0].actions()[0],
            SelectAction::default_launch()
        );
        assert_eq!(config.widgets[1].actions()[0], SelectAction::ActivateWindow);
        assert!(matches!(
            config.widgets[0],
            WidgetConfig::Executables { .. }
        ));
        assert!(matches!(
            config.widgets[1],
            WidgetConfig::WaylandWindows { .. }
        ));
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
    fn unknown_widget_action_is_rejected() {
        let error = parse_config(
            r#"
                widget "history" "clipboard-history" {
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
    fn custom_action_is_selectable_by_name() {
        let config = parse_config(
            r#"
                action "private-firefox" "exec" {
                    command = ["firefox", "--private-window", "${value}"]
                }
                widget "links" "file" {
                    path   = "links.yaml"
                    format = "yaml"
                    action = "private-firefox"
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(
            config.widgets[0].actions()[0],
            SelectAction::Exec {
                command: vec![
                    "firefox".into(),
                    "--private-window".into(),
                    "${value}".into()
                ],
                stdin: String::new(),
                args: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn exec_command_interpolates_key_label_and_value() {
        assert_eq!(
            interpolate_command_with(
                &[
                    "notify-send".into(),
                    "${label}".into(),
                    "${key}: ${value}".into()
                ],
                "url",
                "Project website",
                "https://example.com",
                &BTreeMap::new()
            ),
            vec!["notify-send", "Project website", "url: https://example.com"]
        );
    }

    #[test]
    fn unknown_command_placeholder_is_rejected() {
        let error = parse_config(
            r#"
                action "bad" "exec" {
                    command = ["echo", "$${title}"]
                }
            "#,
            Path::new("/config"),
        )
        .unwrap_err();
        assert!(error.contains("unknown placeholder ${title}"), "{error}");
    }

    #[test]
    fn reserved_launch_name_cannot_be_redefined() {
        let error = parse_config(
            r#"action "launch" "copy-to-clipboard" {}"#,
            Path::new("/config"),
        )
        .unwrap_err();
        assert!(error.contains("built-in"), "{error}");
    }

    #[test]
    fn reserved_action_name_cannot_be_redefined() {
        let error = parse_config(
            r#"action "exec" "copy-to-clipboard" {}"#,
            Path::new("/config"),
        )
        .unwrap_err();
        assert!(error.contains("built-in"), "{error}");
    }

    #[test]
    fn empty_exec_command_is_rejected() {
        let error = parse_config(
            r#"
                action "open" "exec" {
                    command = []
                }
            "#,
            Path::new("/config"),
        )
        .unwrap_err();
        assert!(error.contains("non-empty"), "{error}");
    }

    #[test]
    fn action_type_must_be_a_built_in() {
        let error = parse_config(
            r#"
                action "open" "exec" {}
                action "also" "open" {}
            "#,
            Path::new("/config"),
        )
        .unwrap_err();
        assert!(error.contains("unknown type open"), "{error}");
        assert!(error.contains("activate-window"), "{error}");
    }

    #[test]
    fn launch_action_is_selectable_by_name() {
        let config = parse_config(
            r#"
                action "open" "launch" {
                    command = ["xdg-open", "${value}"]
                }
                widget "links" "file" {
                    path     = "links.yaml"
                    format   = "yaml"
                    action   = "open"
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(
            config.widgets[0].actions()[0],
            SelectAction::Launch {
                command: vec!["xdg-open".into(), "${value}".into()],
                args: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn exec_action_stdin_defaults_to_empty() {
        let config = parse_config(
            r#"
                action "notify" "exec" {
                    command = ["true"]
                }
                widget "links" "file" {
                    path     = "links.yaml"
                    format   = "yaml"
                    action   = "notify"
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(
            config.widgets[0].actions()[0].stdin("k", "Readable", "payload"),
            Some(String::new())
        );
    }

    #[test]
    fn exec_action_can_set_stdin() {
        let config = parse_config(
            r#"
                action "write" "exec" {
                    command = ["cat"]
                    stdin   = "${key}: ${value}"
                }
                widget "links" "file" {
                    path     = "links.yaml"
                    format   = "yaml"
                    action   = "write"
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(
            config.widgets[0].actions()[0].stdin("url", "Site", "https://ex"),
            Some("url: https://ex".into())
        );
    }

    #[test]
    fn widget_action_can_override_declared_args() {
        let config = parse_config(
            r#"
                action "notify" "exec" {
                    args {
                        title = "klemmbrett"
                        body  = "${value}"
                    }
                    command = ["notify-send", "${title}", "${body}"]
                }
                widget "links" "file" {
                    path     = "links.yaml"
                    format   = "yaml"
                    action "notify" {
                        title = "${label}"
                        body  = "${key}: ${value}"
                    }
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(
            config.widgets[0].actions()[0],
            SelectAction::Exec {
                command: vec!["notify-send".into(), "${title}".into(), "${body}".into()],
                stdin: String::new(),
                args: BTreeMap::from([
                    ("title".into(), "${label}".into()),
                    ("body".into(), "${key}: ${value}".into()),
                ]),
            }
        );
        assert_eq!(
            config.widgets[0].actions()[0].argv("url", "Site", "https://ex"),
            Some(vec![
                "notify-send".into(),
                "Site".into(),
                "url: https://ex".into()
            ])
        );
    }

    #[test]
    fn omitted_action_argument_uses_the_default() {
        let config = parse_config(
            r#"
                action "notify" "exec" {
                    args {
                        title = "klemmbrett"
                        body  = "${value}"
                    }
                    command = ["notify-send", "${title}", "${body}"]
                }
                widget "links" "file" {
                    path     = "links.yaml"
                    format   = "yaml"
                    action   = "notify"
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(
            config.widgets[0].actions()[0].argv("url", "Site", "https://ex"),
            Some(vec![
                "notify-send".into(),
                "klemmbrett".into(),
                "https://ex".into()
            ])
        );
    }

    #[test]
    fn empty_action_block_keeps_defaults() {
        let config = parse_config(
            r#"
                action "notify" "exec" {
                    args {
                        title = "klemmbrett"
                    }
                    command = ["notify-send", "${title}", "${value}"]
                }
                widget "links" "file" {
                    path     = "links.yaml"
                    format   = "yaml"
                    action "notify" {}
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        match &config.widgets[0].actions()[0] {
            SelectAction::Exec { args, .. } => {
                assert_eq!(
                    args,
                    &BTreeMap::from([("title".into(), "klemmbrett".into())])
                );
            }
            other => panic!("expected exec action, got {other:?}"),
        }
    }

    #[test]
    fn extra_action_argument_is_rejected() {
        let error = parse_config(
            r#"
                action "open" "exec" {
                    command = ["xdg-open", "${value}"]
                }
                widget "links" "file" {
                    path     = "links.yaml"
                    format   = "yaml"
                    action "open" {
                        title = "nope"
                    }
                }
            "#,
            Path::new("/config"),
        )
        .unwrap_err();
        assert!(error.contains("unknown argument title"), "{error}");
    }

    #[test]
    fn reserved_action_argument_name_is_rejected() {
        let error = parse_config(
            r#"
                action "echo" "exec" {
                    args {
                        value = "ignored"
                    }
                    command = ["echo", "${value}"]
                }
            "#,
            Path::new("/config"),
        )
        .unwrap_err();
        assert!(error.contains("reserved"), "{error}");
    }

    #[test]
    fn widget_action_argument_unknown_placeholder_is_rejected() {
        let error = parse_config(
            r#"
                action "notify" "exec" {
                    args {
                        title = "klemmbrett"
                    }
                    command = ["notify-send", "${title}"]
                }
                widget "links" "file" {
                    path     = "links.yaml"
                    format   = "yaml"
                    action "notify" {
                        title = "$${expr}"
                    }
                }
            "#,
            Path::new("/config"),
        )
        .unwrap_err();
        assert!(error.contains("unknown placeholder ${expr}"), "{error}");
    }

    #[test]
    fn action_name_and_block_together_are_rejected() {
        let error = parse_config(
            r#"
                widget "links" "file" {
                    path     = "links.yaml"
                    format   = "yaml"
                    action   = "xdg-open"
                    action "xdg-open" {}
                }
            "#,
            Path::new("/config"),
        )
        .unwrap_err();
        assert!(error.contains("both a name and a block"), "{error}");
    }

    #[test]
    fn widget_can_run_multiple_actions() {
        let config = parse_config(
            r#"
                action "notify" "exec" {
                    command = ["notify-send", "${value}"]
                }
                widget "links" "file" {
                    path   = "links.yaml"
                    format = "yaml"
                    action "copy-to-clipboard" {}
                    action "notify" {}
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(
            config.widgets[0].actions(),
            [
                SelectAction::CopyToClipboard,
                SelectAction::Exec {
                    command: vec!["notify-send".into(), "${value}".into()],
                    stdin: String::new(),
                    args: BTreeMap::new(),
                }
            ]
        );
    }

    #[test]
    fn copy_to_clipboard_cannot_declare_args() {
        let error = parse_config(
            r#"
                action "copy" "copy-to-clipboard" {
                    args {
                        title = "nope"
                    }
                }
            "#,
            Path::new("/config"),
        )
        .unwrap_err();
        assert!(
            error.contains("only supported for launch or exec"),
            "{error}"
        );
    }

    #[test]
    fn shipped_xdg_open_can_be_overridden() {
        let config = parse_config(
            r#"
                action "xdg-open" "launch" {
                    command = ["handlr", "open", "${value}"]
                }
                widget "links" "file" {
                    path   = "links.yaml"
                    format = "yaml"
                    action = "xdg-open"
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(
            config.widgets[0].actions()[0],
            SelectAction::Launch {
                command: vec!["handlr".into(), "open".into(), "${value}".into()],
                args: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn duplicate_widget_names_are_rejected() {
        let error = parse_config(
            r#"
                widget "same" "file" {
                    path   = "one"
                    format = "text"
                }
                widget "same" "file" {
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
    fn unknown_configuration_and_widget_properties_are_rejected() {
        let error =
            parse_config(r#"configuration { theme = "nord" }"#, Path::new(".")).unwrap_err();
        assert!(
            error.contains("unknown configuration property theme"),
            "{error}"
        );
        let error = parse_config(
            r#"
                widget "history" "clipboard-history" {
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
                widget "broken" "file" {
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
                widget "history" "clipboard-history" {
                    skip_whitespace_only = false
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        match &config.widgets[0] {
            WidgetConfig::ClipboardHistory {
                skip_whitespace_only,
                ..
            } => assert!(!*skip_whitespace_only),
            _ => panic!("expected history widget"),
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
    fn widget_theme_and_layout_compose() {
        let config = parse_config(
            r#"
                theme "nord" {}
                widget "history" "clipboard-history" {
                    layout = "filter"
                }
                widget "snippets" "file" {
                    path   = "snippets.yaml"
                    format = "yaml"
                    layout = "menu"
                    theme  = "dracula"
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        let history = config.widgets[0].presentation();
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
        let snippets = config.widgets[1].presentation();
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
    fn widget_location_overrides_window_and_layout() {
        let config = parse_config(
            r#"
                widget "history" "clipboard-history" {
                    layout   = "filter"
                    location = "south east"
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(
            config.widgets[0].presentation().location,
            Location::SouthEast
        );
    }

    #[test]
    fn layout_location_can_depend_on_widget_provider() {
        let config = parse_config(
            r#"
                layout "smart" {
                    location = widget.provider == "clipboard-history" ? "center" : "pointer"
                }
                widget "history" "clipboard-history" {
                    layout = "smart"
                }
                widget "snippets" "file" {
                    path   = "snippets.yaml"
                    format = "yaml"
                    layout = "smart"
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(config.widgets[0].presentation().location, Location::Center);
        assert_eq!(config.widgets[1].presentation().location, Location::Pointer);
    }

    #[test]
    fn unknown_layout_name_is_rejected() {
        let error = parse_config(
            r#"
                widget "history" "clipboard-history" {
                    layout = "missing"
                }
            "#,
            Path::new("/config"),
        )
        .unwrap_err();
        assert!(error.contains("failed to read layout"), "{error}");
    }

    #[test]
    fn widget_transforms_bind_declared_args() {
        let config = parse_config(
            r#"
                transform "jq" "exec" {
                    args {
                        expr = ".title"
                    }
                    command = ["jq", "-r", "${expr}"]
                }
                transform "decode" "exec" {
                    command = ["base64", "-d"]
                }
                widget "payloads" "file" {
                    path     = "payloads.yaml"
                    format   = "yaml"
                    transform "jq" {
                        expr = ".data"
                    }
                    transform "decode" {}
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(
            config.widgets[0].transforms(),
            [
                Transform::Exec {
                    command: vec!["jq".into(), "-r".into(), "${expr}".into()],
                    stdin: "${value}".into(),
                    args: BTreeMap::from([("expr".into(), ".data".into())]),
                },
                Transform::Exec {
                    command: vec!["base64".into(), "-d".into()],
                    stdin: "${value}".into(),
                    args: BTreeMap::new(),
                }
            ]
        );
    }

    #[test]
    fn omitted_transform_argument_uses_the_default() {
        let config = parse_config(
            r#"
                transform "jq" "exec" {
                    args {
                        expr = ".data"
                    }
                    command = ["jq", "-r", "${expr}"]
                }
                widget "payloads" "file" {
                    path     = "payloads.yaml"
                    format   = "yaml"
                    transform "jq" {}
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(
            config.widgets[0].transforms()[0]
                .args()
                .get("expr")
                .map(String::as_str),
            Some(".data")
        );
    }

    #[test]
    fn extra_transform_argument_is_rejected() {
        let error = parse_config(
            r#"
                transform "decode" "exec" {
                    command = ["base64", "-d"]
                }
                widget "payloads" "file" {
                    path     = "payloads.yaml"
                    format   = "yaml"
                    transform "decode" {
                        expr = ".data"
                    }
                }
            "#,
            Path::new("/config"),
        )
        .unwrap_err();
        assert!(error.contains("unknown argument expr"), "{error}");
    }

    #[test]
    fn unknown_transform_name_is_rejected() {
        let error = parse_config(
            r#"
                widget "payloads" "file" {
                    path     = "payloads.yaml"
                    format   = "yaml"
                    transform "missing" {}
                }
            "#,
            Path::new("/config"),
        )
        .unwrap_err();
        assert!(error.contains("unknown transform missing"), "{error}");
    }

    #[test]
    fn undeclared_transform_placeholder_is_rejected() {
        let error = parse_config(
            r#"
                transform "jq" "exec" {
                    command = ["jq", "-r", "$${expr}"]
                }
            "#,
            Path::new("/config"),
        )
        .unwrap_err();
        assert!(error.contains("unknown placeholder ${expr}"), "{error}");
    }

    #[test]
    fn transform_type_is_required() {
        let error = parse_config(
            r#"
                transform "jq" {
                    command = ["jq"]
                }
            "#,
            Path::new("/config"),
        )
        .unwrap_err();
        assert!(error.contains("type is required"), "{error}");
    }

    #[test]
    fn unknown_transform_type_is_rejected() {
        let error = parse_config(
            r#"
                transform "jq" "jq" {
                    command = ["jq"]
                }
            "#,
            Path::new("/config"),
        )
        .unwrap_err();
        assert!(error.contains("unknown type jq"), "{error}");
        assert!(error.contains("regex"), "{error}");
    }

    #[test]
    fn widget_regex_transform_binds_pattern_and_on_miss() {
        let config = parse_config(
            r#"
                transform "ticket" "regex" {
                    pattern = "https://[^/]+/browse/(?P<ticket>[A-Z]+-\\d+)"
                    replace = "${ticket}"
                    on_miss = "fail"
                }
                widget "links" "file" {
                    path     = "links.yaml"
                    format   = "yaml"
                    transform "ticket" {}
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(
            config.widgets[0].transforms(),
            [Transform::Regex {
                pattern: "https://[^/]+/browse/(?P<ticket>[A-Z]+-\\d+)".into(),
                replace: "${ticket}".into(),
                on_miss: Miss::Fail,
                args: BTreeMap::new(),
            }]
        );
    }

    #[test]
    fn omitted_regex_on_miss_keeps_the_value() {
        let config = parse_config(
            r#"
                transform "id" "regex" {
                    pattern = "\\d+"
                    replace = "N"
                }
                widget "links" "file" {
                    path     = "links.yaml"
                    format   = "yaml"
                    transform "id" {}
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        match &config.widgets[0].transforms()[0] {
            Transform::Regex { on_miss, .. } => assert_eq!(*on_miss, Miss::Keep),
            other => panic!("expected regex transform, got {other:?}"),
        }
    }

    #[test]
    fn unknown_regex_on_miss_is_rejected() {
        let error = parse_config(
            r#"
                transform "id" "regex" {
                    pattern = "\\d+"
                    replace = "N"
                    on_miss = "skip"
                }
            "#,
            Path::new("/config"),
        )
        .unwrap_err();
        assert!(error.contains("unknown on_miss skip"), "{error}");
    }

    #[test]
    fn regex_transform_can_take_pattern_args() {
        let config = parse_config(
            r#"
                transform "id" "regex" {
                    args {
                        prefix = "id-"
                    }
                    pattern = "${prefix}\\d+"
                    replace = "N"
                }
                widget "links" "file" {
                    path     = "links.yaml"
                    format   = "yaml"
                    transform "id" {
                        prefix = "ticket-"
                    }
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        assert_eq!(
            config.widgets[0].transforms(),
            [Transform::Regex {
                pattern: "${prefix}\\d+".into(),
                replace: "N".into(),
                on_miss: Miss::Keep,
                args: BTreeMap::from([("prefix".into(), "ticket-".into())]),
            }]
        );
    }

    #[test]
    fn exec_transform_stdin_defaults_to_the_value() {
        let config = parse_config(
            r#"
                transform "cat" "exec" {
                    command = ["cat"]
                }
                widget "items" "clipboard-history" {
                    transform "cat" {}
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        match &config.widgets[0].transforms()[0] {
            Transform::Exec { stdin, .. } => assert_eq!(stdin, "${value}"),
            other => panic!("expected exec transform, got {other:?}"),
        }
    }

    #[test]
    fn exec_transform_can_set_stdin() {
        let config = parse_config(
            r#"
                transform "wrap" "exec" {
                    command = ["cat"]
                    stdin   = "${key}: ${value}"
                }
                widget "items" "clipboard-history" {
                    transform "wrap" {}
                }
            "#,
            Path::new("/config"),
        )
        .unwrap();
        match &config.widgets[0].transforms()[0] {
            Transform::Exec { stdin, .. } => assert_eq!(stdin, "${key}: ${value}"),
            other => panic!("expected exec transform, got {other:?}"),
        }
    }

    #[test]
    fn reserved_transform_argument_name_is_rejected() {
        let error = parse_config(
            r#"
                transform "echo" "exec" {
                    args {
                        value = "ignored"
                    }
                    command = ["echo", "${value}"]
                }
            "#,
            Path::new("/config"),
        )
        .unwrap_err();
        assert!(error.contains("reserved"), "{error}");
    }

    #[test]
    fn xdg_config_home_wins_over_home() {
        assert_eq!(
            config_path_from_env(Some(OsStr::new("/xdg")), Some(OsStr::new("/home/me"))),
            PathBuf::from("/xdg/klemmbrett/config.hcl")
        );
    }
}
