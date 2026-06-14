//! cmux-gui — the cmux-linux desktop application.
//!
//! Architecture: a single [`Engine`] (model + per-pane PTY/terminal runtimes)
//! lives behind an `Arc<Mutex<…>>`. A background thread serves the control
//! socket against it; the Dioxus UI reads it every animation tick and writes to
//! it from input handlers. Both the socket and the UI go through the same
//! `Engine` methods, so a `cmux split` from the CLI and a keyboard split behave
//! identically.

mod engine;
mod keys;
mod render;

use std::sync::{Arc, Mutex, OnceLock};

use dioxus::desktop::{Config as DesktopConfig, WindowBuilder};
use dioxus::prelude::*;

use cmux_config::{Config, SidebarPosition};
use cmux_core::ids::{PaneId, TabId, WorkspaceId};
use cmux_core::split::Orientation;
use cmux_core::RingState;
use engine::Engine;

type Shared = Arc<Mutex<Engine>>;

static ENGINE: OnceLock<Shared> = OnceLock::new();

fn engine() -> &'static Shared {
    ENGINE.get().expect("engine initialized in main")
}

fn main() {
    let config = Config::default_path()
        .ok()
        .and_then(|p| Config::load(&p).ok())
        .unwrap_or_default();

    let shared: Shared = Arc::new(Mutex::new(Engine::new(config)));
    let _ = ENGINE.set(shared.clone());

    spawn_control_socket(shared.clone());

    if std::env::args().any(|a| a == "--headless") {
        eprintln!("cmux-gui: headless mode — serving control socket only");
        // No UI tick to drain PTYs, so pump here.
        loop {
            shared.lock().unwrap().pump();
            std::thread::sleep(std::time::Duration::from_millis(33));
        }
    }

    let window = WindowBuilder::new().with_title("cmux");
    let cfg = DesktopConfig::new().with_window(window);
    LaunchBuilder::desktop().with_cfg(cfg).launch(App);
}

/// Bind the control socket and serve requests against the shared engine.
fn spawn_control_socket(shared: Shared) {
    std::thread::Builder::new()
        .name("cmux-control-socket".into())
        .spawn(move || {
            let path = match cmux_ipc::default_socket_path() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("cmux-gui: no control socket path: {e}");
                    return;
                }
            };
            let server = match cmux_ipc::Server::bind(&path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("cmux-gui: cannot bind control socket {}: {e}", path.display());
                    return;
                }
            };
            eprintln!("cmux-gui: control socket at {}", path.display());
            server.run(move |req| shared.lock().unwrap().handle_request(req));
        })
        .expect("spawn control socket thread");
}

// ---- view-model snapshots (owned, so the lock isn't held across rsx) -------

#[derive(Clone, PartialEq)]
struct PaneView {
    id: PaneId,
    rect: cmux_core::split::Rect,
    focused: bool,
    ring: RingState,
    alive: bool,
    rows: Vec<Vec<render::StyledRun>>,
}

#[derive(Clone, PartialEq)]
struct TabView {
    id: TabId,
    title: String,
    active: bool,
    attention: bool,
}

#[derive(Clone, PartialEq)]
struct WorkspaceView {
    id: WorkspaceId,
    title: String,
    active: bool,
}

#[derive(Clone, PartialEq, Default)]
struct Snapshot {
    workspaces: Vec<WorkspaceView>,
    tabs: Vec<TabView>,
    panes: Vec<PaneView>,
    unread: usize,
    sidebar_width: f32,
    sidebar_left: bool,
    font_size: f32,
}

fn snapshot() -> Snapshot {
    let e = engine().lock().unwrap();
    let active_ws = e.state.active_workspace;
    let workspaces = e
        .state
        .workspaces
        .iter()
        .map(|w| WorkspaceView {
            id: w.id,
            title: w.title.clone(),
            active: Some(w.id) == active_ws,
        })
        .collect();

    let tabs = e
        .state
        .active_workspace()
        .map(|w| {
            w.tabs
                .iter()
                .map(|t| TabView {
                    id: t.id,
                    title: t.title.clone(),
                    active: w.active_tab == Some(t.id),
                    attention: e.state.tab_has_attention(w.id, t.id),
                })
                .collect()
        })
        .unwrap_or_default();

    let focused = e.state.focused_pane();
    let panes = e
        .active_layout()
        .into_iter()
        .map(|(id, rect)| PaneView {
            id,
            rect,
            focused: Some(id) == focused,
            ring: e.pane_ring(id),
            alive: e.pane_alive(id),
            rows: e
                .terminal(id)
                .map(render::rows_to_runs)
                .unwrap_or_default(),
        })
        .collect();

    Snapshot {
        workspaces,
        tabs,
        panes,
        unread: e.state.notifications.unread_count(),
        sidebar_width: e.config.sidebar.width,
        sidebar_left: e.config.sidebar.position == SidebarPosition::Left,
        font_size: e.config.appearance.font_size,
    }
}

