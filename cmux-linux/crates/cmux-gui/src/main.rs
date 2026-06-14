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
mod palette;
mod render;

use std::sync::{Arc, Mutex, OnceLock};

use dioxus::desktop::{Config as DesktopConfig, WindowBuilder};
use dioxus::events::Key;
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

#[derive(Clone, PartialEq)]
struct NotifView {
    pane: PaneId,
    title: String,
    body: String,
    read: bool,
}

#[derive(Clone, PartialEq, Default)]
struct Snapshot {
    workspaces: Vec<WorkspaceView>,
    tabs: Vec<TabView>,
    panes: Vec<PaneView>,
    notifications: Vec<NotifView>,
    unread: usize,
    sidebar_width: f32,
    sidebar_left: bool,
    font_size: f32,
    theme: &'static str,
    opacity: f32,
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

    let notifications = e
        .notifications()
        .iter()
        .rev()
        .map(|n| NotifView {
            pane: n.pane,
            title: n.title.clone(),
            body: n.body.clone(),
            read: n.read,
        })
        .collect();

    Snapshot {
        workspaces,
        tabs,
        panes,
        notifications,
        unread: e.state.notifications.unread_count(),
        sidebar_width: e.config.sidebar.width,
        sidebar_left: e.config.sidebar.position == SidebarPosition::Left,
        font_size: e.config.appearance.font_size,
        theme: match e.config.appearance.theme {
            cmux_config::Theme::Light => "light",
            // "system" resolves to dark until OS-appearance detection lands.
            _ => "dark",
        },
        opacity: e.config.appearance.opacity.clamp(0.3, 1.0),
    }
}

// ---- components ------------------------------------------------------------

#[component]
fn App() -> Element {
    let mut tick = use_signal(|| 0u64);
    let mut show_notifications = use_signal(|| false);
    let mut show_palette = use_signal(|| false);
    let mut show_settings = use_signal(|| false);

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

    let sidebar = rsx! { Sidebar { snap: snap.clone(), tick, show_notifications } };
    let main = rsx! { PaneArea { snap: snap.clone(), tick } };

    rsx! {
        style { {BASE_CSS} }
        div {
            class: "app theme-{snap.theme}",
            style: "--term-opacity:{snap.opacity};",
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
                        // UI-only actions toggle local view state; everything
                        // else goes through the shared engine action path.
                        let handled = match action.as_str() {
                            "toggleNotifications" => {
                                let v = !show_notifications();
                                show_notifications.set(v);
                                true
                            }
                            "commandPalette" => {
                                let v = !show_palette();
                                show_palette.set(v);
                                true
                            }
                            "openSettings" => {
                                let v = !show_settings();
                                show_settings.set(v);
                                true
                            }
                            "jumpToLatestNotification" => {
                                let h = e.lock().unwrap().dispatch_action(&action);
                                show_notifications.set(false);
                                h
                            }
                            _ => e.lock().unwrap().dispatch_action(&action),
                        };
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
            if show_notifications() {
                NotificationPanel { snap: snap.clone(), tick, show_notifications }
            }
            if show_palette() {
                CommandPalette { tick, show_palette, show_notifications }
            }
            if show_settings() {
                SettingsPanel { tick, show_settings }
            }
        }
    }
}

