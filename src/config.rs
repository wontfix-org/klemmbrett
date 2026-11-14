//! Daemon settings and where the Python configuration lives.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::launch::Launch;
use crate::layout::LayoutSpec;
use crate::notifications::NotificationConfig;
use crate::rank::Ranking;
use crate::widget::{Handlers, NoHandlers, WidgetDef, Widgets};
use crate::DEFAULT_LABEL_MAX;

/// The ad-hoc picker's usage namespace and the name layouts see.
pub const PICKER_NAME: &str = "picker";
/// Shown in front of the filter when the layout's prompt has no text.
pub const PICKER_PROMPT: &str = "> ";

/// Everything the daemon needs once the configuration has been evaluated.
pub struct AppConfig {
    pub label_max: usize,
    pub launch: Launch,
    pub widgets: Widgets,
    pub handlers: Arc<dyn Handlers>,
    /// `Some` when the configuration serves notifications.
    pub notifications: Option<NotificationConfig>,
    /// The Varlink socket; `None` uses [`crate::socket_path`]'s default.
    pub socket: Option<PathBuf>,
    /// The picker `kb` opens when it is given no subcommand.
    pub picker: PickerConfig,
}

/// How the ad-hoc picker looks and matches. `prompt` replaces the label in
/// front of the filter; `None` leaves a layout's own text and fills a blank
/// one with [`PICKER_PROMPT`].
pub struct PickerConfig {
    pub def: WidgetDef,
    pub prompt: Option<String>,
}

impl Default for PickerConfig {
    fn default() -> Self {
        let mut def = WidgetDef::new(PICKER_NAME, None);
        def.layout = LayoutSpec::filter();
        def.ranking = Ranking::fuzzy();
        Self { def, prompt: None }
    }
}

impl Default for AppConfig {
    /// Clipboard sync only: no widgets and no history.
    fn default() -> Self {
        Self {
            label_max: DEFAULT_LABEL_MAX,
            launch: Launch::Auto,
            widgets: Widgets::empty(),
            handlers: Arc::new(NoHandlers),
            notifications: None,
            socket: None,
            picker: PickerConfig::default(),
        }
    }
}

/// Optional multi-select on a widget. Tab toggles and moves down; Shift-Tab
/// toggles and moves up, matching fzf `--multi`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiSelect {
    pub select: KeySpec,
    pub deselect: KeySpec,
}

impl MultiSelect {
    /// `Some(true)` turns multi-select on, using `configured` or Tab /
    /// Shift-Tab. `Some(false)` turns it off. `None` keeps `configured`.
    pub fn resolve(configured: Option<Self>, choice: Option<bool>) -> Option<Self> {
        match choice {
            Some(true) => Some(configured.unwrap_or_else(Self::fzf)),
            Some(false) => None,
            None => configured,
        }
    }

    pub fn fzf() -> Self {
        Self {
            select: KeySpec::parse("tab").expect("tab"),
            deselect: KeySpec::parse("shift-tab").expect("shift-tab"),
        }
    }
}

/// A key plus optional modifiers (`tab`, `shift-tab`, `ctrl-space`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeySpec {
    pub key: String,
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
}

impl KeySpec {
    pub fn parse(spec: &str) -> Result<Self, String> {
        let mut shift = false;
        let mut control = false;
        let mut alt = false;
        let mut key = None;
        for part in spec.split('-') {
            match part.to_ascii_lowercase().as_str() {
                "" => return Err(format!("invalid key {spec:?}")),
                "shift" => shift = true,
                "ctrl" | "control" => control = true,
                "alt" => alt = true,
                "btab" => {
                    if key.is_some() {
                        return Err(format!("invalid key {spec:?}"));
                    }
                    shift = true;
                    key = Some("tab".into());
                }
                part => {
                    if key.is_some() {
                        return Err(format!("invalid key {spec:?}"));
                    }
                    key = Some(part.to_owned());
                }
            }
        }
        let key = key.ok_or_else(|| format!("invalid key {spec:?}"))?;
        Ok(Self {
            key,
            shift,
            control,
            alt,
        })
    }

    pub fn matches(&self, key: &str, shift: bool, control: bool, alt: bool) -> bool {
        self.shift == shift
            && self.control == control
            && self.alt == alt
            && self.key.eq_ignore_ascii_case(key)
    }
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
    config_home.join("klemmbrett").join("config.py")
}

/// The configuration file to evaluate, if any.
///
/// An explicit path is always used. A missing default file means a sync-only
/// daemon.
pub fn resolve_config_path(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = explicit {
        return Some(path.to_owned());
    }
    Some(default_config_path()).filter(|path| path.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_select_resolve_overrides_the_configured_keys() {
        let keys = MultiSelect::fzf();
        assert_eq!(MultiSelect::resolve(None, None), None);
        assert_eq!(
            MultiSelect::resolve(Some(keys.clone()), None),
            Some(keys.clone())
        );
        assert_eq!(MultiSelect::resolve(None, Some(true)), Some(keys.clone()));
        assert_eq!(MultiSelect::resolve(Some(keys), Some(false)), None);
    }

    #[test]
    fn key_spec_parses_fzf_names_and_modifiers() {
        let tab = KeySpec::parse("tab").unwrap();
        assert!(tab.matches("tab", false, false, false));
        assert!(tab.matches("Tab", false, false, false));
        assert!(!tab.matches("tab", true, false, false));
        let shift_tab = KeySpec::parse("shift-tab").unwrap();
        assert_eq!(shift_tab, KeySpec::parse("btab").unwrap());
        assert!(shift_tab.matches("tab", true, false, false));
        let ctrl_space = KeySpec::parse("ctrl-space").unwrap();
        assert!(ctrl_space.matches("space", false, true, false));
        assert!(KeySpec::parse("shift-").is_err());
        assert!(KeySpec::parse("shift").is_err());
    }

    #[test]
    fn xdg_config_home_wins_over_home() {
        assert_eq!(
            config_path_from_env(Some(OsStr::new("/xdg")), Some(OsStr::new("/home/me"))),
            PathBuf::from("/xdg/klemmbrett/config.py")
        );
        assert_eq!(
            config_path_from_env(None, Some(OsStr::new("/home/me"))),
            PathBuf::from("/home/me/.config/klemmbrett/config.py")
        );
    }
}
