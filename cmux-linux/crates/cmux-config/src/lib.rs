//! `cmux-config` — the `cmux.json` configuration model for cmux-linux.
//!
//! Upstream cmux stores user preferences in `~/.config/cmux/cmux.json` and lets
//! the CLI read/write arbitrary keys by dotted path (`cmux config get
//! appearance.fontSize`). This crate provides a strongly-typed [`Config`] with
//! serde defaults (so a partial or missing file still loads), plus generic
//! [`Config::get_path`] / [`Config::set_path`] that operate by dotted JSON path
//! against the same serialized shape.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod shortcuts;
pub use shortcuts::default_shortcuts;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("io error reading config: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid json in config: {0}")]
    Json(#[from] serde_json::Error),
    #[error("no such config path: {0}")]
    NoSuchPath(String),
    #[error("could not determine config directory")]
    NoConfigDir,
}

/// Top-level configuration. All fields default, so an empty `{}` is valid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub appearance: Appearance,
    pub sidebar: Sidebar,
    pub notifications: Notifications,
    /// Map of action id → key chord (e.g. `"newTab" -> "cmd+t"`). Merged over
    /// [`default_shortcuts`] at load time so user overrides win but unspecified
    /// actions keep their defaults.
    pub keyboard_shortcuts: std::collections::BTreeMap<String, String>,
    /// Override the shell used for new terminal panes. `None` = `$SHELL`.
    pub shell: Option<String>,
    /// User-defined actions (id → definition) shown in the command palette and
    /// runnable via `cmux run <id>`; each runs a shell command in a pane.
    #[serde(default)]
    pub actions: std::collections::BTreeMap<String, ActionDef>,
    /// Command run in a new workspace's first pane when it is created.
    #[serde(default)]
    pub new_workspace_command: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            appearance: Appearance::default(),
            sidebar: Sidebar::default(),
            notifications: Notifications::default(),
            keyboard_shortcuts: default_shortcuts(),
            shell: None,
            actions: std::collections::BTreeMap::new(),
            new_workspace_command: None,
        }
    }
}

