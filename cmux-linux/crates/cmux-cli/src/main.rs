//! `cmux` — the command-line interface for driving a running cmux-linux app.
//!
//! Mirrors upstream `CMUXCLI`: it connects to the control socket and issues a
//! single [`Request`], printing the [`Response`]. Argument parsing is a pure
//! function ([`parse`]) so it can be unit-tested without a live server.

use std::path::PathBuf;
use std::process::ExitCode;

use cmux_core::ids::{PaneId, TabId, WorkspaceId};
use cmux_ipc::protocol::{Dir, Request, Response, SplitDir, Target};
use cmux_ipc::{default_socket_path, Client};

const USAGE: &str = "\
cmux — control a running cmux-linux app

USAGE:
    cmux [--socket PATH] <command> [args]

COMMANDS:
    ping                              Check the app is reachable
    list-workspaces                   Print the workspace/tab/pane tree
    send [--pane ID] <text>           Type text into a pane (default: focused)
    send-key [--pane ID] <key>        Send a key/chord (e.g. enter, ctrl+c)
    focus <target>                    Focus workspace:N | tab:N | surface:N
    focus-dir <left|right|up|down>    Move focus within the active tab
    new-tab [--workspace ID]          Open a new tab
    new-workspace [title]             Create a workspace
    split <horizontal|vertical> [--pane ID]
    close-pane <ID>                   Close a pane
    notify <pane> <title> <body>      Raise an attention notification
    snapshot <pane>                   Print a pane's screen as text
    notifications                     List the notification feed
    mark-read                         Mark all notifications read
    rename-tab <tab> <title>          Rename a tab
    rename-workspace <ws> <title>     Rename a workspace
    reorder-tab <tab> <index>         Move a tab within its workspace
    resize <pane> <rows> <cols>       Resize a pane's PTY/grid
    browser <url> [vertical]          Split into a browser pane
    navigate <pane> <url>             Point a browser pane at a URL
    find <pane> <query>               Search a pane's scrollback + screen
    config get [path]                 Read config (whole tree or dotted path)
    config set <path> <value>         Set a config value

