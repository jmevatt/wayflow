//! Receive side of wayflow.
//!
//! Listens on loopback only. The stream is carried by an SSH tunnel, so keystrokes never
//! traverse the LAN in any form and binding a routable interface would only widen the
//! attack surface for no gain.

#[cfg(target_os = "macos")]
mod mac;

use std::process::ExitCode;

#[cfg(target_os = "macos")]
use std::io::{BufRead, BufReader};
#[cfg(target_os = "macos")]
use std::net::{Ipv4Addr, TcpListener, TcpStream};
#[cfg(target_os = "macos")]
use wayflow_proto::Msg;

const DEFAULT_PORT: u16 = 47821;

fn main() -> ExitCode {
    // Reading the cursor needs no Accessibility grant, unlike posting events, so this
    // works over SSH and gives the host side a way to verify injection landed.
    if std::env::args().nth(1).as_deref() == Some("cursor") {
        return report_cursor();
    }
    let port = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(DEFAULT_PORT);
    run(port)
}

#[cfg(target_os = "macos")]
fn run(port: u16) -> ExitCode {
    let mut injector = match mac::Injector::new() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("wayflow-client: {e}");
            return ExitCode::FAILURE;
        }
    };

    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("wayflow-client: cannot bind 127.0.0.1:{port}: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("wayflow-client: listening on 127.0.0.1:{port}");

    for stream in listener.incoming() {
        match stream {
            Ok(s) => serve(s, &mut injector),
            Err(e) => eprintln!("wayflow-client: accept failed: {e}"),
        }
        // A dropped connection must not leave modifiers latched on this machine.
        injector.leave();
    }
    ExitCode::SUCCESS
}

#[cfg(target_os = "macos")]
fn serve(stream: TcpStream, injector: &mut mac::Injector) {
    // Input is worthless if it arrives late, and Nagle would hold small packets back
    // waiting for company. This is the single most important socket option here.
    if let Err(e) = stream.set_nodelay(true) {
        eprintln!("wayflow-client: TCP_NODELAY refused: {e}");
    }
    let peer = stream
        .peer_addr()
        .map_or_else(|_| "?".to_owned(), |a| a.to_string());
    eprintln!("wayflow-client: connected {peer}");

    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        match Msg::from_line(&line) {
            Ok(Msg::Hello {
                proto_version,
                host,
            }) => eprintln!("wayflow-client: host {host} speaking v{proto_version}"),
            Ok(Msg::Enter { edge_ratio }) => injector.enter(edge_ratio),
            Ok(Msg::Leave) => injector.leave(),
            Ok(Msg::Input(input)) => injector.apply(&input),
            // Keep serving: one unparseable line is not a reason to drop a live session,
            // but it must be visible rather than silently skipped.
            Err(e) => eprintln!("wayflow-client: bad frame ({e}): {line}"),
        }
    }
    eprintln!("wayflow-client: {peer} disconnected");
}

#[cfg(target_os = "macos")]
fn report_cursor() -> ExitCode {
    let (x, y) = mac::cursor_position();
    println!("{x:.1},{y:.1}");
    ExitCode::SUCCESS
}

#[cfg(not(target_os = "macos"))]
fn report_cursor() -> ExitCode {
    eprintln!("wayflow-client: macOS only");
    ExitCode::FAILURE
}

#[cfg(not(target_os = "macos"))]
fn run(_port: u16) -> ExitCode {
    eprintln!("wayflow-client: injection backend is macOS-only so far");
    ExitCode::FAILURE
}
