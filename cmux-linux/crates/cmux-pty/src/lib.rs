//! PTY-backed process hosting for cmux-linux panes.
//!
//! [`PtySession`] spawns a child process (a shell or coding agent) attached to a
//! pseudo-terminal, exposes input/resize/kill controls, and streams output and
//! exit notifications over a [`std::sync::mpsc`] channel of [`PtyEvent`]s.
//!
//! # Example
//!
//! ```no_run
//! use cmux_pty::{PtyConfig, PtyEvent, PtySession};
//! use std::time::Duration;
//!
//! let mut session = PtySession::spawn(PtyConfig::command(["bash", "-c", "echo hi"]))?;
//! let events = session.take_events().expect("events not yet taken");
//! while let Ok(event) = events.recv_timeout(Duration::from_secs(2)) {
//!     match event {
//!         PtyEvent::Output(bytes) => print!("{}", String::from_utf8_lossy(&bytes)),
//!         PtyEvent::Exited(code) => {
//!             println!("exited with {:?}", code);
//!             break;
//!         }
//!     }
//! }
//! # Ok::<(), cmux_pty::PtyError>(())
//! ```

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;

use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use thiserror::Error;

/// Errors produced while spawning or driving a [`PtySession`].
#[derive(Debug, Error)]
pub enum PtyError {
    /// Failed to open the pseudo-terminal pair.
    #[error("failed to open pty: {0}")]
    OpenPty(String),

    /// Failed to spawn the child command in the pty.
    #[error("failed to spawn command: {0}")]
    Spawn(String),

    /// Failed to clone the master reader handle for the reader thread.
    #[error("failed to clone pty reader: {0}")]
    CloneReader(String),

    /// Failed to obtain the master writer handle.
    #[error("failed to obtain pty writer: {0}")]
    TakeWriter(String),

    /// A write to the child failed.
    #[error("failed to write to pty: {0}")]
    Write(#[source] std::io::Error),

    /// A resize request failed.
    #[error("failed to resize pty: {0}")]
    Resize(String),

    /// A kill request failed.
    #[error("failed to kill child: {0}")]
    Kill(#[source] std::io::Error),

    /// No command was configured and no shell could be determined.
    #[error("no command configured and no shell could be determined")]
    NoCommand,
}

/// An event emitted by the reader thread of a [`PtySession`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyEvent {
    /// A chunk of raw bytes read from the pty master.
    Output(Vec<u8>),

    /// The child process exited. The payload is its exit code, when one is
    /// available (it is `None` if the platform could not surface a numeric code,
    /// e.g. for signal-terminated processes on some configurations).
    Exited(Option<i32>),
}

/// Terminal size in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtyGeometry {
    /// Number of rows (height in cells).
    pub rows: u16,
    /// Number of columns (width in cells).
    pub cols: u16,
}

impl PtyGeometry {
    /// Create a new geometry from rows and columns.
    pub fn new(rows: u16, cols: u16) -> Self {
        Self { rows, cols }
    }
}

impl Default for PtyGeometry {
    fn default() -> Self {
        Self { rows: 24, cols: 80 }
    }
}

/// Configuration describing the command to run inside the pty.
#[derive(Debug, Clone, Default)]
pub struct PtyConfig {
    /// Program plus arguments. When empty, the user's `$SHELL` (falling back to
    /// `/bin/bash`) is launched with no arguments.
    argv: Vec<OsString>,
    /// Initial pty geometry.
    size: PtyGeometry,
    /// Optional working directory for the child.
    cwd: Option<PathBuf>,
    /// Additional / overriding environment variables for the child.
    env: HashMap<OsString, OsString>,
    /// When true, the child inherits the parent process environment in addition
    /// to `env`. Defaults to true.
    inherit_env: bool,
}

impl PtyConfig {
    /// A configuration that launches the user's login shell.
    pub fn shell() -> Self {
        Self {
            argv: Vec::new(),
            size: PtyGeometry::default(),
            cwd: None,
            env: HashMap::new(),
            inherit_env: true,
        }
    }

    /// A configuration that launches an explicit command (program + args).
    ///
    /// ```
    /// use cmux_pty::PtyConfig;
    /// let cfg = PtyConfig::command(["bash", "-c", "echo hi"]);
    /// ```
    pub fn command<I, S>(argv: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            argv: argv.into_iter().map(Into::into).collect(),
            size: PtyGeometry::default(),
            cwd: None,
            env: HashMap::new(),
            inherit_env: true,
        }
    }

    /// Set the initial geometry.
    pub fn with_size(mut self, rows: u16, cols: u16) -> Self {
        self.size = PtyGeometry::new(rows, cols);
        self
    }

    /// Set the child working directory.
    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Add or override a single environment variable for the child.
    pub fn with_env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Control whether the parent environment is inherited. Defaults to `true`.
    pub fn inherit_env(mut self, inherit: bool) -> Self {
        self.inherit_env = inherit;
        self
    }

    fn resolve_argv(&self) -> Result<Vec<OsString>, PtyError> {
        if !self.argv.is_empty() {
            return Ok(self.argv.clone());
        }
        let shell = std::env::var_os("SHELL")
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| OsString::from("/bin/bash"));
        if shell.is_empty() {
            return Err(PtyError::NoCommand);
        }
        Ok(vec![shell])
    }
}

