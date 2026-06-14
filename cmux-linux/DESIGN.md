# cmux-linux — a Fedora/Linux clone of cmux

## What this is

`cmux` (the upstream project this lives next to) is a **macOS** application:
~55k lines of Swift built on AppKit + SwiftUI + WebKit + ExtensionKit, embedding
the Ghostty terminal via a Metal/Objective-C bridge, shipped as an Xcode project
with Apple code signing and Sparkle auto-update.

None of those UI frameworks exist on Linux, so a literal 1:1 port is impossible.
`cmux-linux` is a **faithful reimplementation of the cmux experience** for
Fedora (and Linux generally), written in **Rust** with a **Dioxus** desktop UI.

It reproduces cmux's defining UX:

- **Workspaces** in a vertical sidebar.
- **Vertical (and horizontal) tabs** — cmux's signature layout.
- **Splits** modeled as a binary split tree (like the upstream `bonsplit`).
- **Terminal panes** backed by real PTYs running your shell / coding agents.
- **Notification rings** — a pane glows and its tab badges when an agent wants
  attention; a notification panel lists everything pending.
- **A control socket + `cmux` CLI** so agents and scripts can drive the app
  (`cmux list-workspaces`, `cmux send`, `cmux focus`, `cmux notify`, …),
  mirroring upstream `CMUXCLI`.
- **`cmux.json` configuration** compatible in spirit with upstream
  (appearance, sidebar, notifications, keyboard shortcuts).

## Deliberate differences from upstream

| Upstream (macOS) | cmux-linux (Fedora) | Why |
|---|---|---|
| Ghostty GPU renderer (Metal) embedded via GhosttyKit | `cmux-term` VT parser → cell grid rendered in the Dioxus webview | A native GL terminal surface can't be composited inside a webview DOM node |
| AppKit / SwiftUI | Dioxus (Rust, RSX) over WebKitGTK | Apple frameworks are macOS-only |
| Sparkle auto-update + Apple codesign | RPM package + Fedora repo / dnf | Native Linux distribution |
| ExtensionKit sidebar extensions | (out of scope for v1) | No Linux equivalent |
| In-app WKWebView browser pane | Browser pane via webview `<iframe>` | A basic browser pane works; the scriptable agent-browser API is a follow-up |

## Crate graph

```
cmux-gui (bin: cmux-gui)  ── Dioxus desktop app
  ├── cmux-core           ── workspaces, tabs, split-tree, focus, notifications (pure model)
  ├── cmux-config         ── cmux.json structs + JSON-path get/set
  ├── cmux-term           ── VT parser → grid/cursor/scrollback
  ├── cmux-pty            ── PTY spawn/read/write/resize
  └── cmux-ipc            ── control protocol + Unix-socket server

cmux-cli (bin: cmux)      ── CLI; talks to the running app over the control socket
  ├── cmux-ipc
  └── cmux-config
```

Dependency direction is acyclic. `cmux-core`, `cmux-config`, `cmux-term`,
`cmux-pty` are leaves with no app-specific deps and are unit-tested in isolation.

## Build & run (Fedora)

```bash
sudo dnf install -y rust cargo gtk3-devel webkit2gtk4.1-devel \
    libxdo-devel libappindicator-gtk3-devel
cd cmux-linux
cargo build --release
./target/release/cmux-gui          # launch the app
./target/release/cmux list-workspaces   # drive it from the CLI
```

See `packaging/` for the RPM spec and systemd/desktop integration.

## Status

This is an actively-built foundation, not feature-complete parity. The roadmap
and what currently works are tracked in `ROADMAP.md`.