/// A user-defined command-palette / CLI action from `cmux.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionDef {
    /// Shell command line to run in the target pane.
    pub command: String,
    /// Label shown in the command palette (defaults to the action id).
    #[serde(default)]
    pub label: Option<String>,
    /// Where the command runs.
    #[serde(default)]
    pub target: ActionTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActionTarget {
    /// Open a new tab and run the command there (default).
    #[default]
    NewTab,
    /// Run the command in the currently focused pane.
    CurrentPane,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Appearance {
    pub theme: Theme,
    pub font_family: String,
    pub font_size: f32,
    /// Background opacity in `[0,1]`.
    pub opacity: f32,
    pub cursor_style: CursorStyle,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            // cmux ships a dark UI by default, matching the macOS app.
            theme: Theme::Dark,
            font_family: "monospace".into(),
            font_size: 13.0,
            opacity: 1.0,
            cursor_style: CursorStyle::Block,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyle {
    Block,
    Bar,
    Underline,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Sidebar {
    pub position: SidebarPosition,
    pub width: f32,
    /// Show vertical tabs (cmux's signature layout) vs. a top tab bar.
    pub vertical_tabs: bool,
    pub show_notification_badges: bool,
}

impl Default for Sidebar {
    fn default() -> Self {
        Self {
            position: SidebarPosition::Left,
            width: 240.0,
            vertical_tabs: true,
            show_notification_badges: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarPosition {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Notifications {
    pub enabled: bool,
    /// Treat a terminal bell (BEL) as an attention signal → blue ring.
    pub ring_on_bell: bool,
    pub sound: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            enabled: true,
            ring_on_bell: true,
            sound: false,
        }
    }
}

impl Config {
    /// `$XDG_CONFIG_HOME/cmux/cmux.json`, falling back to `~/.config/cmux/cmux.json`.
    pub fn default_path() -> Result<PathBuf, ConfigError> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .ok_or(ConfigError::NoConfigDir)?;
        Ok(base.join("cmux").join("cmux.json"))
    }

    /// Load from `path`. A missing file yields [`Config::default`]. User-supplied
    /// shortcuts are merged over the defaults.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(path)?;
        Self::from_json(&text)
    }

    pub fn from_json(text: &str) -> Result<Config, ConfigError> {
        // Deserialize with `keyboard_shortcuts` defaulting to the full set, then
        // re-merge any user keys so partial shortcut maps don't wipe defaults.
        let raw: serde_json::Value = serde_json::from_str(text)?;
        let mut cfg: Config = serde_json::from_value(raw.clone())?;
        if let Some(user) = raw
            .get("keyboardShortcuts")
            .and_then(|v| v.as_object())
        {
            let mut merged = default_shortcuts();
            for (k, v) in user {
                if let Some(s) = v.as_str() {
                    merged.insert(k.clone(), s.to_string());
                }
            }
            cfg.keyboard_shortcuts = merged;
        }
        Ok(cfg)
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("Config serializes")
    }

    /// Write to `path`, creating parent directories.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, self.to_json_pretty())?;
        Ok(())
    }

    /// Read a value by dotted path, e.g. `"appearance.fontSize"`.
    pub fn get_path(&self, path: &str) -> Result<serde_json::Value, ConfigError> {
        let root = serde_json::to_value(self)?;
        let mut cur = &root;
        for seg in path.split('.').filter(|s| !s.is_empty()) {
            cur = cur
                .get(seg)
                .ok_or_else(|| ConfigError::NoSuchPath(path.to_string()))?;
        }
        Ok(cur.clone())
    }

    /// Set a value by dotted path. The string `raw` is parsed as JSON if it
    /// parses (so `13`, `true`, `"dark"` work), otherwise treated as a string.
    /// Re-validates by round-tripping through [`Config`].
    pub fn set_path(&mut self, path: &str, raw: &str) -> Result<(), ConfigError> {
        let value = serde_json::from_str::<serde_json::Value>(raw)
            .unwrap_or_else(|_| serde_json::Value::String(raw.to_string()));
        let mut root = serde_json::to_value(&*self)?;
        let segs: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
        if segs.is_empty() {
            return Err(ConfigError::NoSuchPath(path.to_string()));
        }
        let mut cur = &mut root;
        for seg in &segs[..segs.len() - 1] {
            cur = cur
                .get_mut(*seg)
                .ok_or_else(|| ConfigError::NoSuchPath(path.to_string()))?;
        }
        let obj = cur
            .as_object_mut()
            .ok_or_else(|| ConfigError::NoSuchPath(path.to_string()))?;
        obj.insert(segs[segs.len() - 1].to_string(), value);
        *self = serde_json::from_value(root)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_object_loads_defaults() {
        let cfg = Config::from_json("{}").unwrap();
        assert_eq!(cfg, Config::default());
        assert!(!cfg.keyboard_shortcuts.is_empty());
    }

    #[test]
    fn partial_json_overrides_only_specified_fields() {
        let cfg = Config::from_json(r#"{"appearance":{"fontSize":18}}"#).unwrap();
        assert_eq!(cfg.appearance.font_size, 18.0);
        // Untouched fields keep defaults.
        assert_eq!(cfg.appearance.font_family, "monospace");
        assert_eq!(cfg.sidebar.vertical_tabs, true);
    }

    #[test]
    fn user_shortcuts_merge_over_defaults() {
        let n_defaults = default_shortcuts().len();
        let cfg = Config::from_json(r#"{"keyboardShortcuts":{"newTab":"ctrl+shift+t"}}"#).unwrap();
        assert_eq!(cfg.keyboard_shortcuts.get("newTab").unwrap(), "ctrl+shift+t");
        // Other defaults survive.
        assert_eq!(cfg.keyboard_shortcuts.len(), n_defaults);
        assert!(cfg.keyboard_shortcuts.contains_key("splitHorizontal"));
    }

    #[test]
    fn get_path_reads_nested_value() {
        let cfg = Config::default();
        assert_eq!(cfg.get_path("appearance.fontSize").unwrap(), serde_json::json!(13.0));
        assert_eq!(cfg.get_path("sidebar.verticalTabs").unwrap(), serde_json::json!(true));
    }

    #[test]
    fn get_path_unknown_errors() {
        let cfg = Config::default();
        assert!(cfg.get_path("appearance.nope").is_err());
    }

    #[test]
    fn set_path_parses_json_scalars() {
        let mut cfg = Config::default();
        cfg.set_path("appearance.fontSize", "20").unwrap();
        assert_eq!(cfg.appearance.font_size, 20.0);
        cfg.set_path("appearance.theme", "dark").unwrap();
        assert_eq!(cfg.appearance.theme, Theme::Dark);
        cfg.set_path("notifications.sound", "true").unwrap();
        assert!(cfg.notifications.sound);
    }

    #[test]
    fn set_path_rejects_invalid_value() {
        let mut cfg = Config::default();
        // theme only accepts system/light/dark
        assert!(cfg.set_path("appearance.theme", "neon").is_err());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cmux").join("cmux.json");
        let mut cfg = Config::default();
        cfg.set_path("appearance.fontSize", "15").unwrap();
        cfg.save(&path).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.appearance.font_size, 15.0);
    }

    #[test]
    fn missing_file_is_default() {
        let cfg = Config::load(Path::new("/nonexistent/cmux/cmux.json")).unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn set_path_rebinds_a_keyboard_shortcut() {
        // Backs the in-app Settings shortcut editor (generic JSON-path set).
        let mut cfg = Config::default();
        cfg.set_path("keyboardShortcuts.newTab", "ctrl+alt+t").unwrap();
        assert_eq!(
            cfg.keyboard_shortcuts.get("newTab").map(String::as_str),
            Some("ctrl+alt+t")
        );
        // Other bindings are untouched.
        assert!(cfg.keyboard_shortcuts.contains_key("splitHorizontal"));
    }

    #[test]
    fn custom_actions_deserialize() {
        let json = r#"{"actions":{"deploy":{"command":"make deploy","label":"Deploy","target":"currentPane"},"logs":{"command":"journalctl -f"}}}"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        let deploy = cfg.actions.get("deploy").unwrap();
        assert_eq!(deploy.command, "make deploy");
        assert_eq!(deploy.label.as_deref(), Some("Deploy"));
        assert_eq!(deploy.target, ActionTarget::CurrentPane);
        // Defaults: no label, target = newTab.
        let logs = cfg.actions.get("logs").unwrap();
        assert_eq!(logs.label, None);
        assert_eq!(logs.target, ActionTarget::NewTab);
    }
}