// ---- components ------------------------------------------------------------

#[component]
fn App() -> Element {
    let mut tick = use_signal(|| 0u64);

    // Drive PTY output ingestion + repaint at ~30fps.
    use_future(move || async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(33)).await;
            if let Some(e) = ENGINE.get() {
                if let Ok(mut g) = e.lock() {
                    g.pump();
                }
            }
            tick += 1;
        }
    });

    // Subscribe to the tick so the view re-renders.
    let _ = tick();
    let snap = snapshot();

    let sidebar = rsx! { Sidebar { snap: snap.clone(), tick } };
    let main = rsx! { PaneArea { snap: snap.clone(), tick } };

    rsx! {
        style { {BASE_CSS} }
        div {
            class: "app",
            tabindex: "0",
            autofocus: true,
            onkeydown: move |evt| {
                let Some(e) = ENGINE.get() else { return };
                let key = evt.key();
                let mods = evt.modifiers();
                // 1) A configured cmux shortcut takes precedence over typing.
                if let Some(chord) = keys::event_to_chord(&key, mods) {
                    let action = {
                        let g = e.lock().unwrap();
                        keys::resolve_action(&chord, &g.config.keyboard_shortcuts)
                            .map(|a| a.to_string())
                    };
                    if let Some(action) = action {
                        let mut g = e.lock().unwrap();
                        // Model actions go through the engine; UI-only actions
                        // (palette/settings/notifications) toggle local view state.
                        let handled = g.dispatch_action(&action) || matches!(
                            action.as_str(),
                            "commandPalette" | "openSettings" | "toggleNotifications"
                        );
                        drop(g);
                        if handled {
                            evt.prevent_default();
                            tick += 1;
                            return;
                        }
                    }
                }
                // 2) Otherwise, type into the focused pane's PTY.
                if let Some(bytes) = keys::key_event_to_bytes(&key, mods) {
                    if e.lock().unwrap().write_focused(&bytes) {
                        evt.prevent_default();
                        tick += 1;
                    }
                }
            },
            if snap.sidebar_left {
                {sidebar}
                {main}
            } else {
                {main}
                {sidebar}
            }
        }
    }
}

#[component]
fn Sidebar(snap: Snapshot, tick: Signal<u64>) -> Element {
    let width = snap.sidebar_width;
    rsx! {
        div {
            class: "sidebar",
            style: "width:{width}px;",
            div { class: "sidebar-header",
                span { class: "logo", "cmux" }
                if snap.unread > 0 {
                    span { class: "unread", "{snap.unread}" }
                }
            }
            div { class: "workspaces",
                for w in snap.workspaces.iter().cloned() {
                    button {
                        key: "{w.id}",
                        class: if w.active { "ws active" } else { "ws" },
                        onclick: move |_| {
                            engine().lock().unwrap().state.focus_workspace(w.id);
                            tick += 1;
                        },
                        "{w.title}"
                    }
                }
                button {
                    class: "ws add",
                    title: "New workspace",
                    onclick: move |_| {
                        engine().lock().unwrap().new_workspace("workspace");
                        tick += 1;
                    },
                    "+ ws"
                }
            }
            div { class: "tabs",
                for t in snap.tabs.iter().cloned() {
                    div {
                        key: "{t.id}",
                        class: if t.active { "tab active" } else { "tab" },
                        onclick: move |_| {
                            let mut g = engine().lock().unwrap();
                            if let Some(ws) = g.state.active_workspace {
                                g.state.focus_tab(ws, t.id);
                            }
                            tick += 1;
                        },
                        if t.attention {
                            span { class: "ring-dot", "●" }
                        }
                        span { class: "tab-title", "{t.title}" }
                    }
                }
                button {
                    class: "tab add",
                    onclick: move |_| {
                        engine().lock().unwrap().new_tab();
                        tick += 1;
                    },
                    "+ tab"
                }
            }
        }
    }
}

