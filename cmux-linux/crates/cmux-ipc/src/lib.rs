//! `cmux-ipc` — the control protocol and Unix-socket transport for cmux-linux.
//!
//! The GUI runs a [`Server`] on a background thread and answers [`Request`]s by
//! locking its shared [`cmux_core::AppState`]; the `cmux` CLI is a [`Client`].
//! Keeping the wire types here (rather than in either binary) lets both sides
//! share one definition and lets the protocol be tested without a GUI.

pub mod protocol;
pub mod transport;

pub use protocol::{
    Dir, PaneSummary, Request, Response, SplitDir, TabSummary, Target, WorkspaceSummary,
};
pub use transport::{default_socket_path, Client, IpcError, Server};

use cmux_core::AppState;

/// Build the [`WorkspaceSummary`] list for a `ListWorkspaces` response from the
/// current [`AppState`]. Lives here so the GUI handler and tests share it.
pub fn summarize(state: &AppState) -> Vec<WorkspaceSummary> {
    let active_ws = state.active_workspace;
    state
        .workspaces
        .iter()
        .map(|w| WorkspaceSummary {
            id: w.id,
            title: w.title.clone(),
            active: Some(w.id) == active_ws,
            tabs: w
                .tabs
                .iter()
                .map(|t| TabSummary {
                    id: t.id,
                    title: t.title.clone(),
                    active: w.active_tab == Some(t.id),
                    attention: state.tab_has_attention(w.id, t.id),
                    panes: t
                        .panes()
                        .into_iter()
                        .map(|pid| {
                            let p = state.pane(pid);
                            PaneSummary {
                                id: pid,
                                title: p.map(|p| p.title.clone()).unwrap_or_default(),
                                focused: t.focused == Some(pid),
                                ring: match p.map(|p| p.ring) {
                                    Some(cmux_core::RingState::Attention) => "attention",
                                    Some(cmux_core::RingState::Busy) => "busy",
                                    _ => "idle",
                                }
                                .to_string(),
                            }
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmux_core::Orientation;

    #[test]
    fn summarize_reflects_topology() {
        let mut s = AppState::new();
        let ws = s.new_workspace("proj");
        s.split_focused(Orientation::Horizontal);
        let summary = summarize(&s);
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].id, ws);
        assert!(summary[0].active);
        assert_eq!(summary[0].tabs.len(), 1);
        assert_eq!(summary[0].tabs[0].panes.len(), 2);
        assert!(summary[0].tabs[0].panes.iter().filter(|p| p.focused).count() == 1);
    }
}