IDs accept either a bare number or a prefixed form (surface:3, tab:2).
The socket path defaults to $CMUX_SOCKET or $XDG_RUNTIME_DIR/cmux/control.sock.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (socket_override, rest) = extract_socket(args);

    let request = match parse(&rest) {
        Ok(Some(req)) => req,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(msg) => {
            eprintln!("cmux: {msg}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    let socket = match socket_override
        .or_else(|| std::env::var_os("CMUX_SOCKET").map(PathBuf::from))
    {
        Some(p) => p,
        None => match default_socket_path() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("cmux: {e}");
                return ExitCode::FAILURE;
            }
        },
    };

    let client = Client::new(&socket);
    match client.send(&request) {
        Ok(resp) => {
            print_response(&resp);
            if matches!(resp, Response::Error { .. }) {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(e) => {
            eprintln!("cmux: cannot reach app at {}: {e}", socket.display());
            ExitCode::FAILURE
        }
    }
}

/// Pull an optional `--socket PATH` out of the args, returning it plus the rest.
fn extract_socket(args: Vec<String>) -> (Option<PathBuf>, Vec<String>) {
    let mut socket = None;
    let mut rest = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "--socket" {
            socket = it.next().map(PathBuf::from);
        } else {
            rest.push(a);
        }
    }
    (socket, rest)
}

/// Parse a command line (without the `--socket` flag) into a [`Request`].
/// `Ok(None)` means "print usage" (no command / help).
fn parse(args: &[String]) -> Result<Option<Request>, String> {
    let Some(cmd) = args.first().map(String::as_str) else {
        return Ok(None);
    };
    if matches!(cmd, "-h" | "--help" | "help") {
        return Ok(None);
    }
    let rest = &args[1..];

    let req = match cmd {
        "ping" => Request::Ping,
        "list-workspaces" | "ls" => Request::ListWorkspaces,
        "notifications" | "notes" => Request::ListNotifications,
        "mark-read" => Request::MarkAllRead,
        "rename-tab" => {
            let tab = rest.first().ok_or("rename-tab requires a tab id")?;
            let title = rest.get(1..).map(|s| s.join(" ")).unwrap_or_default();
            if title.is_empty() {
                return Err("rename-tab requires a title".into());
            }
            Request::RenameTab {
                tab: TabId(parse_id(tab)?),
                title,
            }
        }
        "rename-workspace" => {
            let ws = rest.first().ok_or("rename-workspace requires a workspace id")?;
            let title = rest.get(1..).map(|s| s.join(" ")).unwrap_or_default();
            if title.is_empty() {
                return Err("rename-workspace requires a title".into());
            }
            Request::RenameWorkspace {
                workspace: WorkspaceId(parse_id(ws)?),
                title,
            }
        }
        "reorder-tab" => {
            let tab = rest.first().ok_or("reorder-tab requires a tab id")?;
            let index = rest
                .get(1)
                .ok_or("reorder-tab requires an index")?
                .parse::<usize>()
                .map_err(|_| "index must be a number")?;
            Request::ReorderTab {
                tab: TabId(parse_id(tab)?),
                index,
            }
        }
        "browser" => {
            let url = rest.first().ok_or("browser requires a URL")?.clone();
            let orientation = match rest.get(1).map(String::as_str) {
                Some("vertical") | Some("v") => SplitDir::Vertical,
                _ => SplitDir::Horizontal,
            };
            Request::OpenBrowser { url, orientation }
        }
        "navigate" => {
            let pane = rest.first().ok_or("navigate requires a pane id")?;
            let url = rest.get(1).ok_or("navigate requires a URL")?.clone();
            Request::NavigateBrowser {
                pane: PaneId(parse_id(pane)?),
                url,
            }
        }
        "find" => {
            let pane = rest.first().ok_or("find requires a pane id")?;
            let query = rest.get(1..).map(|s| s.join(" ")).unwrap_or_default();
            if query.is_empty() {
                return Err("find requires a query".into());
            }
            Request::Find {
                pane: PaneId(parse_id(pane)?),
                query,
            }
        }
        "resize" => {
            let pane = rest.first().ok_or("resize requires a pane id")?;
            let rows = rest.get(1).ok_or("resize requires rows")?.parse::<u16>().map_err(|_| "rows must be a number")?;
            let cols = rest.get(2).ok_or("resize requires cols")?.parse::<u16>().map_err(|_| "cols must be a number")?;
            Request::ResizePane {
                pane: PaneId(parse_id(pane)?),
                rows,
                cols,
            }
        }
        "send" => {
            let (pane, positional) = take_pane_flag(rest);
            let data = positional.join(" ");
            if data.is_empty() {
                return Err("send requires text".into());
            }
            Request::Send { pane, data }
        }
        "send-key" => {
            let (pane, positional) = take_pane_flag(rest);
            let key = positional
                .first()
                .ok_or("send-key requires a key")?
                .clone();
            Request::SendKey { pane, key }
        }
        "focus" => {
            let t = rest.first().ok_or("focus requires a target")?;
            Request::Focus {
                target: parse_target(t)?,
            }
        }
        "focus-dir" => {
            let d = rest.first().ok_or("focus-dir requires a direction")?;
            Request::FocusDir {
                dir: parse_dir(d)?,
            }
        }
        "new-tab" => {
            let workspace = take_value_flag(rest, "--workspace")
                .map(|v| parse_id(&v).map(WorkspaceId))
                .transpose()?;
            Request::NewTab { workspace }
        }
        "new-workspace" => {
            let title = rest.first().cloned();
            Request::NewWorkspace { title }
        }
        "split" => {
            let (pane, positional) = take_pane_flag(rest);
            let orientation = match positional.first().map(String::as_str) {
                Some("horizontal") | Some("h") => SplitDir::Horizontal,
                Some("vertical") | Some("v") => SplitDir::Vertical,
                _ => return Err("split requires horizontal|vertical".into()),
            };
            Request::Split { pane, orientation }
        }
        "close-pane" => {
            let id = rest.first().ok_or("close-pane requires a pane id")?;
            Request::ClosePane {
                pane: PaneId(parse_id(id)?),
            }
        }
        "notify" => {
            let pane = rest.first().ok_or("notify requires a pane id")?;
            let title = rest.get(1).cloned().unwrap_or_default();
            let body = rest.get(2..).map(|s| s.join(" ")).unwrap_or_default();
            Request::Notify {
                pane: PaneId(parse_id(pane)?),
                title,
                body,
            }
        }
        "snapshot" => {
            let pane = rest.first().ok_or("snapshot requires a pane id")?;
            Request::Snapshot {
                pane: PaneId(parse_id(pane)?),
            }
        }
        "config" => {
            let sub = rest.first().map(String::as_str);
            match sub {
                Some("get") => Request::GetConfig {
                    path: rest.get(1).cloned(),
                },
                Some("set") => {
                    let path = rest.get(1).ok_or("config set requires a path")?.clone();
                    let value = rest.get(2).ok_or("config set requires a value")?.clone();
                    Request::SetConfig { path, value }
                }
                _ => return Err("config requires get|set".into()),
            }
        }
        other => return Err(format!("unknown command: {other}")),
    };
    Ok(Some(req))
}

fn take_pane_flag(args: &[String]) -> (Option<PaneId>, Vec<String>) {
    let mut pane = None;
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--pane" {
            if let Some(v) = args.get(i + 1) {
                if let Ok(n) = parse_id(v) {
                    pane = Some(PaneId(n));
                }
                i += 2;
                continue;
            }
        }
        positional.push(args[i].clone());
        i += 1;
    }
    (pane, positional)
}

