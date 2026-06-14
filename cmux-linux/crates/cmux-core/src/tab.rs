//! A tab: a named split tree of panes, shown as one entry in the vertical sidebar.

use serde::{Deserialize, Serialize};

use crate::ids::{PaneId, TabId};
use crate::split::SplitTree;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    pub id: TabId,
    pub title: String,
    pub tree: SplitTree,
    /// The focused pane within this tab. Kept in sync as panes open/close.
    pub focused: Option<PaneId>,
}

impl Tab {
    pub fn new(id: TabId, title: impl Into<String>, root_pane: PaneId) -> Self {
        Self {
            id,
            title: title.into(),
            tree: SplitTree::single(root_pane),
            focused: Some(root_pane),
        }
    }

    pub fn panes(&self) -> Vec<PaneId> {
        self.tree.leaves()
    }

    pub fn is_empty(&self) -> bool {
        self.tree.is_empty()
    }
}
