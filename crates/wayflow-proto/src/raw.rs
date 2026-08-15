//! Verbatim platform input payloads.
//!
//! Every field here mirrors what the OS handed us, with no interpretation. The probe's
//! entire job is to move these bytes; deciding what they *mean* happens off-box in the
//! lab, where a wrong guess costs a rebuild instead of a trip to the endpoint.

use serde::{Deserialize, Serialize};

/// Which capture backend produced an event. Payload shape follows from this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    /// `CGEventTap` at `kCGHIDEventTap`.
    MacOs,
    /// `WH_KEYBOARD_LL` / `WH_MOUSE_LL` low-level hooks.
    Windows,
    /// `evdev` character devices.
    Linux,
}

/// A single captured event, tagged by originating backend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "snake_case")]
pub enum RawPayload {
    MacOs(MacEvent),
    Windows(WindowsEvent),
    Linux(LinuxEvent),
}

impl RawPayload {
    #[must_use]
    pub fn platform(&self) -> Platform {
        match self {
            Self::MacOs(_) => Platform::MacOs,
            Self::Windows(_) => Platform::Windows,
            Self::Linux(_) => Platform::Linux,
        }
    }
}

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------

/// A `CGEvent` flattened to its accessible fields.
///
/// `flags` is the raw `CGEventFlags` bitfield rather than a decoded modifier set: macOS
/// reports left and right modifiers as distinct device-dependent bits on top of the
/// generic ones, and which bits actually appear varies by keyboard. We record all 64 and
/// work out the asymmetry from real traces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MacEvent {
    /// `CGEventType` discriminant.
    pub event_type: u32,
    /// Mach absolute time as reported by the event itself, not our clock.
    pub cg_timestamp: u64,
    pub flags: u64,
    /// `kCGKeyboardEventKeycode`. Virtual keycode, not a HID usage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keycode: Option<u16>,
    /// `kCGKeyboardEventAutorepeat`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autorepeat: Option<bool>,
    /// `CGEventKeyboardGetUnicodeString`. Present even when empty so we can tell
    /// "produced nothing" apart from "never asked".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unicode: Option<String>,
    /// Global display coordinates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<[f64; 2]>,
    /// `kCGMouseEventDeltaX` / `Y`. Post-acceleration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mouse_delta: Option<[i64; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub button_number: Option<i64>,
    /// `kCGMouseEventClickState`: 1 single, 2 double, 3 triple.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub click_state: Option<i64>,
    /// Coarse line-based scroll (`DeltaAxis1`, `DeltaAxis2`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scroll_lines: Option<[i64; 2]>,
    /// Pixel-precise scroll (`PointDeltaAxis1`, `PointDeltaAxis2`). Trackpads populate
    /// this and leave the line deltas near zero; wheels do the reverse.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scroll_pixels: Option<[i64; 2]>,
    /// `kCGScrollWheelEventIsContinuous`. Distinguishes trackpad from notched wheel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scroll_continuous: Option<bool>,
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

/// A low-level hook callback, flattened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowsEvent {
    /// The `wParam` message: `WM_KEYDOWN`, `WM_LBUTTONUP`, `WM_MOUSEWHEEL`, and so on.
    pub message: u32,
    /// `time` field from the hook struct, in `GetTickCount` units.
    pub hook_time: u32,
    /// `dwExtraInfo`. Nonzero commonly means the event was synthesized, which is how we
    /// avoid capturing our own injected input in a loop.
    pub extra_info: u64,
    pub flags: u32,
    /// `KBDLLHOOKSTRUCT::vkCode`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vk_code: Option<u32>,
    /// `KBDLLHOOKSTRUCT::scanCode`. Closer to physical identity than the virtual key,
    /// and the field we expect to map onto HID usages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan_code: Option<u32>,
    /// Screen coordinates from `MSLLHOOKSTRUCT::pt`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub point: Option<[i32; 2]>,
    /// `MSLLHOOKSTRUCT::mouseData`. High word carries wheel delta or the X-button index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mouse_data: Option<u32>,
}

// ---------------------------------------------------------------------------
// Linux
// ---------------------------------------------------------------------------

/// One `input_event` from an evdev device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinuxEvent {
    /// Source device node, e.g. `/dev/input/event3`. Multiple devices interleave into a
    /// single stream, and `EV_SYN` boundaries are only meaningful per-device.
    pub device: String,
    /// `EV_KEY`, `EV_REL`, `EV_ABS`, `EV_SYN`.
    pub ev_type: u16,
    /// `KEY_*` / `REL_*` code. Linux keycodes are already close to HID usages.
    pub code: u16,
    /// Key: 0 release, 1 press, 2 autorepeat. Relative axes: the delta.
    pub value: i32,
    /// Kernel-stamped time, microseconds.
    pub ev_time_us: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_fields_are_omitted_not_nulled() {
        // A keyboard event carries no mouse fields. Emitting `"point": null` for every
        // keystroke would roughly double trace size for no information.
        let ev = RawPayload::Windows(WindowsEvent {
            message: 0x0100,
            hook_time: 42,
            extra_info: 0,
            flags: 0,
            vk_code: Some(0x41),
            scan_code: Some(0x1E),
            point: None,
            mouse_data: None,
        });
        let json = serde_json::to_string(&ev).unwrap();
        assert!(!json.contains("point"), "absent fields must vanish: {json}");
        assert!(!json.contains("null"), "no nulls in the wire form: {json}");
        assert!(json.contains(r#""backend":"windows""#));
    }

    #[test]
    fn zero_valued_fields_survive_the_round_trip() {
        // `Some(0)` and `None` mean different things: a real zero delta versus a field
        // the backend never populated. Skipping on emptiness would collapse them.
        let ev = RawPayload::MacOs(MacEvent {
            event_type: 22,
            cg_timestamp: 1,
            flags: 0,
            keycode: None,
            autorepeat: None,
            unicode: None,
            location: Some([0.0, 0.0]),
            mouse_delta: Some([0, 0]),
            button_number: Some(0),
            click_state: None,
            scroll_lines: Some([0, 0]),
            scroll_pixels: Some([0, -3]),
            scroll_continuous: Some(true),
        });
        let json = serde_json::to_string(&ev).unwrap();
        let back: RawPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn platform_follows_the_payload_variant() {
        let ev = RawPayload::Linux(LinuxEvent {
            device: "/dev/input/event3".into(),
            ev_type: 1,
            code: 30,
            value: 1,
            ev_time_us: 0,
        });
        assert_eq!(ev.platform(), Platform::Linux);
    }
}