fn take_value_flag(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned())
}

/// Parse `surface:3`, `tab:2`, `workspace:1`, or a bare number.
fn parse_id(s: &str) -> Result<u64, String> {
    let num = s.rsplit(':').next().unwrap_or(s);
    num.parse::<u64>().map_err(|_| format!("invalid id: {s}"))
}

fn parse_target(s: &str) -> Result<Target, String> {
    let id = parse_id(s)?;
    if s.starts_with("workspace:") {
        Ok(Target::Workspace(WorkspaceId(id)))
    } else if s.starts_with("tab:") {
        Ok(Target::Tab(TabId(id)))
    } else if s.starts_with("surface:") || s.starts_with("pane:") {
        Ok(Target::Pane(PaneId(id)))
    } else {
        // Bare number defaults to a pane (surface), the most common target.
        Ok(Target::Pane(PaneId(id)))
    }
}

fn parse_dir(s: &str) -> Result<Dir, String> {
    match s {
        "left" | "l" => Ok(Dir::Left),
        "right" | "r" => Ok(Dir::Right),
        "up" | "u" => Ok(Dir::Up),
        "down" | "d" => Ok(Dir::Down),
        other => Err(format!("invalid direction: {other}")),
    }
}

fn print_response(resp: &Response) {
    match resp {
        Response::Ok => println!("ok"),
        Response::Pong => println!("pong"),
        Response::Created { id } => println!("created {id}"),
        Response::Snapshot { text } => print!("{text}"),
        Response::ConfigValue { value } => {
            println!("{}", serde_json::to_string_pretty(value).unwrap_or_default())
        }
        Response::Error { message } => eprintln!("error: {message}"),
        Response::Workspaces { workspaces } => print_tree(workspaces),
        Response::Matches { matches } => {
            println!("{} match(es)", matches.len());
            for (line, col) in matches {
                println!("  line {line}, col {col}");
            }
        }
        Response::Notifications { notifications } => {
            if notifications.is_empty() {
                println!("(no notifications)");
            }
            for n in notifications {
                let mark = if n.read { " " } else { "•" };
                println!("{mark} {} {} — {}", n.pane, n.title, n.body);
            }
        }
    }
}

