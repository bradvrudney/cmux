//! Workspaces and the top-level [`AppState`] that ties the whole topology
//! together, plus the mutating operations the GUI and control socket invoke.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ids::{IdGen, PaneId, TabId, WorkspaceId};
use crate::notification::{NotificationFeed, RingState};
use crate::pane::Pane;
use crate::split::{FocusDir, Orientation};
use crate::tab::Tab;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub title: String,
    pub cwd: Option<String>,
    pub tabs: Vec<Tab>,
    pub active_tab: Option<TabId>,
}

impl Workspace {
    pub fn active_tab(&self) -> Option<&Tab> {
        let id = self.active_tab?;
        self.tabs.iter().find(|t| t.id == id)
    }
}

/// A tab the user closed, retained so it can be reopened (upstream's
/// closed-item history). Stores enough to recreate an empty tab in place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClosedTab {
    pub workspace: WorkspaceId,
    pub title: String,
}

/// The entire application topology.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppState {
    pub workspaces: Vec<Workspace>,
    pub active_workspace: Option<WorkspaceId>,
    pub panes: HashMap<PaneId, Pane>,
    pub notifications: NotificationFeed,
    #[serde(default)]
    closed_tabs: Vec<ClosedTab>,
    ids: IdGen,
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- creation -------------------------------------------------------

    /// Create a workspace seeded with one tab containing one terminal pane.
    pub fn new_workspace(&mut self, title: impl Into<String>) -> WorkspaceId {
        let ws_id = self.ids.workspace();
        let pane_id = self.ids.pane();
        let tab_id = self.ids.tab();
        self.panes.insert(pane_id, Pane::terminal(pane_id));
        let tab = Tab::new(tab_id, "1", pane_id);
        self.workspaces.push(Workspace {
            id: ws_id,
            title: title.into(),
            cwd: None,
            tabs: vec![tab],
            active_tab: Some(tab_id),
        });
        if self.active_workspace.is_none() {
            self.active_workspace = Some(ws_id);
        }
        ws_id
    }

    /// Add a new tab (one terminal pane) to a workspace and focus it.
    pub fn add_tab(&mut self, ws: WorkspaceId) -> Option<TabId> {
        let pane_id = self.ids.pane();
        let tab_id = self.ids.tab();
        let title = {
            let w = self.workspace_mut(ws)?;
            (w.tabs.len() + 1).to_string()
        };
        self.panes.insert(pane_id, Pane::terminal(pane_id));
        let w = self.workspace_mut(ws)?;
        w.tabs.push(Tab::new(tab_id, title, pane_id));
        w.active_tab = Some(tab_id);
        Some(tab_id)
    }

    // ---- lookups --------------------------------------------------------

    pub fn workspace(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.id == id)
    }
    fn workspace_mut(&mut self, id: WorkspaceId) -> Option<&mut Workspace> {
        self.workspaces.iter_mut().find(|w| w.id == id)
    }
    pub fn active_workspace(&self) -> Option<&Workspace> {
        self.workspace(self.active_workspace?)
    }
    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes.get(&id)
    }

    /// The pane focused in the active tab of the active workspace.
    pub fn focused_pane(&self) -> Option<PaneId> {
        self.active_workspace()?.active_tab()?.focused
    }

    /// Locate the workspace a tab belongs to.
    pub fn locate_tab_workspace(&self, tab: TabId) -> Option<WorkspaceId> {
        self.workspaces
            .iter()
            .find(|w| w.tabs.iter().any(|t| t.id == tab))
            .map(|w| w.id)
    }

    /// Locate the (workspace, tab) a pane lives in.
    pub fn locate_pane(&self, pane: PaneId) -> Option<(WorkspaceId, TabId)> {
        for w in &self.workspaces {
            for t in &w.tabs {
                if t.tree.contains(pane) {
                    return Some((w.id, t.id));
                }
            }
        }
        None
    }

    // ---- focus ----------------------------------------------------------

    pub fn focus_workspace(&mut self, ws: WorkspaceId) -> bool {
        if self.workspace(ws).is_some() {
            self.active_workspace = Some(ws);
            true
        } else {
            false
        }
    }

    pub fn focus_tab(&mut self, ws: WorkspaceId, tab: TabId) -> bool {
        match self.workspace_mut(ws) {
            Some(w) if w.tabs.iter().any(|t| t.id == tab) => {
                w.active_tab = Some(tab);
                self.active_workspace = Some(ws);
                true
            }
            _ => false,
        }
    }

    /// Focus the next (`forward`) or previous tab in the active workspace,
    /// wrapping around. Returns `false` if there are fewer than two tabs.
    pub fn focus_adjacent_tab(&mut self, forward: bool) -> bool {
        let Some(ws) = self.active_workspace else {
            return false;
        };
        let Some(w) = self.workspace_mut(ws) else {
            return false;
        };
        let n = w.tabs.len();
        if n < 2 {
            return false;
        }
        let cur = w
            .active_tab
            .and_then(|id| w.tabs.iter().position(|t| t.id == id))
            .unwrap_or(0);
        let next = if forward {
            (cur + 1) % n
        } else {
            (cur + n - 1) % n
        };
        w.active_tab = Some(w.tabs[next].id);
        true
    }

    /// Focus the workspace at 0-based `index` in sidebar order (drives the
    /// "select workspace 1–9" shortcuts). Returns `false` if out of range.
    pub fn focus_workspace_index(&mut self, index: usize) -> bool {
        match self.workspaces.get(index).map(|w| w.id) {
            Some(id) => {
                self.active_workspace = Some(id);
                true
            }
            None => false,
        }
    }

    /// Focus a pane, clearing its ring and marking its notifications read.
    pub fn focus_pane(&mut self, pane: PaneId) -> bool {
        let Some((ws, tab)) = self.locate_pane(pane) else {
            return false;
        };
        self.active_workspace = Some(ws);
        if let Some(w) = self.workspace_mut(ws) {
            w.active_tab = Some(tab);
            if let Some(t) = w.tabs.iter_mut().find(|t| t.id == tab) {
                t.focused = Some(pane);
            }
        }
        if let Some(p) = self.panes.get_mut(&pane) {
            p.ring = RingState::Idle;
        }
        self.notifications.mark_pane_read(pane);
        true
    }

    /// Move focus spatially within the active tab.
    pub fn focus_dir(&mut self, dir: FocusDir) -> bool {
        let Some(focused) = self.focused_pane() else {
            return false;
        };
        let Some(w) = self.active_workspace() else {
            return false;
        };
        let Some(t) = w.active_tab() else { return false };
        if let Some(next) = t.tree.neighbor(focused, dir) {
            self.focus_pane(next)
        } else {
            false
        }
    }

    // ---- splits & pane lifecycle ---------------------------------------

    /// Split the focused pane in the active tab, returning the new pane id.
    pub fn split_focused(&mut self, orientation: Orientation) -> Option<PaneId> {
        let focused = self.focused_pane()?;
        self.split_pane(focused, orientation)
    }

    pub fn split_pane(&mut self, target: PaneId, orientation: Orientation) -> Option<PaneId> {
        let (ws, tab) = self.locate_pane(target)?;
        let new_pane = self.ids.pane();
        let ok = {
            let w = self.workspace_mut(ws)?;
            let t = w.tabs.iter_mut().find(|t| t.id == tab)?;
            let ok = t.tree.split(target, new_pane, orientation, false);
            if ok {
                t.focused = Some(new_pane);
            }
            ok
        };
        if ok {
            self.panes.insert(new_pane, Pane::terminal(new_pane));
            Some(new_pane)
        } else {
            None
        }
    }

    /// Split the focused pane into a new browser pane showing `url`.
    pub fn split_focused_browser(
        &mut self,
        url: impl Into<String>,
        orientation: Orientation,
    ) -> Option<PaneId> {
        let id = self.split_focused(orientation)?;
        if let Some(p) = self.panes.get_mut(&id) {
            *p = crate::pane::Pane::browser(id, url);
        }
        Some(id)
    }

    /// Navigate a browser pane to `url`. Returns `false` if the pane isn't a
    /// browser pane (or doesn't exist).
    pub fn set_browser_url(&mut self, pane: PaneId, url: impl Into<String>) -> bool {
        match self.panes.get_mut(&pane) {
            Some(p) if p.is_browser() => {
                p.kind = crate::pane::PaneKind::Browser { url: url.into() };
                true
            }
            _ => false,
        }
    }

    /// Reset divider ratios of the active tab to even splits.
    pub fn equalize_active(&mut self) -> bool {
        let Some(ws) = self.active_workspace else {
            return false;
        };
        let Some(w) = self.workspace_mut(ws) else {
            return false;
        };
        let Some(tab) = w.active_tab else {
            return false;
        };
        match w.tabs.iter_mut().find(|t| t.id == tab) {
            Some(t) => {
                t.tree.equalize();
                true
            }
            None => false,
        }
    }

    /// Toggle zoom (maximize) of the focused pane in the active tab. Returns
    /// `true` if there is an active tab to toggle.
    pub fn toggle_zoom(&mut self) -> bool {
        let Some(ws) = self.active_workspace else {
            return false;
        };
        let focused = self.focused_pane();
        let Some(w) = self.workspace_mut(ws) else {
            return false;
        };
        let Some(tab) = w.active_tab else {
            return false;
        };
        match w.tabs.iter_mut().find(|t| t.id == tab) {
            Some(t) => {
                t.zoomed = if t.zoomed.is_some() { None } else { focused };
                true
            }
            None => false,
        }
    }

    /// The pane the active tab is zoomed to, if any and still present.
    pub fn zoomed_pane(&self) -> Option<PaneId> {
        let t = self.active_workspace()?.active_tab()?;
        let z = t.zoomed?;
        if t.tree.contains(z) {
            Some(z)
        } else {
            None
        }
    }

    /// Close a pane. If its tab becomes empty the tab is closed too.
    pub fn close_pane(&mut self, pane: PaneId) -> bool {
        let Some((ws, tab)) = self.locate_pane(pane) else {
            return false;
        };
        let (removed, tab_empty, next_focus) = {
            let Some(w) = self.workspace_mut(ws) else {
                return false;
            };
            let Some(t) = w.tabs.iter_mut().find(|t| t.id == tab) else {
                return false;
            };
            let removed = t.tree.close(pane);
            let next_focus = t.tree.first_leaf();
            if t.focused == Some(pane) {
                t.focused = next_focus;
            }
            if t.zoomed == Some(pane) {
                t.zoomed = None;
            }
            (removed, t.tree.is_empty(), next_focus)
        };
        if removed {
            self.panes.remove(&pane);
            self.notifications.prune_pane(pane);
            let _ = next_focus;
            if tab_empty {
                self.close_tab(ws, tab);
            }
        }
        removed
    }

    /// Close a tab, pruning its panes and recording it in closed history.
    pub fn close_tab(&mut self, ws: WorkspaceId, tab: TabId) -> bool {
        let Some(w) = self.workspace_mut(ws) else {
            return false;
        };
        let Some(idx) = w.tabs.iter().position(|t| t.id == tab) else {
            return false;
        };
        let removed = w.tabs.remove(idx);
        let title = removed.title.clone();
        for p in removed.panes() {
            self.panes.remove(&p);
            self.notifications.prune_pane(p);
        }
        // Re-point the active tab if we removed it.
        let w = self.workspace_mut(ws).expect("workspace exists");
        if w.active_tab == Some(tab) {
            let new_idx = idx.min(w.tabs.len().saturating_sub(1));
            w.active_tab = w.tabs.get(new_idx).map(|t| t.id);
        }
        self.closed_tabs.push(ClosedTab {
            workspace: ws,
            title,
        });
        true
    }

    /// Reopen the most recently closed tab (in its original workspace if it
    /// still exists, otherwise the active one). Returns the new tab id.
    pub fn reopen_closed_tab(&mut self) -> Option<TabId> {
        let closed = self.closed_tabs.pop()?;
        let ws = if self.workspace(closed.workspace).is_some() {
            closed.workspace
        } else {
            self.active_workspace?
        };
        self.add_tab(ws)
    }

    pub fn closed_tab_count(&self) -> usize {
        self.closed_tabs.len()
    }

    // ---- tab / workspace ordering --------------------------------------

    /// Close a workspace, pruning its panes and notifications. Re-points the
    /// active workspace if the closed one was active. Returns `true` if it
    /// existed.
    pub fn close_workspace(&mut self, ws: WorkspaceId) -> bool {
        let Some(idx) = self.workspaces.iter().position(|w| w.id == ws) else {
            return false;
        };
        let removed = self.workspaces.remove(idx);
        for t in &removed.tabs {
            for p in t.panes() {
                self.panes.remove(&p);
                self.notifications.prune_pane(p);
            }
        }
        if self.active_workspace == Some(ws) {
            let new_idx = idx.min(self.workspaces.len().saturating_sub(1));
            self.active_workspace = self.workspaces.get(new_idx).map(|w| w.id);
        }
        true
    }

    /// Move a workspace to a new index in sidebar order.
    pub fn reorder_workspace(&mut self, ws: WorkspaceId, to: usize) -> bool {
        let Some(from) = self.workspaces.iter().position(|w| w.id == ws) else {
            return false;
        };
        let to = to.min(self.workspaces.len().saturating_sub(1));
        let w = self.workspaces.remove(from);
        self.workspaces.insert(to, w);
        true
    }

    /// Move a tab to a new index within its workspace.
    pub fn reorder_tab(&mut self, ws: WorkspaceId, tab: TabId, to: usize) -> bool {
        let Some(w) = self.workspace_mut(ws) else {
            return false;
        };
        let Some(from) = w.tabs.iter().position(|t| t.id == tab) else {
            return false;
        };
        let to = to.min(w.tabs.len() - 1);
        let t = w.tabs.remove(from);
        w.tabs.insert(to, t);
        true
    }

    /// Set the ratio of a divider (by pre-order split index) in the active tab.
    pub fn set_active_divider(&mut self, split_index: usize, ratio: f32) -> bool {
        let Some(ws) = self.active_workspace else {
            return false;
        };
        let Some(w) = self.workspace_mut(ws) else {
            return false;
        };
        let Some(tab) = w.active_tab else {
            return false;
        };
        match w.tabs.iter_mut().find(|t| t.id == tab) {
            Some(t) => t.tree.set_ratio_by_index(split_index, ratio),
            None => false,
        }
    }

    /// Rename a tab. Returns `true` if the tab exists.
    pub fn rename_tab(&mut self, tab: TabId, title: impl Into<String>) -> bool {
        for w in &mut self.workspaces {
            if let Some(t) = w.tabs.iter_mut().find(|t| t.id == tab) {
                t.title = title.into();
                return true;
            }
        }
        false
    }

    /// Rename a workspace. Returns `true` if it exists.
    pub fn rename_workspace(&mut self, ws: WorkspaceId, title: impl Into<String>) -> bool {
        match self.workspace_mut(ws) {
            Some(w) => {
                w.title = title.into();
                true
            }
            None => false,
        }
    }

    // ---- notifications --------------------------------------------------

    /// Raise an attention notification on a pane and set its ring. Returns the
    /// notification id, or `None` if the pane is unknown.
    pub fn notify(
        &mut self,
        pane: PaneId,
        title: impl Into<String>,
        body: impl Into<String>,
        now_ms: u64,
    ) -> Option<u64> {
        let (ws, tab) = self.locate_pane(pane)?;
        let is_focused = self.focused_pane() == Some(pane)
            && self.active_workspace == Some(ws)
            && self.workspace(ws).and_then(|w| w.active_tab) == Some(tab);
        if let Some(p) = self.panes.get_mut(&pane) {
            // A notification for the already-focused pane doesn't nag.
            p.ring = if is_focused {
                RingState::Idle
            } else {
                RingState::Attention
            };
        }
        let id = self.notifications.push(ws, tab, pane, title, body, now_ms);
        if is_focused {
            self.notifications.mark_pane_read(pane);
        }
        Some(id)
    }

    /// Mark a single notification (by id) read. Returns `true` if it exists.
    pub fn mark_notification_read(&mut self, id: u64) -> bool {
        self.notifications.mark_read(id)
    }

    /// Remove a single notification by id. Returns `true` if it existed.
    pub fn dismiss_notification(&mut self, id: u64) -> bool {
        self.notifications.dismiss(id)
    }

    pub fn set_ring(&mut self, pane: PaneId, ring: RingState) -> bool {
        match self.panes.get_mut(&pane) {
            Some(p) => {
                p.ring = ring;
                true
            }
            None => false,
        }
    }

    /// True if any pane in the tab is showing an attention ring.
    pub fn tab_has_attention(&self, ws: WorkspaceId, tab: TabId) -> bool {
        self.workspace(ws)
            .and_then(|w| w.tabs.iter().find(|t| t.id == tab))
            .map(|t| {
                t.panes()
                    .iter()
                    .any(|p| self.panes.get(p).map_or(false, |p| p.ring.is_attention()))
            })
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> (AppState, WorkspaceId) {
        let mut s = AppState::new();
        let ws = s.new_workspace("proj");
        (s, ws)
    }

    #[test]
    fn new_workspace_has_one_tab_one_pane_and_is_active() {
        let (s, ws) = seeded();
        assert_eq!(s.active_workspace, Some(ws));
        let w = s.workspace(ws).unwrap();
        assert_eq!(w.tabs.len(), 1);
        assert_eq!(w.tabs[0].panes().len(), 1);
        assert!(s.focused_pane().is_some());
    }

    #[test]
    fn add_tab_focuses_new_tab() {
        let (mut s, ws) = seeded();
        let t2 = s.add_tab(ws).unwrap();
        assert_eq!(s.workspace(ws).unwrap().active_tab, Some(t2));
        assert_eq!(s.workspace(ws).unwrap().tabs.len(), 2);
    }

    #[test]
    fn split_creates_and_focuses_new_pane() {
        let (mut s, _ws) = seeded();
        let before = s.focused_pane().unwrap();
        let new = s.split_focused(Orientation::Horizontal).unwrap();
        assert_ne!(before, new);
        assert_eq!(s.focused_pane(), Some(new));
        assert!(s.pane(new).is_some());
    }

    #[test]
    fn close_pane_collapses_and_refocuses() {
        let (mut s, _ws) = seeded();
        let first = s.focused_pane().unwrap();
        let second = s.split_focused(Orientation::Horizontal).unwrap();
        assert!(s.close_pane(second));
        assert_eq!(s.focused_pane(), Some(first));
        assert!(s.pane(second).is_none());
    }

    #[test]
    fn closing_last_pane_in_tab_closes_the_tab() {
        let (mut s, ws) = seeded();
        let t2 = s.add_tab(ws).unwrap();
        let pane = s.workspace(ws).unwrap().active_tab().unwrap().focused.unwrap();
        assert_eq!(s.workspace(ws).unwrap().tabs.len(), 2);
        assert!(s.close_pane(pane));
        assert_eq!(s.workspace(ws).unwrap().tabs.len(), 1);
        assert_ne!(s.workspace(ws).unwrap().active_tab, Some(t2));
        assert_eq!(s.closed_tab_count(), 1);
    }

    #[test]
    fn reopen_closed_tab_restores_a_tab() {
        let (mut s, ws) = seeded();
        let t2 = s.add_tab(ws).unwrap();
        s.close_tab(ws, t2);
        assert_eq!(s.workspace(ws).unwrap().tabs.len(), 1);
        let reopened = s.reopen_closed_tab().unwrap();
        assert_eq!(s.workspace(ws).unwrap().tabs.len(), 2);
        assert_eq!(s.workspace(ws).unwrap().active_tab, Some(reopened));
        assert_eq!(s.closed_tab_count(), 0);
    }

    #[test]
    fn notify_sets_ring_and_feed_when_unfocused() {
        let (mut s, ws) = seeded();
        // Make a second tab so the first tab's pane is not focused.
        let bg_pane = s.workspace(ws).unwrap().tabs[0].focused.unwrap();
        s.add_tab(ws);
        let id = s.notify(bg_pane, "Claude", "needs input", 123).unwrap();
        assert_eq!(id, 0);
        assert_eq!(s.pane(bg_pane).unwrap().ring, RingState::Attention);
        assert_eq!(s.notifications.unread_count(), 1);
        let (_w, t) = s.locate_pane(bg_pane).unwrap();
        assert!(s.tab_has_attention(ws, t));
    }

    #[test]
    fn notify_on_focused_pane_does_not_nag() {
        let (mut s, _ws) = seeded();
        let focused = s.focused_pane().unwrap();
        s.notify(focused, "Claude", "done", 1);
        assert_eq!(s.pane(focused).unwrap().ring, RingState::Idle);
        assert_eq!(s.notifications.unread_count(), 0);
    }

    #[test]
    fn focusing_pane_clears_ring_and_marks_read() {
        let (mut s, ws) = seeded();
        let bg_pane = s.workspace(ws).unwrap().tabs[0].focused.unwrap();
        s.add_tab(ws);
        s.notify(bg_pane, "Claude", "needs input", 1);
        assert_eq!(s.notifications.unread_count(), 1);
        assert!(s.focus_pane(bg_pane));
        assert_eq!(s.pane(bg_pane).unwrap().ring, RingState::Idle);
        assert_eq!(s.notifications.unread_count(), 0);
    }

    #[test]
    fn set_active_divider_resizes_split() {
        let (mut s, _ws) = seeded();
        s.split_focused(Orientation::Horizontal);
        assert!(s.set_active_divider(0, 0.3));
        let t = s.active_workspace().unwrap().active_tab().unwrap();
        let d = t.tree.dividers(crate::split::Rect::new(0.0, 0.0, 1.0, 1.0));
        assert_eq!(d[0].ratio, 0.3);
        assert!(!s.set_active_divider(5, 0.5));
    }

    #[test]
    fn split_into_browser_pane_and_navigate() {
        let (mut s, _ws) = seeded();
        let b = s.split_focused_browser("https://example.com", Orientation::Horizontal).unwrap();
        assert!(s.pane(b).unwrap().is_browser());
        assert_eq!(s.pane(b).unwrap().browser_url(), Some("https://example.com"));
        assert_eq!(s.focused_pane(), Some(b));
        assert!(s.set_browser_url(b, "https://docs.rs"));
        assert_eq!(s.pane(b).unwrap().browser_url(), Some("https://docs.rs"));
    }

    #[test]
    fn set_browser_url_rejects_terminal_pane() {
        let (mut s, _ws) = seeded();
        let term = s.focused_pane().unwrap();
        assert!(!s.set_browser_url(term, "https://x"));
    }

    #[test]
    fn rename_tab_and_workspace() {
        let (mut s, ws) = seeded();
        let t = s.workspace(ws).unwrap().tabs[0].id;
        assert!(s.rename_tab(t, "build"));
        assert_eq!(s.workspace(ws).unwrap().tabs[0].title, "build");
        assert!(s.rename_workspace(ws, "myproj"));
        assert_eq!(s.workspace(ws).unwrap().title, "myproj");
        assert!(!s.rename_tab(TabId(999), "x"));
    }

    #[test]
    fn reorder_tab_moves_it() {
        let (mut s, ws) = seeded();
        let t1 = s.workspace(ws).unwrap().tabs[0].id;
        let t2 = s.add_tab(ws).unwrap();
        assert!(s.reorder_tab(ws, t2, 0));
        let order: Vec<_> = s.workspace(ws).unwrap().tabs.iter().map(|t| t.id).collect();
        assert_eq!(order, vec![t2, t1]);
    }

    #[test]
    fn focus_adjacent_tab_wraps() {
        let (mut s, ws) = seeded();
        let t1 = s.workspace(ws).unwrap().tabs[0].id;
        let t2 = s.add_tab(ws).unwrap();
        // active is t2; next wraps to t1, prev wraps back to t2.
        assert!(s.focus_adjacent_tab(true));
        assert_eq!(s.workspace(ws).unwrap().active_tab, Some(t1));
        assert!(s.focus_adjacent_tab(false));
        assert_eq!(s.workspace(ws).unwrap().active_tab, Some(t2));
    }

    #[test]
    fn focus_adjacent_tab_needs_two_tabs() {
        let (mut s, _ws) = seeded();
        assert!(!s.focus_adjacent_tab(true));
    }

    #[test]
    fn focus_workspace_index_selects_by_position() {
        let (mut s, ws1) = seeded();
        let ws2 = s.new_workspace("second");
        assert!(s.focus_workspace_index(0));
        assert_eq!(s.active_workspace, Some(ws1));
        assert!(s.focus_workspace_index(1));
        assert_eq!(s.active_workspace, Some(ws2));
        assert!(!s.focus_workspace_index(9));
    }

    #[test]
    fn equalize_active_resets_dividers() {
        let (mut s, _ws) = seeded();
        s.split_focused(Orientation::Horizontal);
        assert!(s.set_active_divider(0, 0.2));
        assert!(s.equalize_active());
        let t = s.active_workspace().unwrap().active_tab().unwrap();
        let d = t.tree.dividers(crate::split::Rect::new(0.0, 0.0, 1.0, 1.0));
        assert_eq!(d[0].ratio, 0.5);
    }

    #[test]
    fn toggle_zoom_tracks_focused_pane() {
        let (mut s, _ws) = seeded();
        let a = s.focused_pane().unwrap();
        let b = s.split_focused(Orientation::Horizontal).unwrap();
        assert_eq!(s.zoomed_pane(), None);
        assert!(s.toggle_zoom());
        assert_eq!(s.zoomed_pane(), Some(b));
        assert!(s.toggle_zoom());
        assert_eq!(s.zoomed_pane(), None);
        // Zoom a, then close it: zoom clears and doesn't dangle.
        s.focus_pane(a);
        s.toggle_zoom();
        assert_eq!(s.zoomed_pane(), Some(a));
        s.close_pane(a);
        assert_eq!(s.zoomed_pane(), None);
    }

    #[test]
    fn close_workspace_prunes_and_repoints_active() {
        let (mut s, ws1) = seeded();
        let p1 = s.focused_pane().unwrap();
        let ws2 = s.new_workspace("second");
        s.focus_workspace(ws1);
        assert!(s.close_workspace(ws1));
        assert!(s.workspace(ws1).is_none());
        assert!(s.pane(p1).is_none());
        assert_eq!(s.active_workspace, Some(ws2));
        assert!(!s.close_workspace(WorkspaceId(999)));
    }

    #[test]
    fn reorder_workspace_moves_it() {
        let (mut s, ws1) = seeded();
        let ws2 = s.new_workspace("second");
        assert!(s.reorder_workspace(ws2, 0));
        let order: Vec<_> = s.workspaces.iter().map(|w| w.id).collect();
        assert_eq!(order, vec![ws2, ws1]);
    }

    #[test]
    fn dismiss_and_mark_one_notification() {
        let (mut s, ws) = seeded();
        let bg = s.workspace(ws).unwrap().tabs[0].focused.unwrap();
        s.add_tab(ws);
        let id = s.notify(bg, "a", "", 1).unwrap();
        let id2 = s.notify(bg, "b", "", 2).unwrap();
        assert!(s.mark_notification_read(id));
        assert_eq!(s.notifications.unread_count(), 1);
        assert!(s.dismiss_notification(id2));
        assert_eq!(s.notifications.entries().len(), 1);
    }

    #[test]
    fn focus_dir_moves_between_split_panes() {
        let (mut s, _ws) = seeded();
        let left = s.focused_pane().unwrap();
        let right = s.split_focused(Orientation::Horizontal).unwrap();
        assert_eq!(s.focused_pane(), Some(right));
        assert!(s.focus_dir(FocusDir::Left));
        assert_eq!(s.focused_pane(), Some(left));
        assert!(!s.focus_dir(FocusDir::Left));
    }

    #[test]
    fn state_roundtrips_through_json() {
        let (mut s, _ws) = seeded();
        s.split_focused(Orientation::Vertical);
        let json = serde_json::to_string(&s).unwrap();
        let back: AppState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.workspaces.len(), s.workspaces.len());
        assert_eq!(back.panes.len(), s.panes.len());
    }
}
