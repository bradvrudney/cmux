//! The application engine: the authoritative model plus the live runtimes
//! (PTY + terminal emulator) backing each pane.
//!
//! The engine lives behind an `Arc<Mutex<…>>` shared by the Dioxus UI (which
//! reads it every render tick) and the control-socket thread (which mutates it
//! in response to `cmux` CLI commands). All state-changing logic funnels through
//! [`Engine`] so both entrypoints behave identically — the shared-behavior rule
//! the upstream project insists on.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::time::{SystemTime, UNIX_EPOCH};

use cmux_config::Config;
use cmux_core::ids::PaneId;
use cmux_core::split::{FocusDir, Orientation, Rect};
use cmux_core::{AppState, RingState};
use cmux_ipc::protocol::{Dir, Request, Response, Target};
use cmux_pty::{PtyConfig, PtyEvent, PtySession};
use cmux_term::Terminal;

/// Default grid size for a freshly spawned pane until pixel-accurate sizing
/// lands (tracked in ROADMAP).
const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;

/// The live runtime for one pane: its child process and its screen state.
struct PaneRuntime {
    pty: PtySession,
    term: Terminal,
    rx: Receiver<PtyEvent>,
    exited: bool,
}

impl PaneRuntime {
    fn is_alive(&self) -> bool {
        !self.exited
    }
}

pub struct Engine {
    pub state: AppState,
    pub config: Config,
    runtimes: HashMap<PaneId, PaneRuntime>,
}

