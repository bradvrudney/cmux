//! Command palette: a searchable list of cmux actions.
//!
//! The registry and the fuzzy filter are pure functions so they can be tested
//! without a UI; the GUI renders [`filter_actions`] output and executes the
//! chosen action through the same `Engine`/signal paths as keyboard shortcuts.

use std::collections::BTreeMap;

#[derive(Clone, PartialEq, Debug)]
pub struct PaletteAction {
    pub id: String,
    pub label: String,
    /// The chord currently bound to this action, if any (shown as a hint).
    pub shortcut: Option<String>,
}

/// Every action the palette can run, paired with its configured chord.
pub fn all_actions(shortcuts: &BTreeMap<String, String>) -> Vec<PaletteAction> {
    const DEFS: &[(&str, &str)] = &[
        ("splitHorizontal", "Split pane horizontally"),
        ("splitVertical", "Split pane vertically"),
        ("closePane", "Close pane"),
        ("newTab", "New tab"),
        ("closeTab", "Close tab"),
        ("reopenClosedTab", "Reopen closed tab"),
        ("newWorkspace", "New workspace"),
        ("focusLeft", "Focus pane left"),
        ("focusRight", "Focus pane right"),
        ("focusUp", "Focus pane up"),
        ("focusDown", "Focus pane down"),
        ("toggleNotifications", "Toggle notifications panel"),
        ("jumpToLatestNotification", "Jump to latest notification"),
    ];
    DEFS.iter()
        .map(|(id, label)| PaletteAction {
            id: (*id).to_string(),
            label: (*label).to_string(),
            shortcut: shortcuts.get(*id).cloned(),
        })
        .collect()
}

/// Filter and rank actions against a query (case-insensitive subsequence match;
/// empty query returns everything in registry order).
pub fn filter_actions(query: &str, actions: &[PaletteAction]) -> Vec<PaletteAction> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return actions.to_vec();
    }
    let mut scored: Vec<(i32, &PaletteAction)> = actions
        .iter()
        .filter_map(|a| score(&q, &a.label.to_lowercase()).map(|s| (s, a)))
        .collect();
    // Higher score first; stable for equal scores (preserves registry order).
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().map(|(_, a)| a.clone()).collect()
}

/// Subsequence score: matches earn points, contiguous and leading matches earn
/// bonuses. `None` if `query` is not a subsequence of `text`.
fn score(query: &str, text: &str) -> Option<i32> {
    let mut qi = query.chars().peekable();
    let mut total = 0;
    let mut last: Option<usize> = None;
    for (i, c) in text.chars().enumerate() {
        if let Some(&qc) = qi.peek() {
            if c == qc {
                qi.next();
                total += 10;
                if i == 0 {
                    total += 8;
                }
                if last.map_or(false, |l| i == l + 1) {
                    total += 5;
                }
                last = Some(i);
            }
        }
    }
    if qi.peek().is_none() {
        Some(total)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions() -> Vec<PaletteAction> {
        all_actions(&cmux_config::default_shortcuts())
    }

    #[test]
    fn registry_includes_core_actions_with_shortcuts() {
        let a = actions();
        let split = a.iter().find(|x| x.id == "splitHorizontal").unwrap();
        assert_eq!(split.shortcut.as_deref(), Some("ctrl+shift+d"));
    }

    #[test]
    fn empty_query_returns_all_in_order() {
        let a = actions();
        let f = filter_actions("", &a);
        assert_eq!(f.len(), a.len());
        assert_eq!(f[0].id, a[0].id);
    }

    #[test]
    fn query_matches_subsequence() {
        let a = actions();
        let f = filter_actions("split", &a);
        assert!(f.len() >= 2);
        assert!(f.iter().all(|x| x.label.to_lowercase().contains("split")));
    }

    #[test]
    fn contiguous_match_outranks_scattered() {
        let a = actions();
        // "new tab" should rank New tab above New workspace for query "newt".
        let f = filter_actions("newt", &a);
        assert_eq!(f.first().unwrap().id, "newTab");
    }

    #[test]
    fn nonmatching_query_is_empty() {
        let a = actions();
        assert!(filter_actions("zzzzz", &a).is_empty());
    }
}