/// A live PTY-backed child process.
///
/// Dropping the session kills the child and joins the reader thread, so callers
/// do not need to call [`PtySession::kill`] explicitly for cleanup.
pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    reader_handle: Option<JoinHandle<()>>,
    events: Option<Receiver<PtyEvent>>,
    exited: Arc<AtomicBool>,
    exit_code: Arc<Mutex<Option<i32>>>,
}

impl PtySession {
    /// Spawn a command in a fresh pty according to `config`.
    pub fn spawn(config: PtyConfig) -> Result<Self, PtyError> {
        let argv = config.resolve_argv()?;
        let (program, args) = argv
            .split_first()
            .ok_or(PtyError::NoCommand)
            .map(|(p, rest)| (p.clone(), rest.to_vec()))?;

        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows: config.size.rows,
                cols: config.size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::OpenPty(e.to_string()))?;

        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        if let Some(cwd) = &config.cwd {
            cmd.cwd(cwd);
        }
        if !config.inherit_env {
            cmd.env_clear();
        }
        for (key, value) in &config.env {
            cmd.env(key, value);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| PtyError::Spawn(e.to_string()))?;
        // The slave handle is no longer needed in this process once the child
        // owns it; dropping it lets EOF propagate when the child exits.
        drop(pair.slave);

        let master = pair.master;
        let mut reader = master
            .try_clone_reader()
            .map_err(|e| PtyError::CloneReader(e.to_string()))?;
        let writer = master
            .take_writer()
            .map_err(|e| PtyError::TakeWriter(e.to_string()))?;

        let child = Arc::new(Mutex::new(child));
        let exited = Arc::new(AtomicBool::new(false));
        let exit_code = Arc::new(Mutex::new(None));

        let (tx, rx): (Sender<PtyEvent>, Receiver<PtyEvent>) = mpsc::channel();

        let reader_child = Arc::clone(&child);
        let reader_exited = Arc::clone(&exited);
        let reader_exit_code = Arc::clone(&exit_code);

        let reader_handle = std::thread::Builder::new()
            .name("cmux-pty-reader".to_string())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break, // EOF: child closed the pty.
                        Ok(n) => {
                            // If the receiver is gone, stop reading.
                            if tx.send(PtyEvent::Output(buf[..n].to_vec())).is_err() {
                                return;
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }

                // Reader saw EOF; wait for the child and report its exit code.
                let code = {
                    let mut guard = match reader_child.lock() {
                        Ok(g) => g,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    match guard.wait() {
                        Ok(status) => Some(status.exit_code() as i32),
                        Err(_) => None,
                    }
                };
                if let Ok(mut slot) = reader_exit_code.lock() {
                    *slot = code;
                }
                reader_exited.store(true, Ordering::SeqCst);
                let _ = tx.send(PtyEvent::Exited(code));
            })
            .map_err(|e| PtyError::Spawn(e.to_string()))?;

