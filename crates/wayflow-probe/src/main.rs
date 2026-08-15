//! Phase 0 capture harness.
//!
//! Streams verbatim platform input events as NDJSON on stdout. It interprets nothing:
//! deciding what a keycode means happens off-box, where a wrong guess costs a rebuild
//! instead of a trip to the endpoint.

#[cfg(target_os = "linux")]
use wayflow_probe::linux;

use std::process::ExitCode;

const USAGE: &str = "\
wayflow-probe -- Phase 0 input capture

USAGE:
    wayflow-probe list              Show input devices and how they were classified
    wayflow-probe capture [--syn]   Stream NDJSON events on stdout (Ctrl-C to stop)
    wayflow-probe move DX DY [N]    Emit N relative motions of (DX,DY) via uinput
    wayflow-probe key CODE          Press and release one evdev key code via uinput

    --syn   Include EV_SYN delimiters, which are otherwise filtered as noise
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str);
    match cmd {
        Some("list") => run_list(),
        Some("capture") => run_capture(args.iter().any(|a| a == "--syn")),
        Some("move") => run_move(&args),
        Some("key") => run_key(&args),
        Some("-h" | "--help") | None => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("wayflow-probe: unknown command {other:?}\n\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(target_os = "linux")]
fn run_list() -> ExitCode {
    let found = linux::candidates();
    if found.is_empty() {
        eprintln!(
            "wayflow-probe: no readable devices in /dev/input.\n\
             Membership in the 'input' group is required, and it only takes effect \
             after a fresh login."
        );
        return ExitCode::FAILURE;
    }
    for c in &found {
        let kind = match (c.keyboard, c.pointer) {
            (true, true) => "kbd+ptr",
            (true, false) => "kbd",
            (false, true) => "ptr",
            (false, false) => "-",
        };
        println!("{:<24} {:<8} {}", c.path.display(), kind, c.name);
    }
    ExitCode::SUCCESS
}

#[cfg(target_os = "linux")]
fn run_capture(include_syn: bool) -> ExitCode {
    use std::io::Write;
    use std::time::Instant;
    use wayflow_proto::{Event, Frame, Hello, PROTO_VERSION, Platform};

    let t0 = Instant::now();
    let paths = linux::default_paths();
    if paths.is_empty() {
        eprintln!("wayflow-probe: no keyboard or pointer devices readable");
        return ExitCode::FAILURE;
    }
    eprintln!(
        "wayflow-probe: capturing {} device(s), read-only",
        paths.len()
    );

    let rx = match linux::spawn_readers(&paths, t0) {
        Ok(rx) => rx,
        Err(e) => {
            eprintln!("wayflow-probe: cannot open devices: {e}");
            return ExitCode::FAILURE;
        }
    };

    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    let hello = Frame::Hello(Hello {
        proto_version: PROTO_VERSION,
        probe_version: env!("CARGO_PKG_VERSION").to_owned(),
        platform: Platform::Linux,
        hostname: hostname(),
        mono_epoch_unix_ns: linux::mono_epoch_unix_ns(t0),
        // evdev sits below the display server and has no concept of monitor geometry.
        // Populating this on Linux is a compositor question, not a capture one.
        displays: Vec::new(),
    });
    if !emit(&mut out, &hello) {
        return ExitCode::FAILURE;
    }

    let mut seq = 0u64;
    for (t_mono_ns, payload) in rx {
        if !include_syn && linux::is_syn(&payload) {
            continue;
        }
        let frame = Frame::Event(Event {
            seq,
            t_mono_ns,
            payload,
        });
        seq += 1;
        if !emit(&mut out, &frame) {
            return ExitCode::FAILURE;
        }
    }
    let _ = out.flush();
    ExitCode::SUCCESS
}

/// Write one frame, flushing immediately.
///
/// Unbuffered on purpose: a probe whose output sits in an 8KB buffer looks indistinguishable
/// from a probe that captured nothing, and that ambiguity costs far more than the syscalls.
/// Returns false once the sink is gone, so a closed pipe ends the run quietly rather than
/// with a broken-pipe panic.
fn emit(out: &mut impl std::io::Write, frame: &wayflow_proto::Frame) -> bool {
    let Ok(line) = frame.to_line() else {
        eprintln!("wayflow-probe: frame failed to serialize");
        return false;
    };
    out.write_all(line.as_bytes()).is_ok() && out.flush().is_ok()
}

#[cfg(target_os = "linux")]
fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map_or_else(|_| "unknown".to_owned(), |s| s.trim().to_owned())
}

/// Emit relative pointer motion through a virtual device.
///
/// Real motion, indistinguishable from a mouse, so it exercises libinput and the
/// compositor rather than bypassing them the way a cursor warp does. That distinction
/// matters: relative-pointer events only exist on this path.
#[cfg(target_os = "linux")]
fn run_move(args: &[String]) -> ExitCode {
    use evdev::uinput::VirtualDevice;
    use evdev::{AttributeSet, EventType, InputEvent, KeyCode, RelativeAxisCode};

    let dx: i32 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(0);
    let dy: i32 = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(0);
    let count: u32 = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(1);

    let mut axes = AttributeSet::<RelativeAxisCode>::new();
    axes.insert(RelativeAxisCode::REL_X);
    axes.insert(RelativeAxisCode::REL_Y);
    // A relative device with no buttons is classified as something other than a mouse by
    // libinput and its motion is ignored, so BTN_LEFT has to be declared even unused.
    let mut keys = AttributeSet::<KeyCode>::new();
    keys.insert(KeyCode::BTN_LEFT);

    let Ok(builder) = VirtualDevice::builder() else {
        eprintln!("wayflow-probe: cannot open /dev/uinput");
        return ExitCode::FAILURE;
    };
    let Ok(mut dev) = builder
        .name("wayflow-probe-mouse")
        .with_relative_axes(&axes)
        .and_then(|b| b.with_keys(&keys))
        .and_then(evdev::uinput::VirtualDeviceBuilder::build)
    else {
        eprintln!("wayflow-probe: cannot create virtual mouse");
        return ExitCode::FAILURE;
    };
    // libinput needs a moment to notice and configure the new device; motion sent before
    // that is silently discarded.
    std::thread::sleep(std::time::Duration::from_millis(400));

    for _ in 0..count {
        let events = [
            InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_X.0, dx),
            InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_Y.0, dy),
        ];
        if dev.emit(&events).is_err() {
            eprintln!("wayflow-probe: emit failed");
            return ExitCode::FAILURE;
        }
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
    eprintln!("wayflow-probe: emitted {count} motions of ({dx}, {dy})");
    ExitCode::SUCCESS
}

