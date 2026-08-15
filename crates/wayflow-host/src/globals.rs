//! Compositor capability check.
//!
//! Every protocol below is load-bearing for edge capture, and a compositor missing one
//! fails in a way that looks like a bug in our code rather than an absent feature: no
//! pointer lock means the cursor moves on both machines, no shortcuts inhibitor means
//! Super-anything vanishes into sway. Enumerating up front turns all of those into one
//! legible message.

use std::fmt;

use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle};

/// A protocol we need, and what breaks without it.
pub struct Requirement {
    pub interface: &'static str,
    pub purpose: &'static str,
    /// False for protocols we can degrade without, at a cost worth naming.
    pub required: bool,
}

pub const REQUIREMENTS: &[Requirement] = &[
    Requirement {
        interface: "wl_compositor",
        purpose: "create surfaces",
        required: true,
    },
    Requirement {
        interface: "wl_seat",
        purpose: "pointer and keyboard input",
        required: true,
    },
    Requirement {
        interface: "wl_output",
        purpose: "per-monitor geometry for edge placement",
        required: true,
    },
    Requirement {
        interface: "zwlr_layer_shell_v1",
        purpose: "edge strips and the capture overlay",
        required: true,
    },
    Requirement {
        interface: "zwp_relative_pointer_manager_v1",
        purpose: "motion deltas that keep flowing past the screen edge",
        required: true,
    },
    Requirement {
        interface: "zwp_pointer_constraints_v1",
        purpose: "pin the local cursor while captured",
        required: true,
    },
    Requirement {
        interface: "zwp_keyboard_shortcuts_inhibit_manager_v1",
        purpose: "forward Super and Alt-Tab instead of losing them to sway",
        required: true,
    },
    Requirement {
        interface: "zwlr_virtual_pointer_manager_v1",
        purpose: "nudge the cursor off the edge when capture ends",
        required: false,
    },
    Requirement {
        interface: "wp_viewporter",
        purpose: "size surfaces without allocating real buffers",
        required: false,
    },
    Requirement {
        interface: "wp_single_pixel_buffer_manager_v1",
        purpose: "invisible surfaces without a shm pool",
        required: false,
    },
];

/// What the compositor advertised.
#[derive(Default)]
pub struct Advertised {
    pub globals: Vec<(String, u32)>,
}

impl Advertised {
    #[must_use]
    pub fn version_of(&self, interface: &str) -> Option<u32> {
        self.globals
            .iter()
            .find(|(name, _)| name == interface)
            .map(|(_, v)| *v)
    }

    /// Required interfaces the compositor did not advertise.
    #[must_use]
    pub fn missing_required(&self) -> Vec<&'static Requirement> {
        REQUIREMENTS
            .iter()
            .filter(|r| r.required && self.version_of(r.interface).is_none())
            .collect()
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for Advertised {
    fn event(
        state: &mut Self,
        _registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            interface, version, ..
        } = event
        {
            state.globals.push((interface, version));
        }
    }
}

/// Connect and complete one registry roundtrip.
///
/// # Errors
/// Fails when there is no compositor to talk to, which on a headless or SSH session is
/// the expected outcome rather than a fault.
pub fn probe() -> Result<Advertised, ProbeError> {
    let conn = Connection::connect_to_env().map_err(|e| ProbeError::Connect(e.to_string()))?;
    let display = conn.display();
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    display.get_registry(&qh, ());

    let mut state = Advertised::default();
    // A single roundtrip is enough: the compositor sends the full global list before it
    // answers the sync request.
    queue
        .roundtrip(&mut state)
        .map_err(|e| ProbeError::Roundtrip(e.to_string()))?;
    state.globals.sort();
    Ok(state)
}

#[derive(Debug)]
pub enum ProbeError {
    Connect(String),
    Roundtrip(String),
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(e) => write!(
                f,
                "cannot reach a Wayland compositor ({e}). \
                 WAYLAND_DISPLAY and XDG_RUNTIME_DIR must point at a live session; \
                 an SSH shell without them will always land here."
            ),
            Self::Roundtrip(e) => write!(f, "registry roundtrip failed: {e}"),
        }
    }
}

impl std::error::Error for ProbeError {}
