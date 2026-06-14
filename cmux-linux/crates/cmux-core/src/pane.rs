//! A pane: one terminal surface within a tab's split tree.

use serde::{Deserialize, Serialize};

use crate::ids::PaneId;
use crate::notification::RingState;

/// What a pane hosts. Today only terminals; kept as an enum so a browser pane
/// (upstream's WKWebView surface) can be added without touching call sites.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneKind {
    Terminal { command: Option<String> },
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
}
