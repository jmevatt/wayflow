//! Subprocess supervisor: spawns `wayflow server` / `wayflow client` as a
//! child of the current binary, tracks its state, and surfaces stderr lines
//! for status display.

use anyhow::{Context, Result};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Server,
    Client,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Server => "server",
            Mode::Client => "client",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Stopped,
    Running { mode: Mode, pid: u32 },
    Crashed { mode: Mode, code: Option<i32> },
}

impl State {
    pub fn is_running(&self) -> bool {
        matches!(self, State::Running { .. })
    }

    pub fn mode(&self) -> Option<Mode> {
        match self {
            State::Running { mode, .. } | State::Crashed { mode, .. } => Some(*mode),
            State::Stopped => None,
        }
    }
}

/// Shared, observable supervisor state. Cheap to clone (Arc).
#[derive(Clone)]
pub struct Supervisor {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    state: State,
    child: Option<Child>,
    last_lines: Vec<String>, // ring of recent stderr lines, for status tooltip
}

const LINE_RING: usize = 50;

impl Supervisor {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                state: State::Stopped,
                child: None,
                last_lines: Vec::new(),
            })),
        }
    }

    pub fn state(&self) -> State {
        self.inner.lock().unwrap().state.clone()
    }

    pub fn last_lines(&self) -> Vec<String> {
        self.inner.lock().unwrap().last_lines.clone()
    }

    /// Start the given mode. If something is already running, stops it first.
    pub fn start(&self, mode: Mode) -> Result<()> {
        self.stop();

        let exe = std::env::current_exe().context("locate current exe")?;
        let subcommand = mode.label();

        let mut cmd = Command::new(&exe);
        cmd.arg(subcommand);
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::piped());
        // Inherit env (so $WAYFLOW_LOG, $HOME etc. carry through).

        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn {} {}", exe.display(), subcommand))?;

        let pid = child.id();
        let stderr = child.stderr.take().expect("piped stderr");

        // stderr reader thread.
        let watcher = self.clone();
        thread::Builder::new()
            .name(format!("wayflow-tray-stderr-{subcommand}"))
            .spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    let mut g = watcher.inner.lock().unwrap();
                    if g.last_lines.len() >= LINE_RING {
                        g.last_lines.remove(0);
                    }
                    g.last_lines.push(line);
                }
            })
            .ok();

        let mut g = self.inner.lock().unwrap();
        g.state = State::Running { mode, pid };
        g.child = Some(child);
        Ok(())
    }

    /// Stop the running child (if any). Best-effort kill + wait; clears state.
    pub fn stop(&self) {
        let mut g = self.inner.lock().unwrap();
        if let Some(mut child) = g.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        g.state = State::Stopped;
    }

    /// Poll once: if the child has exited on its own, update state to Crashed
    /// (or Stopped on clean exit). Call this from the UI tick.
    pub fn poll(&self) {
        let mut g = self.inner.lock().unwrap();
        let Some(child) = g.child.as_mut() else { return };
        match child.try_wait() {
            Ok(Some(status)) => {
                let mode = g.state.mode().unwrap_or(Mode::Server);
                g.child = None;
                g.state = if status.success() {
                    State::Stopped
                } else {
                    State::Crashed { mode, code: status.code() }
                };
            }
            Ok(None) => {} // still running
            Err(_) => {}   // ignore transient errors
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
