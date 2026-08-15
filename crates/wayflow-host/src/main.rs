//! Capture side of wayflow.
//!
//! Milestone 1 target: pointer crosses the left edge of the leftmost output and lands on
//! the Mac, with keystrokes following it.

#[cfg(target_os = "linux")]
mod globals;
#[cfg(target_os = "linux")]
mod sink;
#[cfg(target_os = "linux")]
mod wlr;

use std::process::ExitCode;

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("check") | None => run_check(),
        Some("edge") => run_edge(std::env::args().nth(2).and_then(|a| a.parse().ok())),
        Some(other) => {
            eprintln!("wayflow-host: unknown command {other:?} (try: check, edge)");
            ExitCode::FAILURE
        }
    }
}

#[cfg(target_os = "linux")]
fn run_edge(port: Option<u16>) -> ExitCode {
    let mut config = wlr::Config::default();
    if let Some(p) = port {
        config.port = p;
    }
    eprintln!(
        "wayflow-host: watching the {:?} edge, client on port {}",
        config.edge, config.port
    );
    match wlr::run(config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("wayflow-host: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn run_edge(_port: Option<u16>) -> ExitCode {
    eprintln!("wayflow-host: capture backend is Linux-only so far");
    ExitCode::FAILURE
}

#[cfg(target_os = "linux")]
fn run_check() -> ExitCode {
    let advertised = match globals::probe() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("wayflow-host: {e}");
            return ExitCode::FAILURE;
        }
    };

    println!("{:<46} {:>7}  PURPOSE", "INTERFACE", "VERSION");
    for req in globals::REQUIREMENTS {
        let (mark, version) = match advertised.version_of(req.interface) {
            Some(v) => ("ok  ", v.to_string()),
            None if req.required => ("MISS", "-".to_owned()),
            None => ("warn", "-".to_owned()),
        };
        println!(
            "{mark} {:<41} {version:>7}  {}",
            req.interface, req.purpose
        );
    }

    let missing = advertised.missing_required();
    if missing.is_empty() {
        println!("\nall required protocols present");
        ExitCode::SUCCESS
    } else {
        eprintln!("\n{} required protocol(s) missing:", missing.len());
        for r in &missing {
            eprintln!("  {} -- needed to {}", r.interface, r.purpose);
        }
        ExitCode::FAILURE
    }
}

#[cfg(not(target_os = "linux"))]
fn run_check() -> ExitCode {
    eprintln!("wayflow-host: capture backend is Linux-only so far");
    ExitCode::FAILURE
}
