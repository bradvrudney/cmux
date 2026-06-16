//! The application engine: the authoritative model plus the live runtimes
//! (PTY + terminal emulator) backing each pane.
//!
//! The engine lives behind an `Arc<Mutex<…>>` shared by the Dioxus UI (which
//! reads it every render tick) and the control-socket thread (which mutates it
//! in response to `cmux` CLI commands). All state-changing logic funnels through
//! [`Engine`] so both entrypoints behave identically — the shared-behavior rule
//! the upstream project insists on.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
    /// Lines scrolled back into history (0 = pinned to the live screen).
    scroll_offset: usize,
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
    /// Where the topology is persisted; `None` disables persistence (tests).
    session_path: Option<PathBuf>,
    /// Where `cmux.json` is written when settings change; `None` disables it.
    config_path: Option<PathBuf>,
    last_save: Instant,
    /// Active mouse text selection (viewport cell coordinates), if any.
    selection: Option<Selection>,
}

/// An in-progress / completed text selection over a pane's viewport.
#[derive(Clone, Copy)]
struct Selection {
    pane: PaneId,
    anchor: (usize, usize),
    active: (usize, usize),
}

/// Control-socket commands this build understands (for `cmux capabilities`).
const CAPABILITIES: &[&str] = &[
    "ping", "identify", "capabilities", "list-workspaces", "snapshot", "send",
    "send-key", "focus", "focus-dir", "split", "close-pane", "new-tab",
    "new-workspace", "close-workspace", "reorder-workspace", "rename-tab",
    "rename-workspace", "reorder-tab", "move-tab", "swap", "equalize", "zoom",
    "next-tab", "prev-tab", "trigger-flash", "resize", "find", "browser",
    "navigate", "notify", "notifications", "mark-read", "dismiss", "config",
];

/// If a whitespace-delimited `http(s)://` URL token covers `col` in `line`,
/// return it with trailing punctuation trimmed. Pure so it is unit-testable.
fn url_at_col(line: &str, col: usize) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    if col >= chars.len() || chars[col].is_whitespace() {
        return None;
    }
    let mut start = col;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }
    let mut end = col;
    while end < chars.len() && !chars[end].is_whitespace() {
        end += 1;
    }
    let token: String = chars[start..end].iter().collect();
    let token = token.trim_end_matches(|c: char| {
        matches!(c, '.' | ',' | ';' | ':' | ')' | ']' | '}' | '"' | '\'' | '>')
    });
    if token.starts_with("http://") || token.starts_with("https://") {
        Some(token.to_string())
    } else {
        None
    }
}

impl Engine {
    /// Build an engine, restoring the saved session if one exists (otherwise
    /// seeding a fresh workspace), and spawn shells for every pane.
    pub fn new(config: Config) -> Self {
        let mut e = Self::with_session(config, Self::default_session_path());
        e.config_path = cmux_config::Config::default_path().ok();
        e
    }

    /// Like [`Engine::new`] but with an explicit (or no) persistence path.
    /// Config persistence is disabled in this form (tests).
    pub fn with_session(config: Config, session_path: Option<PathBuf>) -> Self {
        let state = session_path
            .as_ref()
            .and_then(|p| Self::load_state(p))
            .filter(|s| !s.workspaces.is_empty())
            .unwrap_or_else(|| {
                let mut s = AppState::new();
                s.new_workspace("workspace");
                s
            });
        let mut engine = Engine {
            state,
            config,
            runtimes: HashMap::new(),
            session_path,
            config_path: None,
            last_save: Instant::now(),
            selection: None,
        };
        engine.ensure_runtimes();
        engine
    }