impl Engine {
    /// Build an engine seeded with one workspace and spawn its pane's shell.
    pub fn new(config: Config) -> Self {
        let mut state = AppState::new();
        state.new_workspace("workspace");
        let mut engine = Engine {
            state,
            config,
            runtimes: HashMap::new(),
        };
        engine.ensure_runtimes();
        engine
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Spawn PTY+terminal runtimes for any pane in the model that lacks one.
    /// Called after every operation that can create panes, so GUI- and
    /// socket-initiated splits both get backed by a real shell.
    pub fn ensure_runtimes(&mut self) {
        let panes: Vec<PaneId> = self.state.panes.keys().copied().collect();
        for pane in panes {
            if self.runtimes.contains_key(&pane) {
                continue;
            }
            if let Some(rt) = self.spawn_runtime() {
                self.runtimes.insert(pane, rt);
            }
        }
        // Drop runtimes whose pane was closed.
        let live: Vec<PaneId> = self.runtimes.keys().copied().collect();
        for pane in live {
            if !self.state.panes.contains_key(&pane) {
                self.runtimes.remove(&pane);
            }
        }
    }

    fn spawn_runtime(&self) -> Option<PaneRuntime> {
        let mut cfg = PtyConfig::shell().with_size(DEFAULT_ROWS, DEFAULT_COLS);
        if let Some(shell) = &self.config.shell {
            cfg = PtyConfig::command([shell.clone()]).with_size(DEFAULT_ROWS, DEFAULT_COLS);
        }
        let mut pty = match PtySession::spawn(cfg) {
            Ok(p) => p,
            Err(_) => return None,
        };
        let rx = pty.take_events()?;
        Some(PaneRuntime {
            pty,
            term: Terminal::new(DEFAULT_ROWS as usize, DEFAULT_COLS as usize),
            rx,
            exited: false,
        })
    }

    /// Drain all PTY output into terminal grids and translate bells into
    /// attention notifications. Returns the number of bytes ingested (so the UI
    /// can decide whether anything changed).
    pub fn pump(&mut self) -> usize {
        let mut total = 0;
        let panes: Vec<PaneId> = self.runtimes.keys().copied().collect();
        let mut bells: Vec<PaneId> = Vec::new();
        for pane in panes {
            if let Some(rt) = self.runtimes.get_mut(&pane) {
                while let Ok(evt) = rt.rx.try_recv() {
                    match evt {
                        PtyEvent::Output(bytes) => {
                            total += bytes.len();
                            rt.term.feed(&bytes);
                        }
                        PtyEvent::Exited(_) => rt.exited = true,
                    }
                }
                if rt.term.take_bell() {
                    bells.push(pane);
                }
            }
        }
        if self.config.notifications.enabled && self.config.notifications.ring_on_bell {
            let now = Self::now_ms();
            for pane in bells {
                self.state.notify(pane, "Bell", "terminal bell", now);
            }
        }
        total
    }

    // ---- rendering accessors -------------------------------------------

    pub fn terminal(&self, pane: PaneId) -> Option<&Terminal> {
        self.runtimes.get(&pane).map(|r| &r.term)
    }

    pub fn pane_alive(&self, pane: PaneId) -> bool {
        self.runtimes.get(&pane).map(|r| r.is_alive()).unwrap_or(true)
    }

    // ---- input ----------------------------------------------------------

    /// Write raw bytes to a pane's PTY.
    pub fn write_pane(&mut self, pane: PaneId, bytes: &[u8]) -> bool {
        match self.runtimes.get(&pane) {
            Some(rt) => rt.pty.write(bytes).is_ok(),
            None => false,
        }
    }

    /// Write to the currently focused pane.
    pub fn write_focused(&mut self, bytes: &[u8]) -> bool {
        match self.state.focused_pane() {
            Some(p) => self.write_pane(p, bytes),
            None => false,
        }
    }

    // ---- topology operations (shared by GUI and socket) -----------------

    pub fn split_focused(&mut self, orientation: Orientation) -> Option<PaneId> {
        let new = self.state.split_focused(orientation);
        self.ensure_runtimes();
        new
    }

    pub fn new_tab(&mut self) -> Option<cmux_core::ids::TabId> {
        let ws = self.state.active_workspace?;
        let t = self.state.add_tab(ws);
        self.ensure_runtimes();
        t
    }

    pub fn new_workspace(&mut self, title: &str) -> cmux_core::ids::WorkspaceId {
        let ws = self.state.new_workspace(title);
        self.ensure_runtimes();
        ws
    }

    pub fn close_pane(&mut self, pane: PaneId) -> bool {
        let ok = self.state.close_pane(pane);
        self.ensure_runtimes();
        ok
    }

    /// Resize a pane's PTY and terminal grid to the given dimensions.
    pub fn resize_pane(&mut self, pane: PaneId, rows: u16, cols: u16) -> bool {
        let rows = rows.max(1);
        let cols = cols.max(1);
        match self.runtimes.get_mut(&pane) {
            Some(rt) => {
                let _ = rt.pty.resize(rows, cols);
                rt.term.resize(rows as usize, cols as usize);
                true
            }
            None => false,
        }
    }

    fn close_focused_pane(&mut self) -> bool {
        match self.state.focused_pane() {
            Some(p) => self.close_pane(p),
            None => false,
        }
    }

    fn close_active_tab(&mut self) -> bool {
        let target = self
            .state
            .active_workspace()
            .and_then(|w| w.active_tab.map(|t| (w.id, t)));
        match target {
            Some((ws, tab)) => {
                let ok = self.state.close_tab(ws, tab);
                self.ensure_runtimes();
                ok
            }
            None => false,
        }
    }

    fn reopen_closed_tab(&mut self) -> bool {
        let ok = self.state.reopen_closed_tab().is_some();
        self.ensure_runtimes();
        ok
    }

    /// Focus the pane of the most recent unread notification ("jump to latest").
    fn focus_latest_unread(&mut self) -> bool {
        match self.state.notifications.latest_unread().map(|n| n.pane) {
            Some(pane) => self.state.focus_pane(pane),
            None => false,
        }
    }

    /// Dispatch a configured keyboard-shortcut action id onto the model. Returns
    /// `true` if the action mutated state. UI-only actions (command palette,
    /// settings, notification panel) are handled by the GUI, not here, so this
    /// returns `false` for them.
    pub fn dispatch_action(&mut self, action: &str) -> bool {
        match action {
            "newTab" => self.new_tab().is_some(),
            "closeTab" => self.close_active_tab(),
            "newWorkspace" => {
                self.new_workspace("workspace");
                true
            }
            "splitHorizontal" => self.split_focused(Orientation::Horizontal).is_some(),
            "splitVertical" => self.split_focused(Orientation::Vertical).is_some(),
            "closePane" => self.close_focused_pane(),
            "focusLeft" => self.state.focus_dir(FocusDir::Left),
            "focusRight" => self.state.focus_dir(FocusDir::Right),
            "focusUp" => self.state.focus_dir(FocusDir::Up),
            "focusDown" => self.state.focus_dir(FocusDir::Down),
            "reopenClosedTab" => self.reopen_closed_tab(),
            "jumpToLatestNotification" => self.focus_latest_unread(),
            _ => false,
        }
    }

    // ---- control socket dispatch ---------------------------------------

    /// Map an IPC [`Request`] onto engine operations. Used by the socket server
    /// thread; identical code path to the GUI's own actions.
    pub fn handle_request(&mut self, req: Request) -> Response {
        match req {
            Request::Ping => Response::Pong,
            Request::ListWorkspaces => Response::Workspaces {
                workspaces: cmux_ipc::summarize(&self.state),
            },
            Request::Send { pane, data } => {
                let ok = match pane {
                    Some(p) => self.write_pane(p, data.as_bytes()),
                    None => self.write_focused(data.as_bytes()),
                };
                if ok {
                    Response::Ok
                } else {
                    Response::error("no such pane")
                }
            }
            Request::SendKey { pane, key } => {
                let bytes = crate::keys::chord_to_bytes(&key);
                let target = pane.or_else(|| self.state.focused_pane());
                match (target, bytes) {
                    (Some(p), Some(b)) => {
                        if self.write_pane(p, &b) {
                            Response::Ok
                        } else {
                            Response::error("no such pane")
                        }
                    }
                    (_, None) => Response::error(format!("unknown key: {key}")),
                    (None, _) => Response::error("no focused pane"),
                }
            }
            Request::Focus { target } => {
                let ok = match target {
                    Target::Workspace(w) => self.state.focus_workspace(w),
                    Target::Tab(t) => match self.state.locate_tab_workspace(t) {
                        Some(w) => self.state.focus_tab(w, t),
                        None => false,
                    },
                    Target::Pane(p) => self.state.focus_pane(p),
                };
                if ok {
                    Response::Ok
                } else {
                    Response::error("focus target not found")
                }
            }
            Request::FocusDir { dir } => {
                let d = match dir {
                    Dir::Left => FocusDir::Left,
                    Dir::Right => FocusDir::Right,
                    Dir::Up => FocusDir::Up,
                    Dir::Down => FocusDir::Down,
                };
                if self.state.focus_dir(d) {
                    Response::Ok
                } else {
                    Response::error("no pane in that direction")
                }
            }
            Request::NewTab { workspace } => {
                let ws = workspace.or_else(|| self.state.active_workspace.map(|w| w));
                match ws.and_then(|w| {
                    let t = self.state.add_tab(w);
                    self.ensure_runtimes();
                    t
                }) {
                    Some(t) => Response::Created { id: t.raw() },
                    None => Response::error("no workspace"),
                }
            }
            Request::NewWorkspace { title } => {
                let ws = self.new_workspace(title.as_deref().unwrap_or("workspace"));
                Response::Created { id: ws.raw() }
            }
            Request::Split { pane, orientation } => {
                let target = pane.or_else(|| self.state.focused_pane());
                match target.and_then(|p| {
                    let n = self.state.split_pane(p, orientation.into());
                    self.ensure_runtimes();
                    n
                }) {
                    Some(p) => Response::Created { id: p.raw() },
                    None => Response::error("could not split"),
                }
            }
            Request::ClosePane { pane } => {
                if self.close_pane(pane) {
                    Response::Ok
                } else {
                    Response::error("no such pane")
                }
            }
            Request::Notify { pane, title, body } => {
                match self.state.notify(pane, title, body, Self::now_ms()) {
                    Some(_) => Response::Ok,
                    None => Response::error("no such pane"),
                }
            }
            Request::Snapshot { pane } => match self.terminal(pane) {
                Some(t) => Response::Snapshot {
                    text: t.render_to_string(),
                },
                None => Response::error("no such pane"),
            },
            Request::GetConfig { path } => match path {
                Some(p) => match self.config.get_path(&p) {
                    Ok(v) => Response::ConfigValue { value: v },
                    Err(e) => Response::error(e.to_string()),
                },
                None => Response::ConfigValue {
                    value: serde_json::to_value(&self.config).unwrap_or_default(),
                },
            },
            Request::SetConfig { path, value } => match self.config.set_path(&path, &value) {
                Ok(()) => Response::Ok,
                Err(e) => Response::error(e.to_string()),
            },
            Request::ListNotifications => Response::Notifications {
                notifications: self.notifications().to_vec(),
            },
            Request::MarkAllRead => {
                self.mark_all_read();
                Response::Ok
            }
        }
    }

    /// Layout rectangles (unit square) for the active tab's panes — what the UI
    /// positions terminal views with.
    pub fn active_layout(&self) -> Vec<(PaneId, Rect)> {
        self.state
            .active_workspace()
            .and_then(|w| w.active_tab())
            .map(|t| t.tree.layout(Rect::new(0.0, 0.0, 1.0, 1.0)))
            .unwrap_or_default()
    }

    pub fn pane_ring(&self, pane: PaneId) -> RingState {
        self.state.pane(pane).map(|p| p.ring).unwrap_or_default()
    }

    pub fn notifications(&self) -> &[cmux_core::Notification] {
        self.state.notifications.entries()
    }

    pub fn mark_all_read(&mut self) -> usize {
        self.state.notifications.mark_all_read()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmux_ipc::protocol::SplitDir;

    fn engine() -> Engine {
        Engine::new(Config::default())
    }

    #[test]
    fn new_engine_has_focused_pane_with_runtime() {
        let mut e = engine();
        let focused = e.state.focused_pane().unwrap();
        // A shell was spawned; output should arrive within a moment.
        std::thread::sleep(std::time::Duration::from_millis(150));
        e.pump();
        assert!(e.terminal(focused).is_some());
    }

    #[test]
    fn split_via_request_creates_pane_and_runtime() {
        let mut e = engine();
        let resp = e.handle_request(Request::Split {
            pane: None,
            orientation: SplitDir::Horizontal,
        });
        let id = match resp {
            Response::Created { id } => id,
            other => panic!("expected Created, got {other:?}"),
        };
        assert!(e.terminal(PaneId(id)).is_some());
        assert_eq!(e.active_layout().len(), 2);
    }

    #[test]
    fn snapshot_request_returns_text() {
        let mut e = engine();
        let focused = e.state.focused_pane().unwrap();
        e.write_pane(focused, b"echo cmuxhello\n");
        std::thread::sleep(std::time::Duration::from_millis(300));
        e.pump();
        let resp = e.handle_request(Request::Snapshot { pane: focused });
        match resp {
            Response::Snapshot { text } => assert!(text.contains("cmuxhello")),
            other => panic!("expected Snapshot, got {other:?}"),
        }
    }

    #[test]
    fn config_get_set_via_request() {
        let mut e = engine();
        assert!(matches!(
            e.handle_request(Request::SetConfig {
                path: "appearance.theme".into(),
                value: "dark".into()
            }),
            Response::Ok
        ));
        match e.handle_request(Request::GetConfig {
            path: Some("appearance.theme".into()),
        }) {
            Response::ConfigValue { value } => assert_eq!(value, serde_json::json!("dark")),
            other => panic!("expected ConfigValue, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_split_and_close_actions() {
        let mut e = engine();
        assert_eq!(e.active_layout().len(), 1);
        assert!(e.dispatch_action("splitVertical"));
        assert_eq!(e.active_layout().len(), 2);
        assert!(e.dispatch_action("closePane"));
        assert_eq!(e.active_layout().len(), 1);
    }

    #[test]
    fn dispatch_tab_actions_and_reopen() {
        let mut e = engine();
        let ws = e.state.active_workspace.unwrap();
        assert!(e.dispatch_action("newTab"));
        assert_eq!(e.state.workspace(ws).unwrap().tabs.len(), 2);
        assert!(e.dispatch_action("closeTab"));
        assert_eq!(e.state.workspace(ws).unwrap().tabs.len(), 1);
        assert!(e.dispatch_action("reopenClosedTab"));
        assert_eq!(e.state.workspace(ws).unwrap().tabs.len(), 2);
    }

    #[test]
    fn dispatch_focus_directions() {
        let mut e = engine();
        let left = e.state.focused_pane().unwrap();
        let right = e.split_focused(Orientation::Horizontal).unwrap();
        assert_eq!(e.state.focused_pane(), Some(right));
        assert!(e.dispatch_action("focusLeft"));
        assert_eq!(e.state.focused_pane(), Some(left));
    }

    #[test]
    fn dispatch_unknown_action_is_noop() {
        let mut e = engine();
        assert!(!e.dispatch_action("commandPalette"));
        assert!(!e.dispatch_action("totallyMadeUp"));
    }

    #[test]
    fn resize_pane_succeeds_for_live_pane() {
        let mut e = engine();
        let p = e.state.focused_pane().unwrap();
        assert!(e.resize_pane(p, 40, 120));
        assert_eq!(e.terminal(p).unwrap().rows(), 40);
        assert_eq!(e.terminal(p).unwrap().cols(), 120);
    }

    #[test]
    fn jump_to_latest_unread_focuses_pane() {
        let mut e = engine();
        let bg = e.state.active_workspace().unwrap().tabs[0].focused.unwrap();
        e.new_tab(); // move focus away so bg is unfocused
        e.state.notify(bg, "Claude", "ping", 1);
        assert!(e.dispatch_action("jumpToLatestNotification"));
        assert_eq!(e.state.focused_pane(), Some(bg));
    }

    #[test]
    fn close_pane_request_removes_runtime() {
        let mut e = engine();
        let new = match e.handle_request(Request::Split {
            pane: None,
            orientation: SplitDir::Vertical,
        }) {
            Response::Created { id } => PaneId(id),
            _ => unreachable!(),
        };
        assert!(matches!(
            e.handle_request(Request::ClosePane { pane: new }),
            Response::Ok
        ));
        assert!(e.terminal(new).is_none());
    }
}
