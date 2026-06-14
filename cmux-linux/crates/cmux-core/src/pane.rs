//! A pane: one terminal surface within a tab's split tree.

use serde::{Deserialize, Serialize};

use crate::ids::PaneId;
use crate::notification::RingState;

/// What a pane hosts: a terminal (PTY-backed) or a browser (webview-backed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneKind {
    Terminal { command: Option<String> },
    Browser { url: String },
}

impl Default for PaneKind {
    fn default() -> Self {
        PaneKind::Terminal { command: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pane {
    pub id: PaneId,
    pub title: String,
    pub cwd: Option<String>,
    pub kind: PaneKind,
    pub ring: RingState,
}

impl Pane {
    pub fn terminal(id: PaneId) -> Self {
        Self {
            id,
            title: String::from("terminal"),
            cwd: None,
            kind: PaneKind::default(),
            ring: RingState::Idle,
        }
    }

    pub fn browser(id: PaneId, url: impl Into<String>) -> Self {
        Self {
            id,
            title: String::from("browser"),
            cwd: None,
            kind: PaneKind::Browser { url: url.into() },
            ring: RingState::Idle,
        }
    }

    pub fn is_browser(&self) -> bool {
        matches!(self.kind, PaneKind::Browser { .. })
    }

    /// The browser URL, if this is a browser pane.
    pub fn browser_url(&self) -> Option<&str> {
        match &self.kind {
            PaneKind::Browser { url } => Some(url),
            _ => None,
        }
    }
}
