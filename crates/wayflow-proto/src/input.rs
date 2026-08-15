//! The host-to-client input stream.
//!
//! Distinct from the probe types in [`crate::raw`]: those preserve whatever a platform
//! emitted for study, these are the normalized events one machine sends another.
//!
//! Keys travel as Linux evdev codes rather than as a neutral namespace. The capture side
//! already receives evdev codes from `wl_keyboard`, so choosing anything else would mean
//! translating twice and owning two tables that can disagree. When a non-Linux host is
//! added, it converts into this namespace at its own boundary.

use serde::{Deserialize, Serialize};

/// One message on the host-to-client link.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Msg {
    /// Sent once on connect, before any input.
    Hello {
        proto_version: u32,
        host: String,
    },
    /// Control passed to the client. The pointer should appear at the far edge, at the
    /// same fraction down the screen where it left the host, so the crossing looks
    /// continuous rather than teleporting to a corner.
    Enter {
        edge_ratio: f64,
    },
    /// Control returned to the host.
    Leave,
    Input(Input),
}

/// A single input action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "i", rename_all = "snake_case")]
pub enum Input {
    /// Unaccelerated relative motion. Deliberately not absolute coordinates: the host has
    /// no idea how large the client's desktop is, and the client is the only side that
    /// can clamp sensibly to its own displays.
    Motion { dx: f64, dy: f64 },
    /// evdev button code: `BTN_LEFT` is 272, `BTN_RIGHT` 273, `BTN_MIDDLE` 274.
    Button { code: u16, pressed: bool },
    /// Positive dy scrolls up. Values are in notches, fractional for trackpads.
    Scroll { dx: f64, dy: f64 },
    /// evdev key code. Autorepeat is deliberately absent: the client's own OS generates
    /// repeat from a held key, and forwarding the host's repeats too would double them.
    Key { code: u16, pressed: bool },
}

impl Msg {
    /// Serialize as one NDJSON line, newline included.
    ///
    /// # Errors
    /// Propagates any `serde_json` failure.
    pub fn to_line(&self) -> Result<String, serde_json::Error> {
        let mut s = serde_json::to_string(self)?;
        s.push('\n');
        Ok(s)
    }

    /// Parse one NDJSON line.
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

    #[test]
    fn input_messages_round_trip() {
        let cases = [
            Msg::Hello {
                proto_version: 1,
                host: "terminus".into(),
            },
            Msg::Enter { edge_ratio: 0.25 },
            Msg::Leave,
            Msg::Input(Input::Motion { dx: -3.5, dy: 0.0 }),
            Msg::Input(Input::Button {
                code: 272,
                pressed: true,
            }),
            Msg::Input(Input::Scroll { dx: 0.0, dy: -1.0 }),
            Msg::Input(Input::Key {
                code: 30,
                pressed: true,
            }),
        ];
        for msg in cases {
            let line = msg.to_line().unwrap();
            assert_eq!(line.matches('\n').count(), 1);
            assert_eq!(Msg::from_line(&line).unwrap(), msg, "round trip: {line}");
        }
    }

    #[test]
    fn negative_and_fractional_motion_survives() {
        // Trackpad deltas are fractional and frequently negative; an integer wire type
        // would quietly floor them and the cursor would drift short over time.
        let msg = Msg::Input(Input::Motion {
            dx: -0.5,
            dy: 1234.75,
        });
        assert_eq!(Msg::from_line(&msg.to_line().unwrap()).unwrap(), msg);
    }

    #[test]
    fn unknown_message_type_is_rejected() {
        assert!(Msg::from_line(r#"{"t":"shutdown"}"#).is_err());
    }
}
