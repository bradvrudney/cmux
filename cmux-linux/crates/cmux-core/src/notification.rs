//! Notification rings and the pending-notification feed.
//!
//! In upstream cmux, a pane gets a blue ring and its tab lights up when a coding
//! agent needs attention; a notification panel lists everything pending and lets
//! you jump to the most recent unread. This module models that state.

use serde::{Deserialize, Serialize};

use crate::ids::{PaneId, TabId, WorkspaceId};

/// The visual ring state of a single pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RingState {
    /// No ring; nothing pending.
    #[default]
    Idle,
    /// Agent is actively working (subtle pulse in the UI).
    Busy,
    /// Agent wants attention — the signature blue ring.
    Attention,
}

impl RingState {
    pub fn is_attention(self) -> bool {
        matches!(self, RingState::Attention)
    }
}

/// A single entry in the notification feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    pub id: u64,
    pub workspace: WorkspaceId,
    pub tab: TabId,
    pub pane: PaneId,
    pub title: String,
    pub body: String,
    /// Wall-clock millis since the Unix epoch, supplied by the caller so the core
    /// stays free of `std::time` side effects and remains trivially testable.
    pub created_at_ms: u64,
    pub read: bool,
}

/// The ordered feed of notifications, newest last.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NotificationFeed {
    entries: Vec<Notification>,
    next_id: u64,
}

impl NotificationFeed {
    /// Push a new notification, returning its id.
    pub fn push(
        &mut self,
        workspace: WorkspaceId,
        tab: TabId,
        pane: PaneId,
        title: impl Into<String>,
        body: impl Into<String>,
        created_at_ms: u64,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push(Notification {
            id,
            workspace,
            tab,
            pane,
            title: title.into(),
            body: body.into(),
            created_at_ms,
            read: false,
        });
        id
    }

    pub fn entries(&self) -> &[Notification] {
        &self.entries
    }

    pub fn unread_count(&self) -> usize {
        self.entries.iter().filter(|n| !n.read).count()
    }

    /// Mark every notification for a pane as read. Returns how many changed.
    pub fn mark_pane_read(&mut self, pane: PaneId) -> usize {
        let mut changed = 0;
        for n in self.entries.iter_mut().filter(|n| n.pane == pane && !n.read) {
            n.read = true;
            changed += 1;
        }
        changed
    }

    pub fn mark_all_read(&mut self) -> usize {
        let mut changed = 0;
        for n in self.entries.iter_mut().filter(|n| !n.read) {
            n.read = true;
            changed += 1;
        }
        changed
    }

    /// The most recent unread notification, if any — what "jump to latest" targets.
    pub fn latest_unread(&self) -> Option<&Notification> {
        self.entries.iter().rev().find(|n| !n.read)
    }

    /// Drop notifications belonging to a pane that no longer exists.
    pub fn prune_pane(&mut self, pane: PaneId) {
        self.entries.retain(|n| n.pane != pane);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> (WorkspaceId, TabId, PaneId) {
        (WorkspaceId(1), TabId(2), PaneId(3))
    }

    #[test]
    fn push_increments_unread_and_assigns_ids() {
        let (w, t, p) = ids();
        let mut feed = NotificationFeed::default();
        let a = feed.push(w, t, p, "Claude", "needs input", 100);
        let b = feed.push(w, t, p, "Claude", "done", 200);
        assert_eq!((a, b), (0, 1));
        assert_eq!(feed.unread_count(), 2);
    }

    #[test]
    fn latest_unread_is_newest_first() {
        let (w, t, p) = ids();
        let mut feed = NotificationFeed::default();
        feed.push(w, t, p, "old", "", 1);
        feed.push(w, t, p, "new", "", 2);
        assert_eq!(feed.latest_unread().unwrap().title, "new");
    }

    #[test]
    fn marking_pane_read_clears_only_that_pane() {
        let (w, t, _) = ids();
        let mut feed = NotificationFeed::default();
        feed.push(w, t, PaneId(3), "a", "", 1);
        feed.push(w, t, PaneId(4), "b", "", 2);
        assert_eq!(feed.mark_pane_read(PaneId(3)), 1);
        assert_eq!(feed.unread_count(), 1);
        assert_eq!(feed.latest_unread().unwrap().pane, PaneId(4));
    }

    #[test]
    fn prune_pane_removes_entries() {
        let (w, t, p) = ids();
        let mut feed = NotificationFeed::default();
        feed.push(w, t, p, "a", "", 1);
        feed.prune_pane(p);
        assert!(feed.entries().is_empty());
    }
}