#[component]
fn PaneArea(snap: Snapshot, tick: Signal<u64>) -> Element {
    let font = snap.font_size;
    rsx! {
        div { class: "pane-area",
            for p in snap.panes.iter().cloned() {
                {
                    let left = p.rect.x * 100.0;
                    let top = p.rect.y * 100.0;
                    let w = p.rect.w * 100.0;
                    let h = p.rect.h * 100.0;
                    let cls = match (p.focused, p.ring) {
                        (_, RingState::Attention) => "pane attention",
                        (true, _) => "pane focused",
                        _ => "pane",
                    };
                    let cls = if p.alive { cls.to_string() } else { format!("{cls} exited") };
                    let pid = p.id;
                    rsx! {
                        div {
                            key: "{p.id}",
                            class: "{cls}",
                            style: "left:{left}%;top:{top}%;width:{w}%;height:{h}%;",
                            onmousedown: move |_| {
                                engine().lock().unwrap().state.focus_pane(pid);
                                tick += 1;
                            },
                            div {
                                class: "grid",
                                style: "font-size:{font}px;",
                                onmounted: move |evt| {
                                    // Size the PTY/grid to the rendered pane using
                                    // monospace cell metrics derived from the font.
                                    let char_w = (font as f64) * 0.6;
                                    let line_h = (font as f64) * 1.3;
                                    async move {
                                        if let Ok(rect) = evt.data().get_client_rect().await {
                                            let cols = (rect.width() / char_w).floor().max(1.0) as u16;
                                            let rows = (rect.height() / line_h).floor().max(1.0) as u16;
                                            if let Some(e) = ENGINE.get() {
                                                e.lock().unwrap().resize_pane(pid, rows, cols);
                                            }
                                        }
                                    }
                                },
                                for (ri, runs) in p.rows.iter().enumerate() {
                                    div { key: "{ri}", class: "row",
                                        for (ci, run) in runs.iter().enumerate() {
                                            span { key: "{ci}", style: "{run.style}", "{run.text}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// Keyboard shortcuts for split/close/new-tab handled here so the GUI matches the
// CLI; bound to a few defaults until config-driven binding lands (ROADMAP).
#[allow(dead_code)]
fn handle_shortcut(key: &str, tick: &mut Signal<u64>) -> bool {
    let mut g = engine().lock().unwrap();
    let handled = match key {
        "splitHorizontal" => g.split_focused(Orientation::Horizontal).is_some(),
        "splitVertical" => g.split_focused(Orientation::Vertical).is_some(),
        "newTab" => g.new_tab().is_some(),
        _ => false,
    };
    if handled {
        *tick += 1;
    }
    handled
}

const BASE_CSS: &str = r#"
* { box-sizing: border-box; }
html, body, #main, .app { height: 100%; margin: 0; }
.app {
    display: flex; flex-direction: row; height: 100vh; outline: none;
    font-family: -apple-system, system-ui, sans-serif;
    background: #181825; color: #cdd6f4; overflow: hidden;
}
.sidebar {
    display: flex; flex-direction: column; flex: 0 0 auto;
    background: #1e1e2e; border-right: 1px solid #313244; overflow-y: auto;
}
.sidebar-header {
    display: flex; align-items: center; justify-content: space-between;
    padding: 10px 12px; font-weight: 600; border-bottom: 1px solid #313244;
}
.logo { color: #4c71f2; }
.unread {
    background: #4c71f2; color: white; border-radius: 10px;
    padding: 1px 7px; font-size: 11px;
}
.workspaces { display: flex; flex-wrap: wrap; gap: 4px; padding: 8px; }
.ws {
    background: #313244; color: #cdd6f4; border: none; border-radius: 6px;
    padding: 4px 8px; font-size: 12px; cursor: pointer;
}
.ws.active { background: #4c71f2; color: white; }
.ws.add, .tab.add { background: transparent; border: 1px dashed #45475a; color: #6c7086; }
.tabs { display: flex; flex-direction: column; gap: 2px; padding: 8px; }
.tab {
    display: flex; align-items: center; gap: 6px; padding: 8px 10px;
    border-radius: 6px; cursor: pointer; font-size: 13px; color: #bac2de;
}
.tab:hover { background: #313244; }
.tab.active { background: #313244; color: #fff; box-shadow: inset 2px 0 0 #4c71f2; }
.ring-dot { color: #4c71f2; font-size: 10px; }
.tab-title { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tab.add { justify-content: center; cursor: pointer; }
.pane-area { position: relative; flex: 1 1 auto; background: #11111b; }
.pane {
    position: absolute; overflow: hidden; background: #1e1e2e;
    border: 1px solid #313244;
}
.pane.focused { border-color: #4c71f2; box-shadow: inset 0 0 0 1px #4c71f2; }
.pane.attention {
    border-color: #4c71f2;
    box-shadow: 0 0 0 2px #4c71f2, 0 0 12px #4c71f2;
}
.pane.exited { opacity: 0.55; }
.grid {
    font-family: "JetBrains Mono", "DejaVu Sans Mono", monospace;
    line-height: 1.2; white-space: pre; padding: 6px; height: 100%;
    overflow: hidden;
}
.row { white-space: pre; }
"#;
