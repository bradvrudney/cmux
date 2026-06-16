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
- [x] **OSC capture** — pane output drives the UI: OSC 0/1/2 sets the pane title
      (tab/pane labels follow the running program), OSC 9 and OSC 777 raise
      notifications. This is how coding agents signal cmux directly, beyond the
      terminal bell.
- [x] **Resize tracking** — panes re-measure on window/divider resize, not just
      on mount.
- [x] **Wide characters** — CJK/emoji occupy two cells (via `unicode-width`),
      with correct cursor advance, edge wrapping, and snapshot text.
- [x] **System theme follows the OS** — the `system` theme uses
      `prefers-color-scheme` (honored by WebKitGTK), light or dark to match.
- [x] **In-app browser pane** — split a webview browser alongside terminals
      (`ctrl+shift+b` / `cmux browser <url>`), with a URL bar and `cmux navigate`.
      A scriptable browser API remains a follow-up.
- [x] **Divider drag-to-resize** — split boundaries render as draggable handles
      that adjust the split ratio live (core `dividers`/`set_ratio_by_index`
      tested; the mouse interaction itself is pending visual verification on a
      Fedora desktop).
- [x] **Tab drag-to-reorder** — sidebar tabs are draggable; dropping one onto
      another reorders via `reorder_tab` (also CLI-scriptable). Mouse
      interaction pending visual verification on a Fedora desktop.

- [x] **Background opacity** — the window is transparent and surfaces are tinted
      with the opacity setting via `color-mix` (no alpha compounding); the
      desktop shows through at <1.0. Pending visual verification on a compositor.
- [x] **Terminal scrollback** — lines scrolling off the primary screen are kept
      (10k-line cap; alt screen excluded); the mouse wheel scrolls a pane
      through history, and new output snaps back to the live screen.
- [x] **In-terminal find** — case-insensitive search over scrollback + screen
      (`ctrl+shift+f` find bar with match count and next; `cmux find <pane>
      <query>` from the CLI). Match navigation scrolls the pane to the hit.

- [x] **Layout & control additions** — equalize splits (`ctrl+shift+o` /
      `cmux equalize`), zoom/maximize the focused pane (`ctrl+shift+m` /
      `cmux zoom`), next/previous tab cycling (`ctrl+tab` / `ctrl+shift+tab`,
      `cmux next-tab`/`prev-tab`), select workspace 1–9 (`ctrl+1`…`ctrl+9`),
      close workspace (`ctrl+shift+q` / `cmux close-workspace`), reorder
      workspaces (`cmux reorder-workspace`), per-item notification mark-read /
      dismiss (`cmux mark-read <id>` / `dismiss <id>`), and the configured
      `appearance.fontFamily` is now applied to the terminal grid. All routed
      through the shared `Engine` action path (keyboard + palette + CLI).

- [x] **Terminal text selection + clipboard** — drag to select (single- and
      multi-line, rendered with a selection tint), copy with `ctrl+shift+c` and
      paste with `ctrl+shift+v` via the system clipboard (`arboard`, Wayland +
      X11). Typing clears the selection, like a real terminal.

- [x] **OS desktop notifications + cursor** — feed notifications (OSC 9/777,
      bell, `cmux notify`) for non-focused panes post a freedesktop D-Bus
      notification (`notify-rust`), with a sound-name hint when
      `notifications.sound` is set. The focused terminal pane draws a cursor
      (block / bar / underline per `appearance.cursorStyle`).

- [x] **Clickable URLs** — ctrl-click an `http(s)://` URL in a terminal pane to
      open it in a browser split (URL detected under the pointer).
- [x] **Layout completeness** — move a tab to a new or existing workspace
      (`ctrl+shift+u` / `cmux move-tab`), swap the focused pane with a neighbor
      (`cmux swap <dir>` / palette), select surface 1–9 (`alt+1`…`alt+9`), and
      reorder workspaces by dragging them in the sidebar.

- [x] **Working-directory inheritance** — splitting a pane or opening a new tab
      starts the shell in the source pane's current directory, read live from
      `/proc/<pid>/cwd` (OSC 7 is also parsed when a shell emits it). Plus
      "close other tabs" in the command palette.

## Planned

- [ ] OSC 8 escape-sequence hyperlinks (per-cell link data).
- [ ] Custom notification sound files.
- [ ] Scriptable browser API (upstream's agent-browser port) on the browser pane.
- [ ] Highlight find matches in-place (currently scrolls to the match line).

## Out of scope (no Linux equivalent / deliberately dropped for v1)

- Ghostty's GPU (Metal) renderer embedded in-window — we render the VT grid in
  the webview instead. (A future GTK4 + Ghostty-apprt variant could restore it.)
- ExtensionKit sidebar extensions.
- Sparkle auto-update (replaced by RPM / dnf).
- In-app WKWebView browser pane (nested webviews need extra wiring).
