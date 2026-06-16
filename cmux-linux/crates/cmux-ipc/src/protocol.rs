//! Wire types for the control socket.
//!
//! One JSON object per line (JSONL): the client writes a [`Request`] line and
//! reads back exactly one [`Response`] line. This mirrors upstream cmux's
//! control socket that `CMUXCLI` and agent hooks speak to drive the app.

use cmux_core::ids::{PaneId, TabId, WorkspaceId};
use cmux_core::split::Orientation;
use serde::{Deserialize, Serialize};

/// Where a focus/targeting request points.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "id")]
pub enum Target {
    Workspace(WorkspaceId),
    Tab(TabId),
    Pane(PaneId),
}

/// A direction for spatial focus movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// Split orientation as it appears on the wire (maps to [`Orientation`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDir {
    Horizontal,
    Vertical,
}

impl From<SplitDir> for Orientation {
    fn from(d: SplitDir) -> Self {
        match d {
            SplitDir::Horizontal => Orientation::Horizontal,
            SplitDir::Vertical => Orientation::Vertical,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "cmd")]
pub enum Request {
    /// Liveness check.
    Ping,
    /// Summarize the whole topology.
    ListWorkspaces,
    /// Write raw text to a pane (defaults to the focused pane).
    Send { pane: Option<PaneId>, data: String },
    /// Send a named key/chord to a pane (e.g. `"enter"`, `"ctrl+c"`).
    SendKey { pane: Option<PaneId>, key: String },
    /// Focus a workspace, tab, or pane.
    Focus { target: Target },
    /// Move focus spatially within the active tab.
    FocusDir { dir: Dir },
    /// Open a new tab in a workspace (defaults to the active workspace).
    NewTab { workspace: Option<WorkspaceId> },
    /// Create a new workspace.
    NewWorkspace { title: Option<String> },
    /// Split a pane (defaults to the focused pane).
    Split {
        pane: Option<PaneId>,
        orientation: SplitDir,
    },
    /// Close a pane.
    ClosePane { pane: PaneId },
    /// Raise an attention notification on a pane (sets the ring).
    Notify {
        pane: PaneId,
        title: String,
        body: String,
    },
    /// Plain-text snapshot of a pane's current screen.
    Snapshot { pane: PaneId },
    /// Read a config value by dotted path (whole config if `None`).
    GetConfig { path: Option<String> },
    /// Set a config value by dotted path.
    SetConfig { path: String, value: String },
    /// List the notification feed (newest last).
    ListNotifications,
    /// Mark every notification as read (clears the unread badge).
    MarkAllRead,
    /// Rename a tab.
    RenameTab { tab: TabId, title: String },
    /// Rename a workspace.
    RenameWorkspace { workspace: WorkspaceId, title: String },
    /// Move a tab to a new index within its workspace.
    ReorderTab { tab: TabId, index: usize },
    /// Resize a pane's PTY/grid (rows × cols).
    ResizePane { pane: PaneId, rows: u16, cols: u16 },
    /// Split the focused pane into a browser pane showing `url`.
    OpenBrowser { url: String, orientation: SplitDir },
    /// Navigate an existing browser pane to `url`.
    NavigateBrowser { pane: PaneId, url: String },
    /// Search a pane's scrollback + screen for `query`.
    Find { pane: PaneId, query: String },
    /// Close a workspace by id.
    CloseWorkspace { workspace: WorkspaceId },
    /// Move a workspace to a new 0-based sidebar index.
    ReorderWorkspace { workspace: WorkspaceId, index: usize },
    /// Reset divider ratios of the active tab to even splits.
    Equalize,
    /// Toggle zoom (maximize) of the focused pane in the active tab.
    ToggleZoom,
    /// Focus the next tab in the active workspace (wrapping).
    NextTab,
    /// Focus the previous tab in the active workspace (wrapping).
    PrevTab,
    /// Mark a single notification (by id) read.
    MarkNotificationRead { id: u64 },
    /// Remove a single notification by id.
    DismissNotification { id: u64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneSummary {
    pub id: PaneId,
    pub title: String,
    pub focused: bool,
    /// `"idle" | "busy" | "attention"`.
    pub ring: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabSummary {
    pub id: TabId,
    pub title: String,
    pub active: bool,
    pub attention: bool,
    pub panes: Vec<PaneSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    pub id: WorkspaceId,
    pub title: String,
    pub active: bool,
    pub tabs: Vec<TabSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum Response {
    Ok,
    Pong,
    Workspaces { workspaces: Vec<WorkspaceSummary> },
    Snapshot { text: String },
    Notifications {
        notifications: Vec<cmux_core::notification::Notification>,
    },
    ConfigValue { value: serde_json::Value },
    /// Search hits as (line, col) pairs.
    Matches { matches: Vec<(usize, usize)> },
    /// Returned when an operation succeeded and produced a new id.
    Created { id: u64 },
    Error { message: String },
}

impl Response {
    pub fn error(msg: impl Into<String>) -> Self {
        Response::Error {
            message: msg.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrips_as_tagged_json() {
        let r = Request::Split {
            pane: Some(PaneId(3)),
            orientation: SplitDir::Vertical,
        };
        let line = serde_json::to_string(&r).unwrap();
        assert!(line.contains("\"cmd\":\"split\""));
        let back: Request = serde_json::from_str(&line).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn response_roundtrips() {
        let r = Response::Created { id: 9 };
        let line = serde_json::to_string(&r).unwrap();
        let back: Response = serde_json::from_str(&line).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn target_tagged_form() {
        let t = Target::Pane(PaneId(5));
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["kind"], "pane");
        assert_eq!(v["id"], 5);
    }

    #[test]
    fn split_dir_maps_to_orientation() {
        assert_eq!(Orientation::from(SplitDir::Horizontal), Orientation::Horizontal);
        assert_eq!(Orientation::from(SplitDir::Vertical), Orientation::Vertical);
    }
}
