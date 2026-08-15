//! Edge capture on wlroots compositors.
//!
//! The XDG `InputCapture` portal is the interface designed for this, and sway does not
//! expose it, so we assemble the same behaviour from wlroots protocols directly.
//!
//! Idle: a one-pixel layer surface sits on the configured screen edge. It is transparent
//! and accepts no keyboard focus, so it is invisible to use until the pointer touches it.
//!
//! Active (not yet built): a full-screen overlay takes exclusive keyboard focus, the
//! pointer is locked so the local cursor stops moving, and relative motion is forwarded.

mod capture;
mod dispatch;
mod input;
mod state;

pub use state::{Edge, State};

pub const DEFAULT_PORT: u16 = 47821;

use std::fmt;

use wayland_client::Connection;

/// Which screen edge hands control to the remote machine.
///
/// One edge, not four: this is the boundary the pointer crosses to leave, and the machine
/// it leads to is a property of that edge.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub edge: Edge,
    pub port: u16,
}

impl Default for Config {
    fn default() -> Self {
        // The Mac lives to the left of the ultrawide.
        Self {
            edge: Edge::Left,
            port: DEFAULT_PORT,
        }
    }
}

/// Run the capture loop until the connection drops.
///
/// # Errors
/// Fails if there is no compositor, or if a protocol the design depends on is absent.
pub fn run(config: Config) -> Result<(), Error> {
    let conn = Connection::connect_to_env().map_err(|e| Error::Connect(e.to_string()))?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = State::new(config, config.port);
    // First roundtrip binds globals. Outputs arrive as globals but their geometry comes in
    // follow-up events, so nothing can be placed yet.
    queue
        .roundtrip(&mut state)
        .map_err(|e| Error::Roundtrip(e.to_string()))?;
    state.check_globals()?;
    // Second roundtrip drains the per-output geometry the first one only announced.
    queue
        .roundtrip(&mut state)
        .map_err(|e| Error::Roundtrip(e.to_string()))?;

    state.place_edge_strip(&qh);
    if !state.strip_placed() {
        return Err(Error::NoOutput(config.edge));
    }

    loop {
        queue
            .blocking_dispatch(&mut state)
            .map_err(|e| Error::Roundtrip(e.to_string()))?;
    }
}

#[derive(Debug)]
pub enum Error {
    Connect(String),
    Roundtrip(String),
    MissingGlobal(&'static str),
    NoOutput(Edge),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(e) => write!(
                f,
                "cannot reach a Wayland compositor ({e}); \
                 WAYLAND_DISPLAY must point at a live session"
            ),
            Self::Roundtrip(e) => write!(f, "wayland protocol error: {e}"),
            Self::MissingGlobal(name) => {
                write!(f, "compositor does not offer {name}; run `wayflow-host check`")
            }
            Self::NoOutput(edge) => write!(f, "no output found on the {edge:?} edge"),
        }
    }
}

impl std::error::Error for Error {}