/// Press and release one evdev key through a virtual keyboard.
#[cfg(target_os = "linux")]
fn run_key(args: &[String]) -> ExitCode {
    use evdev::uinput::VirtualDevice;
    use evdev::{AttributeSet, EventType, InputEvent, KeyCode};

    let Some(code) = args.get(1).and_then(|a| a.parse::<u16>().ok()) else {
        eprintln!("wayflow-probe: key needs an evdev code, e.g. 30 for A");
        return ExitCode::FAILURE;
    };
    let mut keys = AttributeSet::<KeyCode>::new();
    keys.insert(KeyCode::new(code));

    let Ok(mut dev) = VirtualDevice::builder()
        .and_then(|b| b.name("wayflow-probe-kbd").with_keys(&keys))
        .and_then(evdev::uinput::VirtualDeviceBuilder::build)
    else {
        eprintln!("wayflow-probe: cannot create virtual keyboard");
        return ExitCode::FAILURE;
    };
    std::thread::sleep(std::time::Duration::from_millis(400));
    for value in [1, 0] {
        if dev
            .emit(&[InputEvent::new(EventType::KEY.0, code, value)])
            .is_err()
        {
            return ExitCode::FAILURE;
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    eprintln!("wayflow-probe: pressed evdev key {code}");
    ExitCode::SUCCESS
}

#[cfg(not(target_os = "linux"))]
fn run_key(_args: &[String]) -> ExitCode {
    eprintln!("wayflow-probe: uinput is Linux-only");
    ExitCode::FAILURE
}

#[cfg(not(target_os = "linux"))]
fn run_move(_args: &[String]) -> ExitCode {
    eprintln!("wayflow-probe: uinput is Linux-only");
    ExitCode::FAILURE
}

#[cfg(not(target_os = "linux"))]
fn run_list() -> ExitCode {
    eprintln!("wayflow-probe: no capture backend built for this platform yet");
    ExitCode::FAILURE
}

#[cfg(not(target_os = "linux"))]
fn run_capture(_include_syn: bool) -> ExitCode {
    eprintln!("wayflow-probe: no capture backend built for this platform yet");
    ExitCode::FAILURE
}
