# cmux-linux parity matrix

A behavior-by-behavior comparison of the **original macOS cmux** against this
**cmux-linux** reimplementation. The original was inventoried from its
authoritative surfaces (keyboard-shortcut registry, control-socket command
coordinators, `cmux.json` schema, Settings UI, command palette, menus, docs, and
`CHANGELOG.md`); cmux-linux was checked against its actual code
(`cmux-cli`, `cmux-ipc` protocol, `cmux-config`, `cmux-core`, `cmux-gui`).

Legend:

- ✅ implemented
- 🟡 partial / basic / config-only (key stored but not fully applied)
- ❌ in scope for a Linux desktop port, but not implemented yet
- ⬜ out of scope (depends on Apple frameworks, the cloud backend, the iOS app,
  or the web backend — can't port literally)

cmux-linux is deliberately a faithful port of the **desktop-core UX**, not the
whole product. The original additionally ships an iOS app, a cloud-VM control
plane, agent-session web renderers, a diff/review viewer, AppleScript, a Dock
tile, a menu-bar extra, browser automation, sidebar extensions, and more — all
tracked here but marked ⬜.

---

## 1. Terminal & rendering

| Original behavior | cmux-linux |
|---|---|
| VT/ANSI emulation: colors, SGR, cursor moves, erase, scroll regions | ✅ `cmux-term` |
| 256-color + truecolor palette, default fg/bg from theme | ✅ |
| Scrollback history + mouse-wheel scroll through it | ✅ (10k-line cap; alt-screen excluded) |
| In-terminal find: search scrollback+screen, next, match count | ✅ (`ctrl+shift+f`, `cmux find`) |
| Wide characters (CJK/emoji occupy two cells) | ✅ (`unicode-width`) |
| OSC 0/1/2 window/icon title → pane & tab title | ✅ |
| OSC 9 / OSC 777 → notifications | ✅ |
| Terminal bell (BEL) detection → pane ring | ✅ (`take_bell` + `ringOnBell`) |
| Resize PTY+grid to fit pane; re-measure on window/divider resize | ✅ |
| Live theming: light/dark palettes, `system` follows OS | ✅ (Catppuccin; `prefers-color-scheme`) |
| Background opacity (desktop shows through) | ✅ (`color-mix`) |
| Configurable font size | ✅ |
| Alt-screen tracking | 🟡 (excluded from scrollback; no mouse-reporting) |
| OSC 7 (working directory) | ❌ |
| OSC 8 hyperlinks + ⌘/Ctrl-click to open | ❌ |
| OSC 133 prompt marks (shell integration) | ❌ |
| OSC 99 (kitty notifications) | ❌ |
| Mouse text selection | ❌ |
| Copy selection / paste to PTY | ❌ |
| Rich-text / image paste, paste bracketing | ⬜ / ❌ |
| Vim-style copy mode | ❌ |
| Mouse reporting to TUIs (SGR/X11) | ❌ |
| Right-click context menu (copy/open) | ❌ |
| Configurable font family | 🟡 (key stored; fixed monospace stack rendered) |
| Cursor style (block/beam/underline) | 🟡 (key stored; no cursor drawn) |
| Ligatures | ❌ |
| IME / dead-key composition (CJK input) | ❌ |
| Metal GPU renderer, IOSurface, CVDisplayLink | ⬜ (DOM cell grid instead) |
| Background blur / vibrancy / glass | ⬜ (macOS NSVisualEffectView) |
| Bell audio with volume | ❌ |

## 2. Workspaces, tabs, splits, navigation

| Original behavior | cmux-linux |
|---|---|
| Workspaces in a vertical sidebar | ✅ |
| New workspace (sidebar `+`, shortcut, CLI) | ✅ |
| Select/focus workspace (click, CLI `focus workspace:N`) | ✅ |
| Rename workspace | ✅ (`cmux rename-workspace`) |
| Vertical tabs (signature layout) | ✅ |
| New tab / new surface | ✅ |
| Close tab | ✅ |
| Rename tab | ✅ (`cmux rename-tab`) |
| Reorder tabs (drag + CLI) | ✅ (`reorder_tab`) |
| Tab attention dot + sidebar unread badge | ✅ |
| Binary split tree (bonsplit-equivalent) | ✅ `cmux-core::split` |
| Split horizontal / vertical | ✅ |
| Close pane | ✅ |
| Spatial focus navigation (left/right/up/down) | ✅ (`focus_dir`) |
| Divider drag-to-resize | ✅ (`set_ratio_by_index`) |
| Reopen most-recently-closed tab | ✅ |
| Command palette (fuzzy) | ✅ (`ctrl+shift+p`) |
| Next/previous tab cycling | ✅ (`ctrl+tab`/`ctrl+shift+tab`, `cmux next-tab`) |
| Close workspace | ✅ (`ctrl+shift+q`, `cmux close-workspace`) |
| Equalize splits | ✅ (`ctrl+shift+o`, `cmux equalize`) |
| Zoom / maximize pane | ✅ (`ctrl+shift+m`, `cmux zoom`) |
| Select workspace by number (`ctrl+1`–`ctrl+9`) | ✅ |
| Reorder workspaces | 🟡 (`cmux reorder-workspace`; no sidebar drag yet) |
| Select surface by number | ❌ |
| Move tab/surface to another workspace | ❌ |
| Close other tabs in pane | ❌ |
| Swap panes / break / join | ❌ |
| Swap panes / break / join | ❌ |
| Reopen closed workspace / window | ❌ |
| Focus history back/forward | ❌ |
| Multiple windows | ❌ (single window) |
| Pin workspace; workspace color/icon | ❌ |
| Workspace groups (collapsible sections, anchors, per-cwd config) | ❌ |
| Right sidebar (Files/Find/Sessions/Feed/Dock) | ❌ (notification slide-in only) |
| Horizontal tab bar option | ❌ (always vertical list) |

## 3. Notifications

| Original behavior | cmux-linux |
|---|---|
| Pane notification ring (glow) | ✅ |
| Tab attention badge + sidebar unread count | ✅ |
| Notification feed / panel | ✅ (slide-in panel) |
| Raise via OSC 9 / OSC 777 and bell | ✅ |
| `cmux notify` from CLI | ✅ |
| List notifications (CLI + panel) | ✅ |
| Mark all read (CLI + panel) | ✅ |
| Jump to latest unread | ✅ |
| Click notification → focus its pane | ✅ |
| Per-item mark-read / dismiss | ✅ (`cmux mark-read <id>` / `dismiss <id>`) |
| `open-notification` / `jump-to-unread` as CLI verbs | ❌ (jump is shortcut-only) |
| OS desktop notifications (notification center / libnotify) | ❌ |
| Notification sounds (built-in + custom file) | ❌ (`notifications.sound` key stored, not played) |
| Notification hooks in `cmux.json` | ❌ |
| iPhone forwarding, cross-device dismiss-sync | ⬜ |
| Dock badge | ⬜ (macOS) |

## 4. CLI / control socket

The original exposes ~115 socket verbs across window/workspace/group/pane/
surface/notification/feed/system/project domains. cmux-linux implements a
focused subset (~23 verbs) over a Unix socket at
`$CMUX_SOCKET` / `$XDG_RUNTIME_DIR/cmux/control.sock`.

| Original command (family) | cmux-linux |
|---|---|
| `ping` | ✅ |
| `list-workspaces` / `tree` | ✅ (`list-workspaces`/`ls`) |
| `send` (surface.send_text) | ✅ |
| `send-key` (surface.send_key) | ✅ |
| `focus` pane/surface/workspace | ✅ (`focus`, `focus-dir`) |
| `split` / new-pane / surface.split | ✅ |
| `close-surface` / close-pane | ✅ (`close-pane`) |
| `new-surface` / new-tab | ✅ (`new-tab`) |
| `new-workspace` | ✅ |
| `rename-tab` / `rename-workspace` | ✅ |
| `reorder-surface` | ✅ (`reorder-tab`) |
| `notify` / `list-notifications` / `mark-read` | ✅ |
| `read-screen` (surface.read_text) | ✅ (`snapshot`) |
| `config get` / `config set` (dotted path) | ✅ |
| `browser` open + `navigate` | ✅ |
| in-terminal `find` | ✅ |
| pane resize | ✅ (`resize`) |
| `equalize` / `zoom` (toggle) | ✅ |
| `next-tab` / `prev-tab` | ✅ |
| `close-workspace` / `reorder-workspace` | ✅ |
| `mark-read <id>` / `dismiss <id>` | ✅ |
| `identify` / `capabilities` / `system.tree` | ❌ |
| window list/create/close/focus/`display` | ❌ |
| `select-workspace` / next/prev/last workspace | ❌ (focus by id works) |
| `move-surface` / `move-tab-to-new-workspace` / `split-off` | ❌ |
| pane swap / break / join | ❌ |
| `trigger-flash` | ❌ |
| surface respawn / health / resume get/set/clear | ❌ |
| workspace-group namespace | ❌ |
| dismiss/open-notification, jump-to-unread (CLI) | ❌ |
| `top` / `memory` | ❌ |
| sidebar-extension verbs (set-status, report-ports/pr/git, log, progress…) | ❌ ⬜ |
| `auth`/`login`/`logout` | ⬜ |
| `vm`/`cloud`, `ssh`, remote workspaces | ⬜ |
| AppleScript bridge | ⬜ |

## 5. Configuration (`cmux.json`)

| Original key/behavior | cmux-linux |
|---|---|
| JSON-path get/set by dotted path | ✅ |
| `appearance.theme` (system/light/dark) | ✅ |
| `appearance.fontSize` | ✅ |
| `appearance.opacity` | ✅ |
| `appearance.fontFamily` | ✅ (applied to the terminal grid) |
| `appearance.cursorStyle` | 🟡 (stored, no cursor drawn) |
| `sidebar.position` (left/right) | ✅ |
| `sidebar.width` | ✅ |
| `sidebar.showNotificationBadges` | ✅ |
| `sidebar.verticalTabs` | 🟡 (stored; layout always vertical) |
| `notifications.enabled` / `ringOnBell` | ✅ |
| `notifications.sound` | 🟡 (stored, not played) |
| `keyboardShortcuts.*` (map, editable, CLI) | ✅ |
| `shell` override (else `$SHELL`) | ✅ |
| Custom `actions` (palette/CLI commands) | ❌ |
| `commands` (custom CLI commands) | ❌ |
| `ui.surfaceTabBar.buttons` / plus-button behavior | ❌ |
| `newWorkspaceCommand` | ❌ |
| `workspaceGroups.byCwd` (color/icon/contextMenu) | ❌ |
| `notifications.hooks` | ❌ |
| `vault` (agent sessions) | ⬜ |

## 6. Settings UI & keyboard shortcuts

| Original behavior | cmux-linux |
|---|---|
| In-app Settings panel (`⌘,` / `ctrl+comma`) | ✅ (theme, font size, opacity, sidebar width/position, vertical tabs, notification toggles; validates + persists live) |
| Configurable keyboard shortcuts (in `cmux.json`) | ✅ (19 default bindings) |
| Working bound actions (split, close, new tab/ws, focus dirs, palette, find, notifications, jump-unread, reopen tab, settings) | ✅ (~17 of 19) |
| Shortcut **editor** inside Settings UI | 🟡 (edit via `cmux.json`, no in-UI editor) |
| `when`-context clauses / VS Code-style context keys | ❌ |
| Global system-wide hotkey | ❌ |
| Settings sections: Account, Mobile, Browser, Automation, Custom Sidebars, Beta, Workspace Colors… | ❌ / ⬜ |

## 7. Browser pane

| Original behavior | cmux-linux |
|---|---|
| Browser pane alongside terminals | ✅ (`ctrl+shift+b`, `cmux browser <url>`) |
| URL bar + navigate (`cmux navigate`) | ✅ |
| Loads arbitrary sites | 🟡 (`<iframe>`; sites sending `X-Frame-Options`/CSP frame-ancestors won't load) |
| Back/forward, reload, zoom, find-in-page | ❌ |
| DevTools / JS console / focus mode | ❌ |
| Profiles, history, import (bookmarks/cookies) | ❌ |
| Scriptable automation API (`react-grab`, devtools, console, zoom, history) | ❌ |
| Browser workspace, mute, popups, WebAuthn, proxy mirror | ❌ / ⬜ |
| ⌘-click terminal links route into the browser | ❌ |

## 8. Session & persistence

| Original behavior | cmux-linux |
|---|---|
| Save topology + restore on launch (ids preserved, shells respawn) | ✅ (`session.json`) |
| Scrollback persistence across restart | ❌ (topology only) |
| Rolling backup / corrupt-snapshot recovery | ❌ |
| Agent session resume (Claude/Codex/Hermes/OMO/Kiro), hook restore | ⬜ |

## 9. Out of scope on Linux (⬜ — not portable / different product surface)

These exist in the original but depend on Apple frameworks, the iOS app, the
cloud backend, or the web backend, and are intentionally not part of cmux-linux:

- **Agent sessions:** React/Solid session web renderers, executable resolution,
  hibernation, fork/resume, permission modes, provider bridges.
- **Cloud VM control plane:** `cmux vm/cloud` lifecycle, device registry, remote
  workspaces, SSH attach, phone push.
- **iOS companion app:** pairing/QR, multi-Mac host switcher, terminal composer,
  toolbar, image paste, notification forwarding, onboarding.
- **macOS integrations:** AppleScript automation, Dock tile plugin, menu-bar
  extra, `cmux://`/`ssh://` URL schemes + default-terminal registration,
  Sparkle auto-update (replaced by RPM/dnf), QuickLook preview.
- **Rich viewers:** diff/review viewer (`cmux diff`, per-repo comments), file
  preview (PDF/image/media), Markdown viewer, Xcode project visualizer.
- **Sidebar extensions:** out-of-process custom sidebars + their CLI surface
  (status/meta/log/progress/ports/PR/git reporting).
- **Shell/agent hooks:** `cmux hooks bash|zsh|fish|omp`, Claude/Hermes/OMO/Kiro
  hook integrations (the OSC-based notify path is supported instead).
- **Global search, file drops, crash-report scrubbing, webview asset splitting.**

---

## Headline coverage (desktop-core, excluding ⬜)

- **Terminal/rendering:** core emulation, scrollback, find, wide chars, OSC
  title/notify, bell, theming, opacity, resize — ✅. Missing the big ones:
  **mouse selection + copy/paste**, OSC 8 hyperlinks, IME, copy mode.
- **Layout/navigation:** workspaces, vertical tabs, splits, focus nav, divider
  drag, tab reorder + cycling, equalize, zoom, close/select workspace,
  reopen-tab, palette — ✅. Missing: move-to-workspace, workspace groups,
  multiple windows, right sidebar.
- **Notifications:** rings/badges/feed/OSC/bell/jump, per-item mark-read +
  dismiss — ✅. Missing: OS desktop notifications, sounds, hooks.
- **CLI/config/settings:** ~23 socket verbs, dotted-path config, live Settings,
  editable shortcuts — ✅. Missing: window/group/move/flash/respawn verbs,
  custom actions, shortcut UI editor.
- **Browser/session:** basic browser pane + topology restore — ✅/🟡.

The single most impactful gap for everyday terminal use is **text selection and
clipboard copy/paste**, which the original has but cmux-linux does not yet.