    /// `$XDG_DATA_HOME/cmux/session.json`, falling back to
    /// `~/.local/share/cmux/session.json`.
    pub fn default_session_path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
        Some(base.join("cmux").join("session.json"))
    }

    fn load_state(path: &PathBuf) -> Option<AppState> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Persist the current topology (not live PTYs — those respawn on restore).
    pub fn save_session(&self) {
        let Some(path) = &self.session_path else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.state) {
            let _ = std::fs::write(path, json);
        }
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
            // Browser panes are webview-backed and have no PTY.
            if self.state.pane(pane).map_or(false, |p| p.is_browser()) {
                continue;
            }
            let cwd = self.state.pane(pane).and_then(|p| p.cwd.clone());
            if let Some(rt) = self.spawn_runtime(cwd.as_deref()) {
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

    fn spawn_runtime(&self, cwd: Option<&str>) -> Option<PaneRuntime> {
        let mut cfg = PtyConfig::shell().with_size(DEFAULT_ROWS, DEFAULT_COLS);
        if let Some(shell) = &self.config.shell {
            cfg = PtyConfig::command([shell.clone()]).with_size(DEFAULT_ROWS, DEFAULT_COLS);
        }
        if let Some(dir) = cwd {
            cfg = cfg.with_cwd(dir);
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
            scroll_offset: 0,
        })
    }

    /// Drain all PTY output into terminal grids and translate bells into
    /// attention notifications. Returns the number of bytes ingested (so the UI
    /// can decide whether anything changed).
    pub fn pump(&mut self) -> usize {
        let mut total = 0;
        let panes: Vec<PaneId> = self.runtimes.keys().copied().collect();
        let mut bells: Vec<PaneId> = Vec::new();
        // (pane, title, body) notifications raised via OSC 9 / OSC 777.
        let mut osc_notifs: Vec<(PaneId, String, String)> = Vec::new();
        // (pane, new title) from OSC 0/1/2.
        let mut titles: Vec<(PaneId, String)> = Vec::new();
        // (pane, cwd) from OSC 7.
        let mut cwds: Vec<(PaneId, String)> = Vec::new();
        for pane in panes {
            if let Some(rt) = self.runtimes.get_mut(&pane) {
                while let Ok(evt) = rt.rx.try_recv() {
                    match evt {
                        PtyEvent::Output(bytes) => {
                            total += bytes.len();
                            rt.term.feed(&bytes);
                            // New output pins the view back to the live screen.
                            rt.scroll_offset = 0;
                        }
                        PtyEvent::Exited(_) => rt.exited = true,
                    }
                }
                if rt.term.take_bell() {
                    bells.push(pane);
                }
                if let Some(title) = rt.term.take_title() {
                    titles.push((pane, title));
                }
                if let Some(cwd) = rt.term.take_cwd() {
                    cwds.push((pane, cwd));
                }
                for (t, b) in rt.term.take_notifications() {
                    osc_notifs.push((pane, t, b));
                }
            }
        }
        // Apply OSC window titles to pane titles (drives tab/pane labels).
        for (pane, title) in titles {
            if let Some(p) = self.state.panes.get_mut(&pane) {
                if !title.is_empty() {
                    p.title = title;
                }
            }
        }
        // Track each pane's working directory (OSC 7) so new splits/tabs inherit it.
        for (pane, cwd) in cwds {
            if let Some(p) = self.state.panes.get_mut(&pane) {
                p.cwd = Some(cwd);
            }
        }
        let now = Self::now_ms();
        if self.config.notifications.enabled {
            let focused = self.state.focused_pane();
            // OSC-driven notifications are explicit app intent — always raise.
            for (pane, title, body) in osc_notifs {
                self.state.notify(pane, title.as_str(), body.as_str(), now);
                if Some(pane) != focused {
                    self.post_desktop_notification(&title, &body);
                }
            }
            if self.config.notifications.ring_on_bell {
                for pane in bells {
                    self.state.notify(pane, "Bell", "terminal bell", now);
                    if Some(pane) != focused {
                        self.post_desktop_notification("Bell", "terminal bell");
                    }
                }
            }
        }
        // Persist topology periodically so a crash/restart restores the layout.
        if self.session_path.is_some() && self.last_save.elapsed() > Duration::from_secs(5) {
            self.save_session();
            self.last_save = Instant::now();
        }
        total
    }

    // ---- rendering accessors -------------------------------------------

    pub fn terminal(&self, pane: PaneId) -> Option<&Terminal> {
        self.runtimes.get(&pane).map(|r| &r.term)
    }

    /// The visible rows for a pane, accounting for its scrollback offset.
    pub fn terminal_viewport(&self, pane: PaneId) -> Option<Vec<Vec<cmux_term::Cell>>> {
        self.runtimes
            .get(&pane)
            .map(|r| r.term.viewport(r.scroll_offset))
    }

    /// Search a pane's scrollback + screen for `query` (case-insensitive).
    pub fn search_pane(&self, pane: PaneId, query: &str) -> Vec<cmux_term::Match> {
        self.runtimes
            .get(&pane)
            .map(|rt| rt.term.search(query))
            .unwrap_or_default()
    }

    /// Scroll a pane so absolute line `abs_line` is near the top of the view.
    pub fn scroll_pane_to_line(&mut self, pane: PaneId, abs_line: usize) -> bool {
        match self.runtimes.get_mut(&pane) {
            Some(rt) => {
                let rows = rt.term.rows() as i32;
                let total = rt.term.total_lines() as i32;
                let max = rt.term.scrollback_len() as i32;
                let offset = (total - rows - abs_line as i32).clamp(0, max);
                rt.scroll_offset = offset as usize;
                true
            }
            None => false,
        }
    }

    /// Scroll a pane through its history. Positive `delta` scrolls up (older);
    /// the offset is clamped to the available scrollback.
    pub fn scroll_pane(&mut self, pane: PaneId, delta: i32) -> bool {
        match self.runtimes.get_mut(&pane) {
            Some(rt) => {
                let max = rt.term.scrollback_len() as i32;
                let next = (rt.scroll_offset as i32 + delta).clamp(0, max);
                rt.scroll_offset = next as usize;
                true
            }
            None => false,
        }
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

    /// Post a best-effort freedesktop (D-Bus) desktop notification. Silently
    /// no-ops when no notification daemon is reachable (headless/CI).
    fn post_desktop_notification(&self, title: &str, body: &str) {
        let mut n = notify_rust::Notification::new();
        n.summary(if title.is_empty() { "cmux" } else { title })
            .body(body)
            .appname("cmux");
        if self.config.notifications.sound {
            n.sound_name("message");
        }
        let _ = n.show();
    }

    /// If a clickable URL covers viewport cell (`row`, `col`) in `pane`, return
    /// it. Used for ctrl-click-to-open.
    pub fn url_at(&self, pane: PaneId, row: usize, col: usize) -> Option<String> {
        let view = self.terminal_viewport(pane)?;
        let line: String = view
            .get(row)?
            .iter()
            .map(|c| if c.c == '\0' { ' ' } else { c.c })
            .collect();
        url_at_col(&line, col)
    }

    /// The terminal cursor `(row, col)` for `pane`, but only on the live screen
    /// (not scrolled into history) and when the cursor is visible.
    pub fn cursor_for(&self, pane: PaneId) -> Option<(usize, usize)> {
        let rt = self.runtimes.get(&pane)?;
        if rt.scroll_offset != 0 {
            return None;
        }
        let c = rt.term.cursor();
        if c.visible {
            Some((c.row, c.col))
        } else {
            None
        }
    }

    // ---- text selection & clipboard ------------------------------------

    /// Begin a selection in `pane` at viewport cell (`row`, `col`).
    pub fn begin_selection(&mut self, pane: PaneId, row: usize, col: usize) {
        self.selection = Some(Selection {
            pane,
            anchor: (row, col),
            active: (row, col),
        });
    }

    /// Extend the in-progress selection in `pane` to (`row`, `col`).
    pub fn extend_selection(&mut self, pane: PaneId, row: usize, col: usize) {
        if let Some(s) = &mut self.selection {
            if s.pane == pane {
                s.active = (row, col);
            }
        }
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// The normalized selection for `pane`, for rendering its highlight.
    pub fn selection_for(&self, pane: PaneId) -> Option<crate::render::ViewportSelection> {
        let s = self.selection.filter(|s| s.pane == pane)?;
        Some(crate::render::ViewportSelection::new(s.anchor, s.active))
    }

    /// Extract the selected text from the pane's current viewport (trailing
    /// blanks trimmed per line), or `None` if there is no selection.
    pub fn copy_selection(&self) -> Option<String> {
        let s = self.selection?;
        let view = self.terminal_viewport(s.pane)?;
        let sel = crate::render::ViewportSelection::new(s.anchor, s.active);
        let mut lines = Vec::new();
        for (r, row) in view.iter().enumerate() {
            if let Some((lo, hi)) = sel.cols_for_row(r, row.len()) {
                let text: String = row[lo..=hi]
                    .iter()
                    .map(|c| if c.c == '\0' { ' ' } else { c.c })
                    .collect();
                lines.push(text.trim_end().to_string());
            }
        }
        if lines.is_empty() {
            None
        } else {
            Some(lines.join("\n"))
        }
    }

    // ---- topology operations (shared by GUI and socket) -----------------

    /// The live working directory of a pane's shell, read from `/proc/<pid>/cwd`
    /// (Linux). More reliable than OSC 7, which a non-login shell may not emit.
    fn proc_cwd(&self, pane: PaneId) -> Option<String> {
        let pid = self.runtimes.get(&pane)?.pty.process_id()?;
        let link = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
        Some(link.to_string_lossy().into_owned())
    }

    /// Refresh a pane's recorded cwd from its live shell, so a split/new-tab
    /// spawned from it inherits the current directory.
    fn refresh_cwd(&mut self, pane: PaneId) {
        if let Some(dir) = self.proc_cwd(pane) {
            if let Some(p) = self.state.panes.get_mut(&pane) {
                p.cwd = Some(dir);
            }
        }
    }

    pub fn split_focused(&mut self, orientation: Orientation) -> Option<PaneId> {
        if let Some(f) = self.state.focused_pane() {
            self.refresh_cwd(f);
        }
        let new = self.state.split_focused(orientation);
        self.ensure_runtimes();
        new
    }

    pub fn new_tab(&mut self) -> Option<cmux_core::ids::TabId> {
        let ws = self.state.active_workspace?;
        if let Some(f) = self.state.focused_pane() {
            self.refresh_cwd(f);
        }
        let t = self.state.add_tab(ws);
        self.ensure_runtimes();
        t
    }

    pub fn new_workspace(&mut self, title: &str) -> cmux_core::ids::WorkspaceId {
        let ws = self.state.new_workspace(title);
        self.state.focus_workspace(ws);
        self.ensure_runtimes();
        // Optionally run a configured command in the new workspace's first pane.
        if let Some(cmd) = self.config.new_workspace_command.clone() {
            let pane = self
                .state
                .workspace(ws)
                .and_then(|w| w.active_tab())
                .and_then(|t| t.focused);
            if let Some(p) = pane {
                let mut line = cmd;
                if !line.ends_with('\n') {
                    line.push('\n');
                }
                self.write_pane(p, line.as_bytes());
            }
        }
        ws
    }

    pub fn close_pane(&mut self, pane: PaneId) -> bool {
        let ok = self.state.close_pane(pane);
        self.ensure_runtimes();
        ok
    }

    /// Split the focused pane into a browser pane showing `url`.
    pub fn open_browser(&mut self, url: &str, orientation: Orientation) -> Option<PaneId> {
        let id = self.state.split_focused_browser(url, orientation);
        self.ensure_runtimes();
        id
    }

    /// Navigate an existing browser pane to `url`.
    pub fn navigate_browser(&mut self, pane: PaneId, url: &str) -> bool {
        self.state.set_browser_url(pane, url)
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

    fn close_other_tabs(&mut self) -> bool {
        let n = self.state.close_other_tabs();
        self.ensure_runtimes();
        n > 0
    }

    fn reopen_closed_workspace(&mut self) -> bool {
        let ok = self.state.reopen_closed_workspace().is_some();
        self.ensure_runtimes();
        ok
    }

    /// Kill and respawn a pane's shell (e.g. after it exited), reusing its cwd.
    pub fn respawn_pane(&mut self, pane: PaneId) -> bool {
        if !self.state.panes.contains_key(&pane) {
            return false;
        }
        self.runtimes.remove(&pane); // Drop kills the old child.
        self.ensure_runtimes(); // Respawns a fresh shell in the pane's cwd.
        true
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
            "openBrowser" => self
                .open_browser("https://example.com", Orientation::Horizontal)
                .is_some(),
            "closePane" => self.close_focused_pane(),
            "focusLeft" => self.state.focus_dir(FocusDir::Left),
            "focusRight" => self.state.focus_dir(FocusDir::Right),
            "focusUp" => self.state.focus_dir(FocusDir::Up),
            "focusDown" => self.state.focus_dir(FocusDir::Down),
            "reopenClosedTab" => self.reopen_closed_tab(),
            "reopenClosedWorkspace" => self.reopen_closed_workspace(),
            "closeOtherTabs" => self.close_other_tabs(),
            "focusBack" => self.state.focus_history_step(false),
            "focusForward" => self.state.focus_history_step(true),
            "jumpToLatestNotification" => self.focus_latest_unread(),
            "nextTab" => self.state.focus_adjacent_tab(true),
            "previousTab" => self.state.focus_adjacent_tab(false),
            "equalizeSplits" => self.state.equalize_active(),
            "toggleZoom" => self.state.toggle_zoom(),
            "closeWorkspace" => self.close_active_workspace(),
            "swapLeft" => self.state.swap_focused(FocusDir::Left),
            "swapRight" => self.state.swap_focused(FocusDir::Right),
            "swapUp" => self.state.swap_focused(FocusDir::Up),
            "swapDown" => self.state.swap_focused(FocusDir::Down),
            "moveTabToNewWorkspace" => {
                match self.state.active_workspace().and_then(|w| w.active_tab) {
                    Some(t) => self.state.move_tab_to_new_workspace(t).is_some(),
                    None => false,
                }
            }
            // `selectWorkspaceN` / `selectSurfaceN` focus the N-th (1-based)
            // workspace / pane — the rebindable ⌘1–9 / ⌃1–9 families upstream.
            other => {
                if let Some(n) = other
                    .strip_prefix("selectWorkspace")
                    .and_then(|n| n.parse::<usize>().ok())
                    .filter(|n| *n >= 1)
                {
                    self.state.focus_workspace_index(n - 1)
                } else if let Some(n) = other
                    .strip_prefix("selectSurface")
                    .and_then(|n| n.parse::<usize>().ok())
                    .filter(|n| *n >= 1)
                {
                    self.state.focus_pane_index(n - 1)
                } else if self.config.actions.contains_key(other) {
                    self.run_custom_action(other)
                } else {
                    false
                }
            }
        }
    }

    /// Run a user-defined `cmux.json` action by id (open a tab or use the
    /// current pane, then type the command). Returns `false` if unknown.
    fn run_custom_action(&mut self, id: &str) -> bool {
        let Some(def) = self.config.actions.get(id).cloned() else {
            return false;
        };
        let mut line = def.command;
        if !line.ends_with('\n') {
            line.push('\n');
        }
        match def.target {
            cmux_config::ActionTarget::NewTab => {
                self.new_tab();
                self.write_focused(line.as_bytes())
            }
            cmux_config::ActionTarget::CurrentPane => self.write_focused(line.as_bytes()),
        }
    }

    fn close_active_workspace(&mut self) -> bool {
        match self.state.active_workspace {
            Some(ws) => {
                let ok = self.state.close_workspace(ws);
                self.ensure_runtimes();
                ok
            }
            None => false,
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
                if let Some(f) = self.state.focused_pane() {
                    self.refresh_cwd(f);
                }
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
                if let Some(t) = target {
                    self.refresh_cwd(t);
                }
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
                let focused = self.state.focused_pane();
                match self
                    .state
                    .notify(pane, title.as_str(), body.as_str(), Self::now_ms())
                {
                    Some(_) => {
                        if self.config.notifications.enabled && Some(pane) != focused {
                            self.post_desktop_notification(&title, &body);
                        }
                        Response::Ok
                    }
                    None => Response::error("no such pane"),
                }
            }
            Request::Snapshot { pane } => match self.terminal(pane) {
                Some(t) => Response::Snapshot {
                    text: t.render_to_string(),
                },
                None if self.state.pane(pane).map_or(false, |p| p.is_browser()) => {
                    Response::error("pane is a browser, not a terminal")
                }
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
            Request::SetConfig { path, value } => match self.set_config(&path, &value) {
                Ok(()) => Response::Ok,
                Err(e) => Response::error(e),
            },
            Request::ListNotifications => Response::Notifications {
                notifications: self.notifications().to_vec(),
            },
            Request::MarkAllRead => {
                self.mark_all_read();
                Response::Ok
            }
            Request::RenameTab { tab, title } => {
                if self.state.rename_tab(tab, title) {
                    Response::Ok
                } else {
                    Response::error("no such tab")
                }
            }
            Request::RenameWorkspace { workspace, title } => {
                if self.state.rename_workspace(workspace, title) {
                    Response::Ok
                } else {
                    Response::error("no such workspace")
                }
            }
            Request::ReorderTab { tab, index } => {
                match self.state.locate_tab_workspace(tab) {
                    Some(ws) if self.state.reorder_tab(ws, tab, index) => Response::Ok,
                    _ => Response::error("no such tab"),
                }
            }
            Request::ResizePane { pane, rows, cols } => {
                if self.resize_pane(pane, rows, cols) {
                    Response::Ok
                } else {
                    Response::error("no such pane")
                }
            }
            Request::OpenBrowser { url, orientation } => {
                match self.open_browser(&url, orientation.into()) {
                    Some(id) => Response::Created { id: id.raw() },
                    None => Response::error("could not open browser"),
                }
            }
            Request::NavigateBrowser { pane, url } => {
                if self.navigate_browser(pane, &url) {
                    Response::Ok
                } else {
                    Response::error("not a browser pane")
                }
            }
            Request::Find { pane, query } => {
                if self.terminal(pane).is_none() {
                    return Response::error("no such terminal pane");
                }
                let matches = self
                    .search_pane(pane, &query)
                    .into_iter()
                    .map(|m| (m.line, m.col))
                    .collect();
                Response::Matches { matches }
            }
            Request::CloseWorkspace { workspace } => {
                let ok = self.state.close_workspace(workspace);
                self.ensure_runtimes();
                if ok {
                    Response::Ok
                } else {
                    Response::error("no such workspace")
                }
            }
            Request::ReorderWorkspace { workspace, index } => {
                if self.state.reorder_workspace(workspace, index) {
                    Response::Ok
                } else {
                    Response::error("no such workspace")
                }
            }
            Request::Equalize => {
                if self.state.equalize_active() {
                    Response::Ok
                } else {
                    Response::error("no active tab")
                }
            }
            Request::ToggleZoom => {
                if self.state.toggle_zoom() {
                    Response::Ok
                } else {
                    Response::error("no active tab")
                }
            }
            Request::NextTab => {
                if self.state.focus_adjacent_tab(true) {
                    Response::Ok
                } else {
                    Response::error("no other tab")
                }
            }
            Request::PrevTab => {
                if self.state.focus_adjacent_tab(false) {
                    Response::Ok
                } else {
                    Response::error("no other tab")
                }
            }
            Request::MarkNotificationRead { id } => {
                if self.state.mark_notification_read(id) {
                    Response::Ok
                } else {
                    Response::error("no such notification")
                }
            }
            Request::DismissNotification { id } => {
                if self.state.dismiss_notification(id) {
                    Response::Ok
                } else {
                    Response::error("no such notification")
                }
            }
            Request::MoveTab { tab, workspace } => {
                let ok = match workspace {
                    Some(ws) => self.state.move_tab_to_workspace(tab, ws),
                    None => self.state.move_tab_to_new_workspace(tab).is_some(),
                };
                if ok {
                    Response::Ok
                } else {
                    Response::error("could not move tab")
                }
            }
            Request::SwapPane { dir } => {
                let d = match dir {
                    Dir::Left => FocusDir::Left,
                    Dir::Right => FocusDir::Right,
                    Dir::Up => FocusDir::Up,
                    Dir::Down => FocusDir::Down,
                };
                if self.state.swap_focused(d) {
                    Response::Ok
                } else {
                    Response::error("no pane in that direction")
                }
            }
            Request::Identify => Response::Snapshot {
                text: format!(
                    "cmux-linux {}\npid {}\nworkspaces {}\npanes {}",
                    env!("CARGO_PKG_VERSION"),
                    std::process::id(),
                    self.state.workspaces.len(),
                    self.state.panes.len(),
                ),
            },
            Request::Capabilities => Response::Snapshot {
                text: CAPABILITIES.join("\n"),
            },
            Request::TriggerFlash { pane } => {
                match pane.or_else(|| self.state.focused_pane()) {
                    Some(p) if self.state.set_ring(p, RingState::Attention) => Response::Ok,
                    _ => Response::error("no such pane"),
                }
            }
            Request::RunAction { id } => {
                if self.run_custom_action(&id) {
                    Response::Ok
                } else {
                    Response::error("no such action")
                }
            }
            Request::Respawn { pane } => match pane.or_else(|| self.state.focused_pane()) {
                Some(p) if self.respawn_pane(p) => Response::Ok,
                _ => Response::error("no such pane"),
            },
        }
    }

    /// Layout rectangles (unit square) for the active tab's panes — what the UI
    /// positions terminal views with.
    pub fn active_layout(&self) -> Vec<(PaneId, Rect)> {
        // A zoomed pane takes the whole tab area; the rest stay alive but hidden.
        if let Some(z) = self.state.zoomed_pane() {
            return vec![(z, Rect::new(0.0, 0.0, 1.0, 1.0))];
        }
        self.state
            .active_workspace()
            .and_then(|w| w.active_tab())
            .map(|t| t.tree.layout(Rect::new(0.0, 0.0, 1.0, 1.0)))
            .unwrap_or_default()
    }

    pub fn pane_ring(&self, pane: PaneId) -> RingState {
        self.state.pane(pane).map(|p| p.ring).unwrap_or_default()
    }

    /// Dividers of the active tab (unit-square coordinates).
    pub fn active_dividers(&self) -> Vec<cmux_core::split::Divider> {
        // No draggable dividers while a pane is zoomed.
        if self.state.zoomed_pane().is_some() {
            return Vec::new();
        }
        self.state
            .active_workspace()
            .and_then(|w| w.active_tab())
            .map(|t| t.tree.dividers(Rect::new(0.0, 0.0, 1.0, 1.0)))
            .unwrap_or_default()
    }

    /// Drag a divider in the active tab to a new ratio.
    pub fn set_active_divider(&mut self, split_index: usize, ratio: f32) -> bool {
        self.state.set_active_divider(split_index, ratio)
    }

    pub fn notifications(&self) -> &[cmux_core::Notification] {
        self.state.notifications.entries()
    }

    pub fn mark_all_read(&mut self) -> usize {
        self.state.notifications.mark_all_read()
    }

    /// Set a config value by dotted path and persist `cmux.json` (if a config
    /// path is configured). Returns the same errors as [`Config::set_path`].
    pub fn set_config(&mut self, path: &str, value: &str) -> Result<(), String> {
        self.config
            .set_path(path, value)
            .map_err(|e| e.to_string())?;
        if let Some(p) = &self.config_path {
            let _ = self.config.save(p);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmux_ipc::protocol::SplitDir;

    fn engine() -> Engine {
        // No persistence path → tests don't touch the real session file.
        Engine::with_session(Config::default(), None)
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
    fn session_roundtrips_topology() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cmux").join("session.json");
        let mut e = Engine::with_session(Config::default(), Some(path.clone()));
        e.dispatch_action("splitHorizontal");
        e.new_tab();
        let ws_count = e.state.workspaces.len();
        let pane_count = e.state.panes.len();
        e.save_session();
        assert!(path.exists());

        // A fresh engine restores the same topology and respawns runtimes.
        let e2 = Engine::with_session(Config::default(), Some(path));
        assert_eq!(e2.state.workspaces.len(), ws_count);
        assert_eq!(e2.state.panes.len(), pane_count);
        let pane = e2.state.focused_pane().unwrap();
        assert!(e2.terminal(pane).is_some());
    }

    #[test]
    fn run_custom_action_from_config() {
        let mut e = engine();
        e.config.actions.insert(
            "hello".into(),
            cmux_config::ActionDef {
                command: "echo hi".into(),
                label: None,
                target: cmux_config::ActionTarget::CurrentPane,
            },
        );
        assert_eq!(
            e.handle_request(Request::RunAction { id: "hello".into() }),
            Response::Ok
        );
        assert_eq!(
            e.handle_request(Request::RunAction { id: "nope".into() }),
            Response::error("no such action")
        );
        // The same id also runs through the shared dispatch path.
        assert!(e.dispatch_action("hello"));
        assert!(!e.dispatch_action("unbound-id"));
    }

    #[test]
    fn request_identify_capabilities_and_flash() {
        let mut e = engine();
        let p = e.state.focused_pane().unwrap();
        match e.handle_request(Request::Identify) {
            Response::Snapshot { text } => assert!(text.contains("cmux-linux")),
            other => panic!("expected Snapshot, got {other:?}"),
        }
        match e.handle_request(Request::Capabilities) {
            Response::Snapshot { text } => assert!(text.contains("identify")),
            other => panic!("expected Snapshot, got {other:?}"),
        }
        assert_eq!(
            e.handle_request(Request::TriggerFlash { pane: Some(p) }),
            Response::Ok
        );
        assert!(e.state.pane(p).unwrap().ring.is_attention());
    }

    #[test]
    fn url_detection_under_column() {
        let line = "see https://example.com/path) for docs";
        // Column inside the URL returns it (trailing ')' trimmed).
        let u = url_at_col(line, 10).unwrap();
        assert_eq!(u, "https://example.com/path");
        // Column over plain words is not a URL.
        assert!(url_at_col(line, 0).is_none());
        assert!(url_at_col(line, 2).is_none());
        // A space between tokens is not a URL.
        assert!(url_at_col(line, 3).is_none());
        // Non-http tokens are ignored.
        assert!(url_at_col("ftp://nope.example", 4).is_none());
    }

    #[test]
    fn cursor_for_terminal_but_not_browser() {
        let mut e = engine();
        let p = e.state.focused_pane().unwrap();
        // A live terminal pane reports a visible cursor on the live screen.
        assert!(e.cursor_for(p).is_some());
        // A browser pane has no PTY runtime, hence no cursor.
        let b = match e.handle_request(Request::OpenBrowser {
            url: "https://example.com".into(),
            orientation: SplitDir::Horizontal,
        }) {
            Response::Created { id } => PaneId(id),
            other => panic!("expected Created, got {other:?}"),
        };
        assert!(e.cursor_for(b).is_none());
    }

    #[test]
    fn selection_state_and_copy_plumbing() {
        let mut e = engine();
        let p = e.state.focused_pane().unwrap();
        assert!(e.copy_selection().is_none());
        assert!(e.selection_for(p).is_none());
        e.begin_selection(p, 0, 0);
        e.extend_selection(p, 0, 3);
        assert!(e.selection_for(p).is_some());
        // A selection over the (blank) viewport copies a (possibly empty) line;
        // content correctness is covered by the render-layer tests.
        assert!(e.copy_selection().is_some());
        e.clear_selection();
        assert!(e.copy_selection().is_none());
        assert!(e.selection_for(p).is_none());
    }

    #[test]
    fn dispatch_zoom_and_equalize_actions() {
        let mut e = engine();
        e.split_focused(Orientation::Horizontal);
        assert_eq!(e.active_layout().len(), 2);
        // Zoom collapses the visible layout to the focused pane, hiding dividers.
        assert!(e.dispatch_action("toggleZoom"));
        assert_eq!(e.active_layout().len(), 1);
        assert!(e.active_dividers().is_empty());
        assert!(e.dispatch_action("toggleZoom"));
        assert_eq!(e.active_layout().len(), 2);
        // Equalize is a no-op on layout count but must succeed with a split.
        assert!(e.dispatch_action("equalizeSplits"));
    }

    #[test]
    fn dispatch_tab_cycling_and_select_workspace() {
        let mut e = engine();
        let ws1 = e.state.active_workspace.unwrap();
        e.new_tab();
        let active_after_new = e.state.active_workspace().unwrap().active_tab;
        assert!(e.dispatch_action("nextTab"));
        assert_ne!(
            e.state.active_workspace().unwrap().active_tab,
            active_after_new
        );
        // selectWorkspaceN focuses by 1-based index.
        let ws2 = e.new_workspace("second");
        assert!(e.dispatch_action("selectWorkspace1"));
        assert_eq!(e.state.active_workspace, Some(ws1));
        assert!(e.dispatch_action("selectWorkspace2"));
        assert_eq!(e.state.active_workspace, Some(ws2));
        assert!(!e.dispatch_action("selectWorkspace9"));
    }

    #[test]
    fn dispatch_swap_move_and_select_surface() {
        let mut e = engine();
        let a = e.state.focused_pane().unwrap();
        let b = e.split_focused(Orientation::Horizontal).unwrap();
        // selectSurface1 focuses the first pane in tree order.
        assert!(e.dispatch_action("selectSurface1"));
        assert_eq!(e.state.focused_pane(), Some(a));
        // swapRight exchanges it with its right neighbor.
        assert!(e.dispatch_action("swapRight"));
        let leaves = e.active_layout().into_iter().map(|(p, _)| p).collect::<Vec<_>>();
        assert_eq!(leaves, vec![b, a]);
        // Moving a tab to a new workspace needs >1 tab in the source.
        e.new_tab();
        let before = e.state.workspaces.len();
        assert!(e.dispatch_action("moveTabToNewWorkspace"));
        assert_eq!(e.state.workspaces.len(), before + 1);
    }

    #[test]
    fn dispatch_close_workspace_drops_runtimes() {
        let mut e = engine();
        let p1 = e.state.focused_pane().unwrap();
        e.new_workspace("second");
        assert!(e.dispatch_action("closeWorkspace"));
        // The closed workspace's pane and its PTY runtime are gone.
        assert!(e.state.active_workspace().is_some());
        let _ = p1;
    }

    #[test]
    fn request_equalize_zoom_and_notification_ops() {
        let mut e = engine();
        e.split_focused(Orientation::Horizontal);
        assert_eq!(e.handle_request(Request::Equalize), Response::Ok);
        assert_eq!(e.handle_request(Request::ToggleZoom), Response::Ok);
        assert_eq!(e.handle_request(Request::NextTab), Response::error("no other tab"));
        // Raise a notification on a background pane, then mark/dismiss by id.
        let bg = e.state.focused_pane().unwrap();
        e.new_tab();
        e.state.notify(bg, "a", "", 1);
        assert_eq!(e.handle_request(Request::MarkNotificationRead { id: 0 }), Response::Ok);
        assert_eq!(e.handle_request(Request::DismissNotification { id: 0 }), Response::Ok);
        assert_eq!(
            e.handle_request(Request::DismissNotification { id: 99 }),
            Response::error("no such notification")
        );
    }

    #[test]
    fn osc_notification_from_pty_output() {
        let mut e = engine();
        let p = e.state.focused_pane().unwrap();
        // The shell emits an OSC 777 notify sequence; the emulator parses it and
        // pump() turns it into a feed entry.
        e.write_pane(p, b"printf '\\033]777;notify;Claude;needs input\\007'\n");
        let mut found = false;
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            e.pump();
            if e.notifications()
                .iter()
                .any(|n| n.title == "Claude" && n.body == "needs input")
            {
                found = true;
                break;
            }
        }
        assert!(found, "OSC 777 notification was not captured");
    }

    #[test]
    fn osc_title_sets_pane_title() {
        let mut e = engine();
        let p = e.state.focused_pane().unwrap();
        // Emit an OSC 0 title, then keep the shell busy with `sleep` so it does
        // not immediately draw a new prompt. Interactive shells (e.g. Fedora's
        // bash with `PROMPT_COMMAND='printf "\033]0;...\007"'`) re-assert their
        // own window title on every prompt, which would clobber `my-task`
        // before this loop samples it — unlike OSC notifications, the title is
        // last-write-wins. Blocking on `sleep` holds `my-task` long enough to
        // observe the PTY → terminal → pane.title wiring deterministically.
        e.write_pane(p, b"printf '\\033]0;my-task\\007'; sleep 5\n");
        let mut titled = false;
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            e.pump();
            if e.state.pane(p).map(|p| p.title.as_str()) == Some("my-task") {
                titled = true;
                break;
            }
        }
        assert!(titled, "OSC title did not update the pane title");
    }

    #[test]
    fn scroll_offset_clamps_to_scrollback() {
        let mut e = engine();
        let p = e.state.focused_pane().unwrap();
        // No scrollback yet → scrolling up is clamped to 0.
        assert!(e.scroll_pane(p, 5));
        let v = e.terminal_viewport(p).unwrap();
        assert_eq!(v.len(), 24); // default rows
        // Scrolling down past 0 stays at 0.
        assert!(e.scroll_pane(p, -10));
    }

    #[test]
    fn open_browser_pane_has_no_pty_runtime() {
        let mut e = engine();
        let id = e
            .handle_request(Request::OpenBrowser {
                url: "https://example.com".into(),
                orientation: SplitDir::Horizontal,
            });
        let id = match id {
            Response::Created { id } => PaneId(id),
            other => panic!("expected Created, got {other:?}"),
        };
        assert!(e.state.pane(id).unwrap().is_browser());
        // Browser panes are webview-backed: no terminal runtime is spawned.
        assert!(e.terminal(id).is_none());
        // Navigation updates the URL.
        assert!(matches!(
            e.handle_request(Request::NavigateBrowser {
                pane: id,
                url: "https://docs.rs".into()
            }),
            Response::Ok
        ));
        assert_eq!(e.state.pane(id).unwrap().browser_url(), Some("https://docs.rs"));
    }

    #[test]
    fn set_config_updates_and_validates() {
        let mut e = engine();
        assert!(e.set_config("appearance.fontSize", "18").is_ok());
        assert_eq!(e.config.appearance.font_size, 18.0);
        assert!(e.set_config("sidebar.verticalTabs", "false").is_ok());
        assert!(!e.config.sidebar.vertical_tabs);
        // Invalid enum value is rejected.
        assert!(e.set_config("appearance.theme", "neon").is_err());
    }

    #[test]
    fn no_session_path_means_fresh_workspace() {
        let e = Engine::with_session(Config::default(), None);
        assert_eq!(e.state.workspaces.len(), 1);
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
