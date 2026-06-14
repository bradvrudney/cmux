//! Unix-domain-socket transport for the control protocol.
//!
//! Framing is newline-delimited JSON: each request is one line, each response
//! is one line. The [`Server`] runs an accept loop and dispatches every request
//! through a caller-supplied handler (the GUI's, which locks shared state); the
//! [`Client`] is what the `cmux` CLI uses.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use crate::protocol::{Request, Response};

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol error: {0}")]
    Protocol(#[from] serde_json::Error),
    #[error("server closed the connection without responding")]
    NoResponse,
    #[error("could not determine runtime directory for socket")]
    NoRuntimeDir,
}

/// Default control-socket path: `$XDG_RUNTIME_DIR/cmux/control.sock`, falling
/// back to `/tmp/cmux-$UID/control.sock`.
pub fn default_socket_path() -> Result<PathBuf, IpcError> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            let uid = unsafe { libc_getuid() };
            PathBuf::from(format!("/tmp/cmux-{uid}"))
        });
    Ok(base.join("cmux").join("control.sock"))
}

// Avoid pulling the whole `libc` crate for a single call.
extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}

/// A bound control-socket server.
pub struct Server {
    listener: UnixListener,
    path: PathBuf,
}

impl Server {
    /// Bind to `path`, creating parent dirs and clearing a stale socket file.
    pub fn bind(path: &Path) -> Result<Server, IpcError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Clear a leftover socket file from a previous run. If a live server is
        // already listening, `bind` below will still succeed only after removal;
        // callers that care about single-instance should ping first.
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
        let listener = UnixListener::bind(path)?;
        Ok(Server {
            listener,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run the accept loop, dispatching each request through `handler`.
    /// Connections are served sequentially; this blocks forever (run it on a
    /// dedicated thread). Per-connection I/O errors are swallowed so one bad
    /// client cannot take the server down.
    pub fn run<F>(self, mut handler: F)
    where
        F: FnMut(Request) -> Response,
    {
        for stream in self.listener.incoming() {
            let Ok(stream) = stream else { continue };
            if let Err(_e) = Self::handle_conn(stream, &mut handler) {
                continue;
            }
        }
    }

    fn handle_conn<F>(stream: UnixStream, handler: &mut F) -> Result<(), IpcError>
    where
        F: FnMut(Request) -> Response,
    {
        let mut writer = stream.try_clone()?;
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let response = match serde_json::from_str::<Request>(&line) {
                Ok(req) => handler(req),
                Err(e) => Response::error(format!("bad request: {e}")),
            };
            let mut out = serde_json::to_string(&response)?;
            out.push('\n');
            writer.write_all(out.as_bytes())?;
            writer.flush()?;
        }
        Ok(())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A control-socket client (one request/response per call).
pub struct Client {
    path: PathBuf,
}

impl Client {
    pub fn new(path: &Path) -> Self {
        Client {
            path: path.to_path_buf(),
        }
    }

    /// Connect, send `req`, and read exactly one response line.
    pub fn send(&self, req: &Request) -> Result<Response, IpcError> {
        let stream = UnixStream::connect(&self.path)?;
        let mut writer = stream.try_clone()?;
        let mut line = serde_json::to_string(req)?;
        line.push('\n');
        writer.write_all(line.as_bytes())?;
        writer.flush()?;

        let mut reader = BufReader::new(stream);
        let mut resp = String::new();
        if reader.read_line(&mut resp)? == 0 {
            return Err(IpcError::NoResponse);
        }
        Ok(serde_json::from_str(&resp)?)
    }

    pub fn is_reachable(&self) -> bool {
        UnixStream::connect(&self.path).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn temp_socket() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cmux-ipc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{}.sock", rand_suffix()))
    }

    fn rand_suffix() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64
    }

    #[test]
    fn ping_pong_over_socket() {
        let path = temp_socket();
        let server = Server::bind(&path).unwrap();
        let handle = thread::spawn(move || {
            server.run(|req| match req {
                Request::Ping => Response::Pong,
                _ => Response::error("unexpected"),
            });
        });

        // Give the accept loop a moment, then talk to it.
        let client = Client::new(&path);
        let mut attempts = 0;
        while !client.is_reachable() && attempts < 100 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            attempts += 1;
        }
        let resp = client.send(&Request::Ping).unwrap();
        assert_eq!(resp, Response::Pong);

        // Closing the listener ends the loop once the process exits; detach.
        drop(handle);
    }

    #[test]
    fn unknown_command_still_gets_a_response() {
        let path = temp_socket();
        let server = Server::bind(&path).unwrap();
        thread::spawn(move || {
            server.run(|_req| Response::error("nope"));
        });
        let client = Client::new(&path);
        let mut attempts = 0;
        while !client.is_reachable() && attempts < 100 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            attempts += 1;
        }
        let resp = client.send(&Request::ListWorkspaces).unwrap();
        assert!(matches!(resp, Response::Error { .. }));
    }
}
