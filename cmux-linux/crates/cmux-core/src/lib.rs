//! `cmux-core` — the topology model for cmux-linux.
//!
//! This crate is pure data and logic: no I/O, no async, no UI. It owns the
//! [`AppState`] tree of workspaces → tabs → split tree → panes, the focus
//! model, and the notification feed/rings. The GUI and the control socket are
//! thin layers that call into the operations defined here, which keeps the
//! interesting behavior unit-testable without a display server or a shell.

pub mod ids;
pub mod notification;
pub mod pane;
pub mod split;
pub mod tab;
pub mod workspace;

pub use ids::{IdGen, PaneId, TabId, WorkspaceId};
pub use notification::{Notification, NotificationFeed, RingState};
pub use pane::{Pane, PaneKind};
pub use split::{FocusDir, Node, Orientation, Rect, SplitTree};
pub use tab::Tab;
pub use workspace::{AppState, ClosedTab, Workspace};
