<h1 align="center">cmux-linux</h1>
<p align="center">A Fedora/Linux reimplementation of <a href="../README.md">cmux</a> — a terminal multiplexer with vertical tabs and notifications for AI coding agents.</p>

---

## What this is

Upstream **cmux** is a macOS application (~55k lines of Swift on AppKit/SwiftUI,
embedding the Ghostty terminal via Metal). Those UI frameworks are Apple-only, so
a literal 1:1 port to Linux is impossible. **cmux-linux** is a faithful
reimplementation of the cmux *experience* for Fedora, written in **Rust** with a
**Dioxus** desktop UI over WebKitGTK.

It reproduces what makes cmux cmux:

- **Workspaces** in a vertical sidebar.
- **Vertical (and horizontal) tabs** — the signature layout.
- **Split panes** arranged in a binary split tree (like upstream `bonsplit`),
  with spatial focus navigation.
- **Terminal panes** backed by real PTYs running your shell or coding agents,
  rendered by a built-in VT/ANSI emulator.
- **Notification rings** — a pane glows and its tab badges when an agent wants
  attention; a notification feed lists everything pending.
- **A control socket + `cmux` CLI** so agents and scripts can drive the app
  (`cmux list-workspaces`, `cmux send`, `cmux focus`, `cmux notify`, …).
- **`cmux.json` configuration** (appearance, sidebar, notifications, keyboard
  shortcuts) with CLI get/set by dotted path.

See [`DESIGN.md`](DESIGN.md) for the architecture and the deliberate differences
from the macOS app, and [`ROADMAP.md`](ROADMAP.md) for status.

## Install / build on Fedora

```bash
sudo dnf install -y rust cargo gcc gtk3-devel webkit2gtk4.1-devel libxdo-devel

git clone https://github.com/manaflow-ai/cmux
cd cmux/cmux-linux
cargo build --release

./target/release/cmux-gui                 # launch the app
./target/release/cmux list-workspaces     # drive it from another terminal
```

> On Debian/Ubuntu the equivalent dev packages are
> `libgtk-3-dev libwebkit2gtk-4.1-dev libxdo-dev`.

### Build an RPM

```bash
sudo dnf install -y rpm-build
packaging/build-rpm.sh
```

This produces `cmux-*.rpm` under `target/rpmbuild/RPMS/`, installing the
`cmux-gui` and `cmux` binaries, a `.desktop` entry, an icon, and a systemd user
unit (`cmux-daemon.service`).

## The CLI

```text
cmux ping                              # check the app is reachable
cmux list-workspaces                   # print the workspace/tab/pane tree
cmux send --pane surface:3 "ls -la"    # type into a pane
cmux send-key ctrl+c                   # send a chord to the focused pane
cmux focus surface:5                   # focus a pane (or workspace:N / tab:N)
cmux focus-dir left                    # move focus within the active tab
cmux split vertical                    # split the focused pane
cmux equalize                          # reset divider ratios in the active tab
cmux zoom                              # toggle maximize of the focused pane
cmux next-tab / cmux prev-tab          # cycle tabs in the active workspace
cmux new-workspace proj                # create a workspace
cmux close-workspace workspace:2       # close a workspace
cmux reorder-workspace workspace:2 0   # move a workspace in the sidebar
cmux notify surface:3 "Claude" "needs input"
cmux notifications                     # list the feed
cmux mark-read [id]                    # mark one (id) or all notifications read
cmux dismiss 0                         # remove one notification by id
cmux run deploy                        # run a custom cmux.json action
cmux config get appearance.fontSize
cmux config set appearance.theme dark
```

Custom actions live under `actions` in `cmux.json` and show up in the command
palette:

```json
{ "actions": { "deploy": { "command": "make deploy", "label": "Deploy", "target": "currentPane" } } }
```

Keyboard: drag to select terminal text, `ctrl+shift+c` / `ctrl+shift+v` to
copy/paste, `ctrl+shift+o` equalize, `ctrl+shift+m` zoom, `ctrl+shift+q` close
workspace, `ctrl+1`…`ctrl+9` select workspace 1–9, `ctrl+tab` /
`ctrl+shift+tab` cycle tabs (all rebindable in `cmux.json`).

The socket path is `$CMUX_SOCKET` or `$XDG_RUNTIME_DIR/cmux/control.sock`.

## Workspace layout

| Crate | Role |
|---|---|
| `cmux-core` | Topology model: workspaces, tabs, split tree, focus, notifications |
| `cmux-config` | `cmux.json` model + JSON-path get/set |
| `cmux-term` | VT/ANSI parser → renderable cell grid |
| `cmux-pty` | PTY spawn/read/write/resize |
| `cmux-ipc` | Control protocol + Unix-socket server/client |
| `cmux-cli` | The `cmux` command-line tool |
| `cmux-gui` | The Dioxus desktop application |

## Testing

```bash
cargo test            # all crates
cargo test -p cmux-core
```

## License

MIT, same as upstream cmux.