#[component]
fn SettingsPanel(tick: Signal<u64>, show_settings: Signal<bool>) -> Element {
    // Read current values under a brief lock.
    let cfg = engine().lock().unwrap().config.clone();
    let theme = match cfg.appearance.theme {
        cmux_config::Theme::System => "system",
        cmux_config::Theme::Light => "light",
        cmux_config::Theme::Dark => "dark",
    };
    let position = match cfg.sidebar.position {
        SidebarPosition::Left => "left",
        SidebarPosition::Right => "right",
    };

    rsx! {
        div {
            class: "settings-backdrop",
            onclick: move |_| show_settings.set(false),
            div {
                class: "settings",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "settings-head",
                    span { "Settings" }
                    button {
                        class: "settings-close",
                        onclick: move |_| show_settings.set(false),
                        "✕"
                    }
                }
                div { class: "settings-body",
                    SettingRow { label: "Theme",
                        select {
                            value: "{theme}",
                            onchange: move |evt| apply_config("appearance.theme", &evt.value(), tick),
                            option { value: "system", "System" }
                            option { value: "light", "Light" }
                            option { value: "dark", "Dark" }
                        }
                    }
                    SettingRow { label: "Font size",
                        input {
                            r#type: "number", min: "8", max: "32",
                            value: "{cfg.appearance.font_size}",
                            onchange: move |evt| apply_config("appearance.fontSize", &evt.value(), tick),
                        }
                    }
                    SettingRow { label: "Background opacity",
                        input {
                            r#type: "number", min: "0.3", max: "1", step: "0.05",
                            value: "{cfg.appearance.opacity}",
                            onchange: move |evt| apply_config("appearance.opacity", &evt.value(), tick),
                        }
                    }
                    SettingRow { label: "Sidebar width",
                        input {
                            r#type: "number", min: "120", max: "480",
                            value: "{cfg.sidebar.width}",
                            onchange: move |evt| apply_config("sidebar.width", &evt.value(), tick),
                        }
                    }
                    SettingRow { label: "Sidebar position",
                        select {
                            value: "{position}",
                            onchange: move |evt| apply_config("sidebar.position", &evt.value(), tick),
                            option { value: "left", "Left" }
                            option { value: "right", "Right" }
                        }
                    }
                    SettingRow { label: "Vertical tabs",
                        input {
                            r#type: "checkbox",
                            checked: cfg.sidebar.vertical_tabs,
                            onchange: move |evt| apply_config("sidebar.verticalTabs", &bool_str(&evt.value()), tick),
                        }
                    }
                    SettingRow { label: "Notifications enabled",
                        input {
                            r#type: "checkbox",
                            checked: cfg.notifications.enabled,
                            onchange: move |evt| apply_config("notifications.enabled", &bool_str(&evt.value()), tick),
                        }
                    }
                    SettingRow { label: "Ring on bell",
                        input {
                            r#type: "checkbox",
                            checked: cfg.notifications.ring_on_bell,
                            onchange: move |evt| apply_config("notifications.ringOnBell", &bool_str(&evt.value()), tick),
                        }
                    }
                }
                div { class: "settings-foot", "Saved to ~/.config/cmux/cmux.json" }
            }
        }
    }
}

#[component]
fn SettingRow(label: String, children: Element) -> Element {
    rsx! {
        div { class: "settings-row",
            span { class: "settings-label", "{label}" }
            div { class: "settings-control", {children} }
        }
    }
}

/// Checkbox `onchange` reports "true"/"false" or "on"/""; normalize to JSON bool.
fn bool_str(v: &str) -> String {
    if v == "true" || v == "on" {
        "true".into()
    } else {
        "false".into()
    }
}

/// Apply a settings edit through the engine (validates + persists cmux.json).
fn apply_config(path: &str, value: &str, mut tick: Signal<u64>) {
    let _ = engine().lock().unwrap().set_config(path, value);
    tick += 1;
}

/// Execute a palette action through the same paths as keyboard shortcuts.
/// Signals are `Copy`, so this is safe to call from multiple event closures.
fn run_palette_action(
    id: &str,
    mut tick: Signal<u64>,
    mut show_palette: Signal<bool>,
    mut show_notifications: Signal<bool>,
) {
    match id {
        "toggleNotifications" => {
            let v = !show_notifications();
            show_notifications.set(v);
        }
        _ => {
            engine().lock().unwrap().dispatch_action(id);
        }
    }
    show_palette.set(false);
    tick += 1;
}

