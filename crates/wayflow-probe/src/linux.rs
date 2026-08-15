//! evdev capture.
//!
//! Read-only by design. evdev offers `EVIOCGRAB` to take exclusive ownership of a device,
//! which is what a real KVM host needs, and which Phase 0 deliberately does not use: a
//! grab on the keyboard you are typing this with has no kill switch, and a panic while
//! holding one leaves the desk unusable until a TTY switch. Grabbing is a Phase 1 problem
//! with a Phase 1 escape hatch. Right now we only watch.

use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use evdev::{Device, EventType, KeyCode, RelativeAxisCode};
use wayflow_proto::{LinuxEvent, RawPayload};

/// A device we chose to listen to, with the reason we found it interesting.
#[derive(Debug)]
pub struct Candidate {
    pub path: PathBuf,
    pub name: String,
    pub keyboard: bool,
    pub pointer: bool,
}

impl Candidate {
    fn interesting(&self) -> bool {
        self.keyboard || self.pointer
    }
}

/// Enumerate `/dev/input`, classifying by capability rather than by name.
///
/// Name matching would be a trap here: this laptop exposes four separate nodes all called
/// some variant of "ROG OMNI RECEIVER", only some of which carry keys.
#[must_use]
pub fn candidates() -> Vec<Candidate> {
    let mut found: Vec<Candidate> = evdev::enumerate()
        .map(|(path, dev)| {
            let keyboard = dev
                .supported_keys()
                .is_some_and(|k| k.contains(KeyCode::KEY_A));
            let pointer = dev
                .supported_relative_axes()
                .is_some_and(|a| a.contains(RelativeAxisCode::REL_X))
                || dev
                    .supported_keys()
                    .is_some_and(|k| k.contains(KeyCode::BTN_LEFT));
            Candidate {
                path,
                name: dev.name().unwrap_or("<unnamed>").to_owned(),
                keyboard,
                pointer,
            }
        })
        .collect();
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

/// A captured event, stamped at the moment it left the kernel's queue.
pub type Stamped = (u64, RawPayload);

/// Spawn one reader thread per device and merge them onto a single channel.
///
/// A thread each, rather than one poll loop, because Phase 0 is measuring latency: a
/// single-threaded reader would serialize devices behind one another and the queueing
/// delay would show up in the traces as if the kernel had caused it.
///
/// # Errors
/// Returns the first device that could not be opened. A partial capture is worse than a
/// loud failure, because a silently missing device looks identical to a device that
/// simply never fired.
pub fn spawn_readers(paths: &[PathBuf], t0: Instant) -> io::Result<Receiver<Stamped>> {
    let (tx, rx) = mpsc::channel();
    for path in paths {
        let device = Device::open(path)?;
        let label = path.display().to_string();
        let tx: Sender<Stamped> = tx.clone();
        let handle = thread::Builder::new().name(format!("evdev:{label}"));
        handle.spawn(move || read_loop(device, &label, t0, &tx))?;
    }
    // Every clone lives in a thread; dropping the original lets the channel close if they
    // all exit, instead of the consumer blocking forever on a dead capture.
    drop(tx);
    Ok(rx)
}

fn read_loop(mut device: Device, label: &str, t0: Instant, tx: &Sender<Stamped>) {
    loop {
        let batch = match device.fetch_events() {
            Ok(b) => b,
            // The device went away: unplugged receiver, suspend/resume. That ends this
            // thread but must not take the other devices down with it.
            Err(e) => {
                eprintln!("wayflow-probe: {label} closed: {e}");
                return;
            }
        };
        for event in batch {
            // Stamped here, in the reading thread, before the channel hop. Timing this on
            // the consumer side would fold our own queueing delay into the measurement.
            let t_mono_ns = u64::try_from(t0.elapsed().as_nanos()).unwrap_or(u64::MAX);
            let ev_time_us = event
                .timestamp()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros();
            let payload = RawPayload::Linux(LinuxEvent {
                device: label.to_owned(),
                ev_type: event.event_type().0,
                code: event.code(),
                value: event.value(),
                ev_time_us: u64::try_from(ev_time_us).unwrap_or(u64::MAX),
            });
            if tx.send((t_mono_ns, payload)).is_err() {
                return; // consumer hung up
            }
        }
    }
}

/// Wall-clock reading of the monotonic zero point, for cross-machine trace alignment.
#[must_use]
pub fn mono_epoch_unix_ns(t0: Instant) -> u64 {
    let since_start = t0.elapsed();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    u64::try_from(now.saturating_sub(since_start).as_nanos()).unwrap_or(u64::MAX)
}

/// Devices worth capturing, in enumeration order.
#[must_use]
pub fn default_paths() -> Vec<PathBuf> {
    candidates()
        .into_iter()
        .filter(Candidate::interesting)
        .map(|c| c.path)
        .collect()
}

/// `EV_SYN` frames are pure delimiters and dominate the stream by volume.
#[must_use]
pub fn is_syn(payload: &RawPayload) -> bool {
    matches!(payload, RawPayload::Linux(e) if e.ev_type == EventType::SYNCHRONIZATION.0)
}
