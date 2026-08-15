//! End-to-end check that events crossing the kernel reach our capture path intact.
//!
//! 🚨 This test creates a real uinput device and emits real key events into the live
//! session. The kernel cannot tell them apart from a physical keyboard, so whatever
//! window has focus receives them. It uses `KEY_F24`, which essentially nothing binds,
//! and it is `#[ignore]`d so a routine `cargo test` never fires input at an unsuspecting
//! desktop. Run it deliberately:
//!
//! ```text
//! cargo test -p wayflow-probe --test uinput_roundtrip -- --ignored --nocapture
//! ```
//!
//! Requires write access to `/dev/uinput` and membership in the `input` group.

#![cfg(target_os = "linux")]

use std::time::{Duration, Instant};

use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent, KeyCode};
use wayflow_proto::RawPayload;

const DEVICE_NAME: &str = "wayflow-probe-selftest";
/// Chosen because desktops essentially never bind it. A test that types `a` into the
/// focused window would be a genuinely hostile thing to run.
const TEST_KEY: KeyCode = KeyCode::KEY_F24;
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(3);

fn virtual_keyboard() -> VirtualDevice {
    let mut keys = AttributeSet::<KeyCode>::new();
    keys.insert(TEST_KEY);
    VirtualDevice::builder()
        .expect("open /dev/uinput -- is it writable, and is uinput loaded?")
        .name(DEVICE_NAME)
        .with_keys(&keys)
        .expect("declare key capability")
        .build()
        .expect("register virtual device")
}

/// Wait for udev to publish the node. Creation is asynchronous: enumerating immediately
/// after `build()` reliably finds nothing, and the node also has to pick up group
/// ownership before we can open it.
fn await_node() -> std::path::PathBuf {
    let deadline = Instant::now() + DISCOVERY_TIMEOUT;
    while Instant::now() < deadline {
        if let Some(c) = wayflow_probe::linux::candidates()
            .into_iter()
            .find(|c| c.name == DEVICE_NAME)
        {
            return c.path;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("virtual device never appeared in /dev/input within {DISCOVERY_TIMEOUT:?}");
}

#[test]
#[ignore = "emits real input events into the live session; run explicitly"]
fn emitted_key_events_survive_the_round_trip() {
    let mut vdev = virtual_keyboard();
    let path = await_node();

    let t0 = Instant::now();
    let rx = wayflow_probe::linux::spawn_readers(std::slice::from_ref(&path), t0)
        .expect("open the virtual device for reading");
    // The reader thread races the first emit; without this the press is dropped and the
    // failure looks like a capture bug rather than a startup ordering one.
    std::thread::sleep(Duration::from_millis(150));

    let press = InputEvent::new(EventType::KEY.0, TEST_KEY.code(), 1);
    let release = InputEvent::new(EventType::KEY.0, TEST_KEY.code(), 0);
    let emitted_at = Instant::now();
    vdev.emit(&[press, release]).expect("emit key events");

    let mut key_events = Vec::new();
    let deadline = Instant::now() + CAPTURE_TIMEOUT;
    while Instant::now() < deadline && key_events.len() < 2 {
        if let Ok((t_mono_ns, payload)) = rx.recv_timeout(Duration::from_millis(200)) {
            let RawPayload::Linux(ev) = &payload else {
                panic!("linux reader produced a non-linux payload: {payload:?}");
            };
            if ev.ev_type == EventType::KEY.0 {
                key_events.push((t_mono_ns, ev.clone()));
            }
        }
    }

    assert_eq!(
        key_events.len(),
        2,
        "expected press+release, captured {key_events:#?}"
    );

    let (t_press, press_ev) = &key_events[0];
    let (_, release_ev) = &key_events[1];

    assert_eq!(
        press_ev.code,
        TEST_KEY.code(),
        "keycode must survive intact"
    );
    assert_eq!(press_ev.value, 1, "first event should be the press");
    assert_eq!(release_ev.value, 0, "second event should be the release");
    assert_eq!(release_ev.code, TEST_KEY.code());
    assert!(
        press_ev.device.contains("event"),
        "device path should be recorded: {}",
        press_ev.device
    );
    assert!(
        press_ev.ev_time_us > 0,
        "kernel timestamp must be populated, got {}",
        press_ev.ev_time_us
    );

    // Not an assertion, a measurement: this is the kernel's own uinput-to-evdev turnaround
    // and it is the floor any Phase 1 latency budget has to be built on top of.
    let observed = u64::try_from(emitted_at.duration_since(t0).as_nanos()).unwrap_or(0);
    let delta_us = t_press.saturating_sub(observed) / 1_000;
    println!("kernel uinput -> evdev round trip: {delta_us} us");
}