#[component]
fn CommandPalette(
    tick: Signal<u64>,
    show_palette: Signal<bool>,
    show_notifications: Signal<bool>,
) -> Element {
    let mut query = use_signal(String::new);
    let shortcuts = {
        let g = engine().lock().unwrap();
        g.config.keyboard_shortcuts.clone()
    };
    let all = palette::all_actions(&shortcuts);
    let results = palette::filter_actions(&query(), &all);
    let top = results.first().map(|a| a.id.clone());

    rsx! {
        div {
            class: "palette-backdrop",
            onclick: move |_| show_palette.set(false),
            div {
                class: "palette",
                onclick: move |evt| evt.stop_propagation(),
                input {
                    class: "palette-input",
                    autofocus: true,
                    placeholder: "Type a command…",
                    value: "{query}",
                    oninput: move |evt| query.set(evt.value()),
                    onkeydown: move |evt| {
                        match evt.key() {
                            Key::Escape => {
                                show_palette.set(false);
                                evt.prevent_default();
                            }
                            Key::Enter => {
                                if let Some(id) = top.clone() {
                                    run_palette_action(&id, tick, show_palette, show_notifications);
                                }
                                evt.prevent_default();
                            }
                            _ => {}
                        }
                    },
                }
                div { class: "palette-list",
                    for a in results.iter().cloned() {
                        div {
                            key: "{a.id}",
                            class: "palette-item",
                            onclick: move |_| run_palette_action(&a.id, tick, show_palette, show_notifications),
                            span { class: "palette-label", "{a.label}" }
                            if let Some(sc) = a.shortcut.clone() {
                                span { class: "palette-chord", "{sc}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn NotificationPanel(
    snap: Snapshot,
    tick: Signal<u64>,
    show_notifications: Signal<bool>,
) -> Element {
    rsx! {
        div {
            class: "notif-backdrop",
            onclick: move |_| show_notifications.set(false),
            div {
                class: "notif-panel",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "notif-head",
                    span { "Notifications" }
                    button {
                        class: "notif-clear",
                        onclick: move |_| {
                            engine().lock().unwrap().mark_all_read();
                            tick += 1;
                        },
                        "Mark all read"
                    }
                }
                if snap.notifications.is_empty() {
                    div { class: "notif-empty", "No notifications" }
                }
                for (i, n) in snap.notifications.iter().cloned().enumerate() {
                    div {
                        key: "{i}",
                        class: if n.read { "notif-item read" } else { "notif-item" },
                        onclick: move |_| {
                            engine().lock().unwrap().state.focus_pane(n.pane);
                            show_notifications.set(false);
                            tick += 1;
                        },
                        div { class: "notif-title", "{n.title}" }
                        div { class: "notif-body", "{n.body}" }
                    }
                }
            }
        }
    }
}

#[component]
fn Sidebar(snap: Snapshot, tick: Signal<u64>, show_notifications: Signal<bool>) -> Element {
    let width = snap.sidebar_width;
    let mut show_notifications = show_notifications;
    rsx! {
        div {
            class: "sidebar",
            style: "width:{width}px;",
            div { class: "sidebar-header",
                span { class: "logo", "cmux" }
                button {
                    class: if snap.unread > 0 { "unread" } else { "unread zero" },
                    title: "Notifications",
                    onclick: move |_| {
                        let v = !show_notifications();
                        show_notifications.set(v);
                        tick += 1;
                    },
                    if snap.unread > 0 { "{snap.unread}" } else { "🔔" }
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
.app.theme-dark {
    --bg:#181825; --panel:#1e1e2e; --deep:#11111b; --panel2:#313244;
    --border:#313244; --border-strong:#45475a; --text:#cdd6f4; --text-dim:#bac2de;
    --muted:#6c7086; --accent:#4c71f2; --on-accent:#ffffff;
    --term-fg:#cdd6f4; --term-bg:#1e1e2e; --overlay:rgba(0,0,0,0.4);
}
.app.theme-light {
    --bg:#eff1f5; --panel:#e6e9ef; --deep:#dce0e8; --panel2:#ccd0da;
    --border:#ccd0da; --border-strong:#bcc0cc; --text:#4c4f69; --text-dim:#5c5f77;
    --muted:#8c8fa1; --accent:#1e66f5; --on-accent:#ffffff;
    --term-fg:#4c4f69; --term-bg:#eff1f5; --overlay:rgba(60,60,80,0.35);
}
.app {
    display: flex; flex-direction: row; height: 100vh; outline: none;
    font-family: -apple-system, system-ui, sans-serif;
    background: var(--bg); color: var(--text); overflow: hidden;
}
.sidebar {
    display: flex; flex-direction: column; flex: 0 0 auto;
    background: var(--panel); border-right: 1px solid var(--border); overflow-y: auto;
}
.sidebar-header {
    display: flex; align-items: center; justify-content: space-between;
    padding: 10px 12px; font-weight: 600; border-bottom: 1px solid var(--border);
}
.logo { color: var(--accent); }
.unread {
    background: var(--accent); color: var(--on-accent); border-radius: 10px;
    padding: 1px 7px; font-size: 11px;
}
.workspaces { display: flex; flex-wrap: wrap; gap: 4px; padding: 8px; }
.ws {
    background: var(--panel2); color: var(--text); border: none; border-radius: 6px;
    padding: 4px 8px; font-size: 12px; cursor: pointer;
}
.ws.active { background: var(--accent); color: var(--on-accent); }
.ws.add, .tab.add { background: transparent; border: 1px dashed var(--border-strong); color: var(--muted); }
.tabs { display: flex; flex-direction: column; gap: 2px; padding: 8px; }
.tab {
    display: flex; align-items: center; gap: 6px; padding: 8px 10px;
    border-radius: 6px; cursor: pointer; font-size: 13px; color: var(--text-dim);
}
.tab:hover { background: var(--panel2); }
.tab.active { background: var(--panel2); color: var(--text); box-shadow: inset 2px 0 0 var(--accent); }
.ring-dot { color: var(--accent); font-size: 10px; }
.tab-title { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tab.add { justify-content: center; cursor: pointer; }
.pane-area { position: relative; flex: 1 1 auto; background: var(--deep); }
.pane {
    position: absolute; overflow: hidden; background: var(--term-bg);
    border: 1px solid var(--border);
}
.pane.focused { border-color: var(--accent); box-shadow: inset 0 0 0 1px var(--accent); }
.pane.attention {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent), 0 0 12px var(--accent);
}
.pane.exited { opacity: 0.55; }
.grid {
    font-family: "JetBrains Mono", "DejaVu Sans Mono", monospace;
    line-height: 1.2; white-space: pre; padding: 6px; height: 100%;
    overflow: hidden; color: var(--term-fg); background: var(--term-bg);
}
.row { white-space: pre; }
.unread { cursor: pointer; border: none; }
.unread.zero { background: transparent; color: var(--muted); padding: 0; font-size: 14px; }
.notif-backdrop {
    position: fixed; inset: 0; background: var(--overlay);
    display: flex; justify-content: flex-end; z-index: 50;
}
.notif-panel {
    width: 360px; height: 100%; background: var(--panel);
    border-left: 1px solid var(--border); display: flex; flex-direction: column;
    box-shadow: -8px 0 24px rgba(0,0,0,0.4);
}
.notif-head {
    display: flex; align-items: center; justify-content: space-between;
    padding: 12px 14px; border-bottom: 1px solid var(--border); font-weight: 600;
}
.notif-clear {
    background: var(--panel2); color: var(--text-dim); border: none; border-radius: 6px;
    padding: 4px 8px; font-size: 12px; cursor: pointer;
}
.notif-empty { padding: 24px; color: var(--muted); text-align: center; }
.notif-item {
    padding: 10px 14px; border-bottom: 1px solid var(--border); cursor: pointer;
    border-left: 3px solid var(--accent);
}
.notif-item:hover { background: var(--panel2); }
.notif-item.read { border-left-color: transparent; opacity: 0.7; }
.notif-title { font-size: 13px; font-weight: 600; }
.notif-body { font-size: 12px; color: var(--muted); }
.palette-backdrop {
    position: fixed; inset: 0; background: var(--overlay);
    display: flex; justify-content: center; align-items: flex-start; z-index: 60;
}
.palette {
    margin-top: 12vh; width: 520px; max-width: 90vw; background: var(--panel);
    border: 1px solid var(--border-strong); border-radius: 10px; overflow: hidden;
    box-shadow: 0 16px 48px rgba(0,0,0,0.5);
}
.palette-input {
    width: 100%; border: none; outline: none; background: var(--bg);
    color: var(--text); font-size: 15px; padding: 14px 16px;
    border-bottom: 1px solid var(--border);
}
.palette-list { max-height: 50vh; overflow-y: auto; }
.palette-item {
    display: flex; align-items: center; justify-content: space-between;
    padding: 10px 16px; cursor: pointer; font-size: 13px;
}
.palette-item:hover { background: var(--panel2); }
.palette-chord { color: var(--muted); font-size: 11px; font-family: monospace; }
.settings-backdrop {
    position: fixed; inset: 0; background: var(--overlay);
    display: flex; justify-content: center; align-items: flex-start; z-index: 70;
}
.settings {
    margin-top: 8vh; width: 480px; max-width: 92vw; background: var(--panel);
    border: 1px solid var(--border-strong); border-radius: 10px; overflow: hidden;
    box-shadow: 0 16px 48px rgba(0,0,0,0.5);
}
.settings-head {
    display: flex; align-items: center; justify-content: space-between;
    padding: 14px 16px; border-bottom: 1px solid var(--border); font-weight: 600;
}
.settings-close { background: none; border: none; color: var(--muted); cursor: pointer; font-size: 14px; }
.settings-body { padding: 8px 16px; max-height: 60vh; overflow-y: auto; }
.settings-row {
    display: flex; align-items: center; justify-content: space-between;
    padding: 10px 0; border-bottom: 1px solid var(--border); font-size: 13px;
}
.settings-label { color: var(--text-dim); }
.settings-control input, .settings-control select {
    background: var(--bg); color: var(--text); border: 1px solid var(--border);
    border-radius: 6px; padding: 4px 8px; font-size: 13px;
}
.settings-control input[type=checkbox] { width: 16px; height: 16px; }
.settings-foot { padding: 10px 16px; color: var(--muted); font-size: 11px; border-top: 1px solid var(--border); }
"#;
