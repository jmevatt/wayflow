// Runtime telemetry counters + a route-state snapshot for SIGUSR1 dumps.
//
// Read live by the signal handler in `server::run` to produce a JSON dump
// at /tmp/wayflow-server-state.json without disrupting the routing path.

use std::sync::atomic::{AtomicU64, Ordering};
use serde::Serialize;
use wayflow_proto::ScreenInfo;

#[derive(Default)]
pub struct Telemetry {
    /// InputEvents dropped because route_events couldn't drain the channel.
    /// High value = TLS write to client is back-pressuring the routing loop.
    pub input_events_dropped_full: AtomicU64,
    /// InputEvents dropped because the InputEvent receiver task is gone.
    /// Should only happen during shutdown.
    pub input_events_dropped_closed: AtomicU64,
    /// S2C send().await calls that took longer than SLOW_S2C_THRESHOLD.
    /// High value = same as above; sustained pressure on the per-client
    /// write channel.
    pub s2c_slow_sends: AtomicU64,
    /// Number of SIGUSR1 dumps successfully emitted.
    pub dumps_emitted: AtomicU64,
}

#[derive(Serialize)]
pub struct TelemetryView {
    pub input_events_dropped_full: u64,
    pub input_events_dropped_closed: u64,
    pub s2c_slow_sends: u64,
    pub dumps_emitted: u64,
}

impl Telemetry {
    pub fn snapshot(&self) -> TelemetryView {
        TelemetryView {
            input_events_dropped_full:   self.input_events_dropped_full.load(Ordering::Relaxed),
            input_events_dropped_closed: self.input_events_dropped_closed.load(Ordering::Relaxed),
            s2c_slow_sends:              self.s2c_slow_sends.load(Ordering::Relaxed),
            dumps_emitted:               self.dumps_emitted.load(Ordering::Relaxed),
        }
    }
}

/// A snapshot of `route_events`'s internal state. Published via watch channel
/// after every state-changing event so the signal handler can read it
/// lock-free.
#[derive(Default, Clone, Serialize)]
pub struct RouteSnapshot {
    pub active_client: Option<String>,
    pub active_edge: Option<String>,
    pub server_cursor: (i32, i32),
    pub client_cursor: (i32, i32),
    /// HID keycodes formatted as hex strings (e.g. "0xE2").
    pub held_keys: Vec<String>,
    pub monitor_count: usize,
    pub server_screens: Vec<ScreenInfo>,
}