fn print_tree(workspaces: &[cmux_ipc::WorkspaceSummary]) {
    for w in workspaces {
        let marker = if w.active { "*" } else { " " };
        println!("{marker} {} {}", w.id, w.title);
        for t in &w.tabs {
            let tm = if t.active { ">" } else { " " };
            let bell = if t.attention { " ●" } else { "" };
            println!("  {tm} {} {}{}", t.id, t.title, bell);
            for p in &t.panes {
                let pm = if p.focused { "·" } else { " " };
                let ring = if p.ring == "idle" {
                    String::new()
                } else {
                    format!(" [{}]", p.ring)
                };
                println!("    {pm} {} {}{}", p.id, p.title, ring);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_args_prints_usage() {
        assert!(parse(&[]).unwrap().is_none());
        assert!(parse(&argv(&["--help"])).unwrap().is_none());
    }

    #[test]
    fn ping_and_list() {
        assert_eq!(parse(&argv(&["ping"])).unwrap(), Some(Request::Ping));
        assert_eq!(
            parse(&argv(&["list-workspaces"])).unwrap(),
            Some(Request::ListWorkspaces)
        );
    }

    #[test]
    fn send_joins_text_and_takes_pane_flag() {
        let r = parse(&argv(&["send", "--pane", "surface:4", "echo", "hi"])).unwrap();
        assert_eq!(
            r,
            Some(Request::Send {
                pane: Some(PaneId(4)),
                data: "echo hi".into()
            })
        );
    }

    #[test]
    fn send_without_pane_defaults_none() {
        let r = parse(&argv(&["send", "ls"])).unwrap();
        assert_eq!(
            r,
            Some(Request::Send {
                pane: None,
                data: "ls".into()
            })
        );
    }

    #[test]
    fn split_orientation_and_aliases() {
        assert_eq!(
            parse(&argv(&["split", "vertical"])).unwrap(),
            Some(Request::Split {
                pane: None,
                orientation: SplitDir::Vertical
            })
        );
        assert_eq!(
            parse(&argv(&["split", "h"])).unwrap(),
            Some(Request::Split {
                pane: None,
                orientation: SplitDir::Horizontal
            })
        );
        assert!(parse(&argv(&["split", "diagonal"])).is_err());
    }

    #[test]
    fn focus_target_parsing() {
        assert_eq!(
            parse(&argv(&["focus", "workspace:2"])).unwrap(),
            Some(Request::Focus {
                target: Target::Workspace(WorkspaceId(2))
            })
        );
        assert_eq!(
            parse(&argv(&["focus", "5"])).unwrap(),
            Some(Request::Focus {
                target: Target::Pane(PaneId(5))
            })
        );
    }

    #[test]
    fn focus_dir_parsing() {
        assert_eq!(
            parse(&argv(&["focus-dir", "left"])).unwrap(),
            Some(Request::FocusDir { dir: Dir::Left })
        );
        assert!(parse(&argv(&["focus-dir", "sideways"])).is_err());
    }

    #[test]
    fn config_get_set() {
        assert_eq!(
            parse(&argv(&["config", "get", "appearance.fontSize"])).unwrap(),
            Some(Request::GetConfig {
                path: Some("appearance.fontSize".into())
            })
        );
        assert_eq!(
            parse(&argv(&["config", "set", "appearance.theme", "dark"])).unwrap(),
            Some(Request::SetConfig {
                path: "appearance.theme".into(),
                value: "dark".into()
            })
        );
        assert!(parse(&argv(&["config"])).is_err());
    }

    #[test]
    fn notify_collects_body() {
        let r = parse(&argv(&["notify", "surface:3", "Claude", "needs", "input"])).unwrap();
        assert_eq!(
            r,
            Some(Request::Notify {
                pane: PaneId(3),
                title: "Claude".into(),
                body: "needs input".into()
            })
        );
    }

    #[test]
    fn socket_flag_is_extracted() {
        let (sock, rest) = extract_socket(argv(&["--socket", "/tmp/x.sock", "ping"]));
        assert_eq!(sock, Some(PathBuf::from("/tmp/x.sock")));
        assert_eq!(rest, argv(&["ping"]));
    }

    #[test]
    fn notifications_and_mark_read() {
        assert_eq!(
            parse(&argv(&["notifications"])).unwrap(),
            Some(Request::ListNotifications)
        );
        assert_eq!(
            parse(&argv(&["mark-read"])).unwrap(),
            Some(Request::MarkAllRead)
        );
    }

    #[test]
    fn rename_and_reorder() {
        assert_eq!(
            parse(&argv(&["rename-tab", "tab:3", "my", "build"])).unwrap(),
            Some(Request::RenameTab {
                tab: TabId(3),
                title: "my build".into()
            })
        );
        assert_eq!(
            parse(&argv(&["reorder-tab", "tab:3", "0"])).unwrap(),
            Some(Request::ReorderTab {
                tab: TabId(3),
                index: 0
            })
        );
        assert_eq!(
            parse(&argv(&["resize", "surface:2", "40", "120"])).unwrap(),
            Some(Request::ResizePane {
                pane: PaneId(2),
                rows: 40,
                cols: 120
            })
        );
        assert!(parse(&argv(&["rename-tab", "tab:3"])).is_err());
    }

    #[test]
    fn browser_and_navigate() {
        assert_eq!(
            parse(&argv(&["browser", "https://example.com"])).unwrap(),
            Some(Request::OpenBrowser {
                url: "https://example.com".into(),
                orientation: SplitDir::Horizontal
            })
        );
        assert_eq!(
            parse(&argv(&["navigate", "surface:4", "https://docs.rs"])).unwrap(),
            Some(Request::NavigateBrowser {
                pane: PaneId(4),
                url: "https://docs.rs".into()
            })
        );
        assert!(parse(&argv(&["browser"])).is_err());
    }

    #[test]
    fn find_command() {
        assert_eq!(
            parse(&argv(&["find", "surface:2", "error", "log"])).unwrap(),
            Some(Request::Find {
                pane: PaneId(2),
                query: "error log".into()
            })
        );
        assert!(parse(&argv(&["find", "surface:2"])).is_err());
    }

    #[test]
    fn unknown_command_errors() {
        assert!(parse(&argv(&["frobnicate"])).is_err());
    }
}