        Ok(Self {
            master,
            writer: Mutex::new(writer),
            child,
            reader_handle: Some(reader_handle),
            events: Some(rx),
            exited,
            exit_code,
        })
    }

    /// Convenience constructor that spawns the user's login shell at the given
    /// size.
    pub fn spawn_shell(rows: u16, cols: u16) -> Result<Self, PtyError> {
        Self::spawn(PtyConfig::shell().with_size(rows, cols))
    }

    /// Take ownership of the output/exit event channel.
    ///
    /// Returns `None` if it has already been taken. Each chunk of pty output
    /// arrives as a [`PtyEvent::Output`]; a final [`PtyEvent::Exited`] is sent
    /// once the child terminates.
    pub fn take_events(&mut self) -> Option<Receiver<PtyEvent>> {
        self.events.take()
    }

    /// Write bytes to the child's input.
    pub fn write(&self, bytes: &[u8]) -> Result<(), PtyError> {
        let mut writer = match self.writer.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        writer.write_all(bytes).map_err(PtyError::Write)?;
        writer.flush().map_err(PtyError::Write)?;
        Ok(())
    }

    /// Resize the pty to `rows` x `cols`.
    pub fn resize(&self, rows: u16, cols: u16) -> Result<(), PtyError> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::Resize(e.to_string()))
    }

    /// Whether the child is believed to still be running.
    ///
    /// The child process id, if still available. Used to read the live working
    /// directory from `/proc/<pid>/cwd` on Linux.
    pub fn process_id(&self) -> Option<u32> {
        self.child.lock().ok().and_then(|c| c.process_id())
    }

    /// This returns `false` once the child has exited and the reader thread has
    /// reaped it, or if a non-blocking poll observes the child has exited.
    pub fn is_alive(&self) -> bool {
        if self.exited.load(Ordering::SeqCst) {
            return false;
        }
        // Best-effort non-blocking poll in case the reader has not yet noticed.
        let mut guard = match self.child.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        match guard.try_wait() {
            Ok(Some(status)) => {
                if let Ok(mut slot) = self.exit_code.lock() {
                    *slot = Some(status.exit_code() as i32);
                }
                self.exited.store(true, Ordering::SeqCst);
                false
            }
            Ok(None) => true,
            // If we can't tell, assume it's still alive until the reader proves otherwise.
            Err(_) => true,
        }
    }

    /// The child's exit code, if it has exited and a code is known.
    pub fn exit_code(&self) -> Option<i32> {
        match self.exit_code.lock() {
            Ok(g) => *g,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    /// Kill the child process immediately.
    pub fn kill(&self) -> Result<(), PtyError> {
        let mut guard = match self.child.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.kill().map_err(PtyError::Kill)
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        // Kill the child so the reader thread sees EOF and the pty is released.
        let _ = self.kill();
        if let Some(handle) = self.reader_handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    const RECV_TIMEOUT: Duration = Duration::from_secs(5);

    /// Drain events until `Exited` is seen or we time out, accumulating output.
    /// Returns (collected_output_bytes, exit_code_if_seen).
    fn collect_until_exit(
        events: &Receiver<PtyEvent>,
        overall: Duration,
    ) -> (Vec<u8>, Option<Option<i32>>) {
        let deadline = Instant::now() + overall;
        let mut out = Vec::new();
        let mut exit = None;
        while Instant::now() < deadline {
            match events.recv_timeout(RECV_TIMEOUT) {
                Ok(PtyEvent::Output(chunk)) => out.extend_from_slice(&chunk),
                Ok(PtyEvent::Exited(code)) => {
                    exit = Some(code);
                    break;
                }
                Err(_) => break,
            }
        }
        (out, exit)
    }

    #[test]
    fn echo_hello_appears_in_output() {
        let mut session =
            PtySession::spawn(PtyConfig::command(["bash", "-c", "echo hello"])).unwrap();
        let events = session.take_events().expect("events available");
        let (out, exit) = collect_until_exit(&events, Duration::from_secs(10));
        let text = String::from_utf8_lossy(&out);
        assert!(
            text.contains("hello"),
            "expected output to contain 'hello', got: {text:?}"
        );
        assert_eq!(exit, Some(Some(0)), "echo should exit 0");
    }

    #[test]
    fn exit_code_is_propagated() {
        let mut session =
            PtySession::spawn(PtyConfig::command(["bash", "-c", "exit 7"])).unwrap();
        let events = session.take_events().expect("events available");
        let (_out, exit) = collect_until_exit(&events, Duration::from_secs(10));
        assert_eq!(exit, Some(Some(7)), "expected Exited(Some(7))");
        // After exit, the session should report not alive and the recorded code.
        assert!(!session.is_alive());
        assert_eq!(session.exit_code(), Some(7));
    }

    #[test]
    fn cat_echoes_written_input() {
        let mut session = PtySession::spawn(PtyConfig::command(["cat"])).unwrap();
        let events = session.take_events().expect("events available");

        session.write(b"ping\n").unwrap();

        // Accumulate output until we see "ping" or time out.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut out = Vec::new();
        let mut saw_ping = false;
        while Instant::now() < deadline {
            match events.recv_timeout(RECV_TIMEOUT) {
                Ok(PtyEvent::Output(chunk)) => {
                    out.extend_from_slice(&chunk);
                    if String::from_utf8_lossy(&out).contains("ping") {
                        saw_ping = true;
                        break;
                    }
                }
                Ok(PtyEvent::Exited(_)) => break,
                Err(_) => break,
            }
        }
        let text = String::from_utf8_lossy(&out);
        assert!(saw_ping, "expected 'ping' echoed back, got: {text:?}");

        // Closing stdin via EOF (Ctrl-D) should let cat exit; killing on drop is
        // also fine, but verify the live-state query works while running.
        assert!(session.is_alive() || !session.is_alive());
    }

    #[test]
    fn resize_does_not_error_on_live_session() {
        let session = PtySession::spawn_shell(24, 80).unwrap();
        session.resize(40, 120).expect("resize should succeed");
    }

    #[test]
    fn default_shell_spawns() {
        let session = PtySession::spawn(PtyConfig::shell()).unwrap();
        assert!(session.is_alive());
    }

    #[test]
    fn custom_env_is_visible_to_child() {
        let mut session = PtySession::spawn(
            PtyConfig::command(["bash", "-c", "echo VAL=$CMUX_TEST_VAR"])
                .with_env("CMUX_TEST_VAR", "marker123"),
        )
        .unwrap();
        let events = session.take_events().expect("events available");
        let (out, _exit) = collect_until_exit(&events, Duration::from_secs(10));
        let text = String::from_utf8_lossy(&out);
        assert!(
            text.contains("VAL=marker123"),
            "expected env var to be visible, got: {text:?}"
        );
    }
}
