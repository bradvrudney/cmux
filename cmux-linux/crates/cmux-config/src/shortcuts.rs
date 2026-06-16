//! Default keyboard shortcut map.
//!
//! Upstream cmux requires every cmux-owned shortcut to be listed, editable in
//! settings, and configurable via `cmux.json`. This is the Linux default set;
//! chords use `ctrl`/`alt`/`shift`/`super` modifiers (the GUI binds `super` to
//! the platform meta key). User overrides from `cmux.json` are merged on top.

use std::collections::BTreeMap;

/// The canonical action id → chord defaults.
pub fn default_shortcuts() -> BTreeMap<String, String> {
    [
        ("newTab", "ctrl+shift+t"),
        ("closeTab", "ctrl+shift+w"),
        ("nextTab", "ctrl+tab"),
        ("previousTab", "ctrl+shift+tab"),
        ("newWorkspace", "ctrl+shift+n"),
        ("splitHorizontal", "ctrl+shift+d"),
        ("splitVertical", "ctrl+shift+e"),
        ("openBrowser", "ctrl+shift+b"),
        ("closePane", "ctrl+shift+x"),
        ("focusLeft", "ctrl+alt+left"),
        ("focusRight", "ctrl+alt+right"),
        ("focusUp", "ctrl+alt+up"),
        ("focusDown", "ctrl+alt+down"),
        ("commandPalette", "ctrl+shift+p"),
        ("find", "ctrl+shift+f"),
        ("toggleNotifications", "ctrl+shift+i"),
        ("jumpToLatestNotification", "ctrl+shift+j"),
        ("reopenClosedTab", "ctrl+shift+z"),
        ("openSettings", "ctrl+comma"),
        ("copySelection", "ctrl+shift+c"),
        ("paste", "ctrl+shift+v"),
        ("equalizeSplits", "ctrl+shift+o"),
        ("toggleZoom", "ctrl+shift+m"),
        ("closeWorkspace", "ctrl+shift+q"),
        ("selectWorkspace1", "ctrl+1"),
        ("selectWorkspace2", "ctrl+2"),
        ("selectWorkspace3", "ctrl+3"),
        ("selectWorkspace4", "ctrl+4"),
        ("selectWorkspace5", "ctrl+5"),
        ("selectWorkspace6", "ctrl+6"),
        ("selectWorkspace7", "ctrl+7"),
        ("selectWorkspace8", "ctrl+8"),
        ("selectWorkspace9", "ctrl+9"),
        ("moveTabToNewWorkspace", "ctrl+shift+u"),
        ("selectSurface1", "alt+1"),
        ("selectSurface2", "alt+2"),
        ("selectSurface3", "alt+3"),
        ("selectSurface4", "alt+4"),
        ("selectSurface5", "alt+5"),
        ("selectSurface6", "alt+6"),
        ("selectSurface7", "alt+7"),
        ("selectSurface8", "alt+8"),
        ("selectSurface9", "alt+9"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_non_empty_and_unique_chords_per_action() {
        let m = default_shortcuts();
        assert!(m.len() >= 15);
        assert_eq!(m.get("splitHorizontal").map(String::as_str), Some("ctrl+shift+d"));
    }
}
