//! Wire types for the wayflow probe harness.
//!
//! This is Phase 0 instrumentation, not the wayflow protocol. Its job is to answer what
//! three operating systems actually emit when a key is pressed, so the real protocol can
//! be designed against measurements instead of assumptions. Expect to throw it away.
//!
//! Framing is newline-delimited JSON: verbose, but greppable and diffable by hand, which
//! is worth more than bytes on a LAN while we are still learning the shape of the data.

pub mod input;
pub mod raw;

pub use input::{Input, Msg};
pub use raw::{LinuxEvent, MacEvent, Platform, RawPayload, WindowsEvent};

use serde::{Deserialize, Serialize};

/// Bumped on any breaking change to this module. The probe refuses a mismatched peer
/// rather than silently misreading fields, because a stale probe on a remote box that
/// half-works is far more expensive to debug than one that will not start.
pub const PROTO_VERSION: u32 = 1;

/// One newline-delimited message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Frame {
    /// First frame on every connection.
    Hello(Hello),
    /// Captured input, probe to lab.
    Event(Event),
    /// Replay request, lab to probe.
    Inject(Inject),
    /// The probe rejected something. Carries a reason instead of just closing, so a
    /// version mismatch is not indistinguishable from a dropped network link.
    Error { message: String },
}

/// Probe self-description, sent once on connect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub proto_version: u32,
    pub probe_version: String,
    pub platform: Platform,
    pub hostname: String,
    /// Wall-clock reading of the monotonic zero point, in Unix nanoseconds. Timestamps
    /// on the wire stay monotonic, but traces from different machines have to be laid on
    /// a shared axis somehow, and this is the offset that permits it. It is deliberately
    /// not a synchronization mechanism: clock skew across the LAN is one of the things
    /// Phase 0 is meant to measure, not something to paper over.
    pub mono_epoch_unix_ns: u64,
    /// Display rectangles as `[x, y, width, height]` in the platform's own global
    /// coordinate space, whatever its origin and orientation turn out to be.
    pub displays: Vec<[f64; 4]>,
}

/// A captured event with probe-local ordering and timing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Strictly increasing from 0. Gaps mean the probe dropped events under load, which
    /// is itself a measurement worth keeping rather than hiding.
    pub seq: u64,
    /// Nanoseconds since probe start, from a monotonic source.
    ///
    /// Kept separate from the platform's own timestamp inside the payload: those use
    /// incomparable units and epochs per OS, and at least one of them is known to be
    /// coarse. Recording both lets us find out how coarse.
    pub t_mono_ns: u64,
    pub payload: RawPayload,
}

/// Replay an event on the probe's machine.
///
/// Phase 0 injection is verbatim replay rather than a normalized command, on purpose: it
/// isolates whether the injection path works at all from whether our normalization is
/// correct. Feed back an event the same box just produced, and any difference is the
/// platform's, not ours.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Inject {
    /// Echoed back on the resulting capture where the platform permits tagging
    /// synthesized input, so the lab can pair injections with what they produced and
    /// measure the round trip.
    pub token: u64,
    pub payload: RawPayload,
}

impl Frame {
    /// Serialize as one NDJSON line, newline included.
    ///
    /// # Errors
    /// Propagates any `serde_json` failure.
    pub fn to_line(&self) -> Result<String, serde_json::Error> {
        let mut s = serde_json::to_string(self)?;
        s.push('\n');
        Ok(s)
    }

    /// Parse one NDJSON line. Surrounding whitespace is tolerated.
    ///
    /// # Errors
    /// Propagates any `serde_json` failure.
    pub fn from_line(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event() -> Frame {
        Frame::Event(Event {
            seq: 7,
            t_mono_ns: 1_234_567_890,
            payload: RawPayload::Linux(LinuxEvent {
                device: "/dev/input/event3".into(),
                ev_type: 1,
                code: 30,
                value: 1,
                ev_time_us: 999,
            }),
        })
    }

    #[test]
    fn line_form_is_single_line_and_round_trips() {
        let line = sample_event().to_line().unwrap();
        assert!(line.ends_with('\n'));
        assert_eq!(
            line.matches('\n').count(),
            1,
            "NDJSON framing needs exactly one terminator"
        );
        assert_eq!(Frame::from_line(&line).unwrap(), sample_event());
    }

    #[test]
    fn embedded_newlines_cannot_break_framing() {
        // A macOS unicode payload is attacker-adjacent free text: pressing Return makes
        // the OS hand us "\r". If that reached the wire literally it would split one
        // frame into two unparseable halves.
        let frame = Frame::Event(Event {
            seq: 0,
            t_mono_ns: 0,
            payload: RawPayload::MacOs(MacEvent {
                event_type: 10,
                cg_timestamp: 0,
                flags: 0,
                keycode: Some(36),
                autorepeat: Some(false),
                unicode: Some("\r\n".into()),
                location: None,
                mouse_delta: None,
                button_number: None,
                click_state: None,
                scroll_lines: None,
                scroll_pixels: None,
                scroll_continuous: None,
            }),
        });
        let line = frame.to_line().unwrap();
        assert_eq!(
            line.matches('\n').count(),
            1,
            "raw newline leaked into framing: {line}"
        );
        assert_eq!(Frame::from_line(&line).unwrap(), frame);
    }

    #[test]
    fn unknown_kind_is_an_error_not_a_silent_drop() {
        let err = Frame::from_line(r#"{"kind":"telemetry","payload":{}}"#);
        assert!(err.is_err(), "unrecognized frames must fail loudly");
    }
}
