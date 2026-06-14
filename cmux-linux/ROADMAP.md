# cmux-linux roadmap

Honest status of the Fedora reimplementation. This is a foundation under active
construction, not feature parity with the macOS app.

## Done

- [x] **cmux-core** — workspaces, vertical tabs, binary split tree, spatial
      focus navigation, notification rings + feed, closed-tab history. Fully
      unit-tested.
- [x] **cmux-config** — `cmux.json` model (appearance, sidebar, notifications,
      keyboard shortcuts) with serde defaults and JSON-path get/set.
- [x] **cmux-pty** — PTY-backed process sessions (spawn/write/resize/events).
- [x] **cmux-term** — VT/ANSI parser into a renderable cell grid (colors, SGR,
      cursor movement, erase, scroll, bell detection).
- [x] **cmux-ipc** — control protocol + Unix-socket server/client.
- [x] **cmux-cli** — the `cmux` CLI driving the app over the socket.
- [x] **packaging** — RPM spec, `.desktop`, systemd user unit, icon, build script.

- [x] **cmux-gui** — Dioxus desktop shell: sidebar with vertical tabs +
      notification badges, split-pane layout, live terminal widget. Wires
      PTY → terminal grid → DOM and forwards input.
- [x] Control socket hosted by the GUI (so the CLI drives the live app).
- [x] **Keyboard-shortcut binding from `cmux.json`** — live key events are
      normalized to chords, matched against the configured shortcut map, and
      dispatched through the same `Engine` actions the CLI uses (split, close,
      new tab/workspace, focus directions, reopen tab, jump to latest unread).
- [x] **Pane sizing** — each pane measures its rendered box on mount and resizes
      its PTY + grid to fit (cell metrics derived from the font size).

- [x] **Notification panel** — slide-in panel from the sidebar bell listing the
      feed; click to jump to a pane, "mark all read"; CLI `notifications` /
      `mark-read`. (README headline feature.)
- [x] **Command palette** — fuzzy-searchable action list (`ctrl+shift+p`),
      executes through the shared action path; shows bound chords.
- [x] **Session persistence** — topology saved to `session.json` and restored on
      startup (ids preserved); shells respawn for restored panes.

- [x] **Settings view** — in-app panel (`ctrl+comma`) editing theme, font size,
      opacity, sidebar width/position, vertical tabs, and notification options;
      changes validate, apply live, and persist to `cmux.json`.

- [x] **Live theming** — light/dark palettes via CSS variables (Catppuccin
      Mocha/Latte), including terminal default colors; switches instantly from
      Settings.

## In progress

- [ ] Background opacity (persisted; needs window transparency to apply visually).
- [ ] OS-appearance detection for the "system" theme (resolves to dark for now).

## Planned

- [ ] Re-measure panes on window/divider resize (currently measured on mount).
- [ ] Pane drag-to-reorder and divider drag-to-resize.
- [ ] Session persistence / restore across restarts.
- [ ] Agent hooks parity (notification triggers from coding-agent lifecycle).
- [ ] Theme/font live config application.

## Out of scope (no Linux equivalent / deliberately dropped for v1)

- Ghostty's GPU (Metal) renderer embedded in-window — we render the VT grid in
  the webview instead. (A future GTK4 + Ghostty-apprt variant could restore it.)
- ExtensionKit sidebar extensions.
- Sparkle auto-update (replaced by RPM / dnf).
- In-app WKWebView browser pane (nested webviews need extra wiring).
