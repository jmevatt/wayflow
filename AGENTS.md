# Wayflow Agent Guide

Wayflow is a ground-up Rust rewrite of keyboard/mouse sharing (KVM-over-network),
targeting Wayland-first Linux with cross-platform support for macOS and Windows.
It is built by Jordan Evatt (jordan@evattlabs.com) using agentic coding tools.

---

## Why this exists

Upstream Deskflow (the C++/Qt fork of Synergy) does not accept AI-generated
contributions and has architectural problems: Qt threading constraints cause
clipboard crashes, wl-copy subprocesses appear as taskbar entries, and the
protocol carries 20 years of X11 assumptions. Wayflow starts clean.

---

## Goals

- Wayland-native: libei/EIS for injection, XDG RemoteDesktop portal for capture.
  No external subprocess for clipboard. No Qt.
- Cross-platform: Linux (Wayland + X11), macOS, Windows x86_64. Jordan has
  hardware for all three and tests across them.
- Memory safe, async-native: tokio throughout, no thread-blocking I/O.
- Clean protocol: postcard over TLS, no legacy Synergy wire format.

---

## Workspace layout

```
crates/
  wayflow-proto/    wire types only -- no I/O, compiles no_std
  wayflow-core/     protocol logic, config, framed TLS transport
  wayflow-server/   input capture backends + server loop
  wayflow-client/   input injection backends + client loop
apps/
  wayflow-cli/      CLI binary (wayflow server / wayflow client)
  wayflow-gui/      Tauri v2 GUI -- tray icon + settings (not yet scaffolded)
```

---

## Protocol

### Framing

All messages are serialized with `postcard` and prefixed with a 4-byte
little-endian message length. The TCP connection is wrapped with TLS
(rustls + tokio-rustls). The server generates a self-signed cert on first
run (rcgen); clients use TOFU -- accept on first connection, pin thereafter.

### Handshake

```
client -> server: C2S::Hello { version, name, screens }
server -> client: S2C::Hello { version, screens }
```

Version mismatch: server closes with an error if `version != PROTOCOL_VERSION`.

### Ongoing messages

Server -> Client:
  S2C::EnterScreen { x, y }        cursor entering client screen at (x,y)
  S2C::LeaveScreen                 cursor left; client stops injecting
  S2C::MouseMoveAbs { x, y }       absolute coords within client screen
  S2C::MouseButton { button, pressed }
  S2C::Scroll { dx, dy }           logical pixel deltas
  S2C::KeyEvent { keycode, pressed, modifiers }
  S2C::ClipboardData(content)      server clipboard changed
  S2C::Ping

Client -> Server:
  C2S::ClipboardData(content)      client clipboard changed
  C2S::Pong

### Coordinate system

- All coordinates are screen-local: (0,0) is the top-left of that screen.
- Edge detection is server-side: when the cursor reaches a configured edge,
  the server sends EnterScreen to the mapped client and begins routing events.
- Screen dimensions are exchanged in the Hello message.

---

## Key crates and why

| Crate              | Used in           | Purpose                                      |
|--------------------|-------------------|----------------------------------------------|
| tokio              | everywhere        | async runtime                                |
| tokio-rustls       | core              | async TLS over tokio streams                 |
| rustls             | core              | TLS implementation (no OpenSSL)              |
| rcgen              | core              | self-signed cert generation                  |
| postcard           | proto, core       | compact no_std-compatible serialization      |
| serde              | proto, core       | derive Serialize/Deserialize                 |
| thiserror          | crates (libraries)| typed error enums                            |
| anyhow             | apps, binaries    | error propagation at call sites              |
| tracing            | everywhere        | structured logging                           |
| clap               | wayflow-cli       | CLI argument parsing                         |
| ashpd              | server (linux)    | XDG desktop portal (RemoteDesktop capture)   |
| reis               | client (linux)    | libei Rust bindings for EIS injection        |
| rdev               | server+client     | input capture/inject on macOS + Windows      |
| smithay-clipboard  | client (linux)    | Wayland clipboard read/write                 |
| arboard            | client (non-linux)| clipboard on macOS + Windows                 |
| dirs               | core              | platform config/data dir paths               |
| toml               | core              | config file parsing                          |

---

## Input backends

### Server -- capture (reads local input, sends to clients)

| Platform        | Crate/API                  | File                                      |
|-----------------|----------------------------|-------------------------------------------|
| Linux/Wayland   | ashpd RemoteDesktop portal | server/src/backend/linux_wayland.rs       |
| Linux/X11       | x11rb + XGrab              | server/src/backend/linux_x11.rs (TODO)    |
| macOS           | rdev CGEventTap            | server/src/backend/rdev_backend.rs (TODO) |
| Windows         | rdev SetWindowsHookEx      | server/src/backend/rdev_backend.rs (TODO) |

The ashpd flow on Wayland:
1. `RemoteDesktop::new().await` -- open a portal session
2. `session.select_devices(DeviceType::Pointer | DeviceType::Keyboard).await`
3. `session.start().await` -- compositor shows permission prompt once
4. `session.connect_to_eis().await` -- returns an EIS file descriptor
5. Wrap fd with `reis::EiClient`, read `InputEvent` loop

### Client -- injection (receives events from server, injects locally)

| Platform        | Crate/API                  | File                                      |
|-----------------|----------------------------|-------------------------------------------|
| Linux/Wayland   | reis (libei / EIS)         | client/src/backend/linux_wayland.rs       |
| Linux/X11       | x11rb + XTest              | client/src/backend/linux_x11.rs (TODO)    |
| macOS           | rdev                       | client/src/backend/rdev_backend.rs (TODO) |
| Windows         | rdev / windows crate       | client/src/backend/rdev_backend.rs (TODO) |

The reis flow on Wayland:
1. Get an EIS fd from the compositor (via portal or direct socket).
2. `reis::EiClient::from_fd(fd)` -- connect
3. Negotiate Seat + Pointer + Keyboard interfaces
4. Inject with `pointer.motion_absolute(x, y)`, `keyboard.key(code, state)`, etc.

### Clipboard

| Platform        | Crate              | Notes                                        |
|-----------------|--------------------|----------------------------------------------|
| Linux/Wayland   | smithay-clipboard  | wraps wl_data_device_manager natively        |
| Linux/X11       | arboard            |                                              |
| macOS           | arboard            |                                              |
| Windows         | arboard            |                                              |

Do NOT spawn wl-copy or wl-paste subprocesses. The whole point is native clipboard.

---

## Testing

### Same-machine Wayland-to-Wayland (primary dev loop)

```bash
# Terminal 1 -- fake client machine (nested compositor)
weston --socket=wayland-test &

# Terminal 2 -- server in main session
WAYFLOW_LOG=debug cargo run -p wayflow-cli -- server

# Terminal 3 -- client in nested compositor
WAYLAND_DISPLAY=wayland-test WAYFLOW_LOG=debug cargo run -p wayflow-cli -- client --server 127.0.0.1
```

Move cursor to the configured screen edge. Verify:
- Cursor appears and moves in the weston window
- Keyboard input appears in weston
- Clipboard syncs bidirectionally

### Cross-platform (Jordan has all three)

Jordan has Linux (helicon, NixOS), macOS, and Windows x86_64 machines.
Use them for integration testing once the Wayland-to-Wayland loop is solid.
Platform-specific edge cases (permission prompts, codesign on macOS,
UAC on Windows) should be tested on the actual hardware.

### Unit tests

```bash
cargo test --workspace
```

wayflow-proto must have tests for round-trip serialization of every message type.
wayflow-core must have tests for the framing/transport layer using in-memory streams.

---

## Code conventions

### Error handling
- Library crates (`wayflow-proto`, `wayflow-core`, `wayflow-server`, `wayflow-client`):
  use `thiserror` for typed error enums. Expose them in `crate::Error`.
- Binary/app crates (`wayflow-cli`, `wayflow-gui`): use `anyhow` at entry points.
- Never `unwrap()` in library code. Use `?` and propagate.
- Never `expect()` unless the invariant is documented and genuinely impossible to violate.

### Async
- `tokio` throughout. No `std::thread::spawn` for I/O-bound work.
- Blocking operations (filesystem, blocking C libs) go in `tokio::task::spawn_blocking`.
- `tokio::sync::Mutex` / `RwLock` for shared async state; `std::sync::Mutex` only
  for short-lived guards that never await.

### Platform gating
- All platform-conditional code lives in `backend/` modules.
- Gate with `#[cfg(target_os = "linux")]` etc. at the module level.
- The `mod.rs` in each backend directory is platform-agnostic and defines the trait.
- Never put platform-specific code in the crate root or lib.rs.

### Logging
- `tracing` crate: `trace!`, `debug!`, `info!`, `warn!`, `error!`.
- No `println!` or `eprintln!` in library code.
- Span context for connections: `tracing::instrument` on connection handlers.
- Log level env var: `WAYFLOW_LOG` (e.g. `WAYFLOW_LOG=wayflow=debug`).

### wayflow-proto rules
- Must compile with `#![no_std]` (uses `extern crate alloc`).
- Zero I/O. Zero platform deps. Zero network calls.
- Every message type needs a round-trip serde test.

### Config
- TOML format, stored in `dirs::config_dir() / "wayflow" / "config.toml"`.
- Never hardcode paths.
- Never store secrets in plaintext config. TLS certs go in
  `dirs::data_local_dir() / "wayflow" / "certs"`.

---

## What NOT to do

- Do not spawn subprocesses for clipboard (wl-copy, wl-paste, xclip, pbcopy, etc.).
  Use the native crates listed above.
- Do not add Qt, GTK, or any C++ GUI framework.
- Do not use `std::process::Command` to call system tools for input injection.
  Use the Rust crates or direct syscalls.
- Do not add synchronous blocking I/O on the async task thread. Use spawn_blocking.
- Do not put platform-specific code outside of `backend/` modules.
- Do not add `Cargo.lock` to `.gitignore` for the CLI binary (it should be tracked
  for reproducible builds). It IS in .gitignore for library crates -- check the
  workspace .gitignore and adjust per Cargo conventions.
- Do not use `unsafe` without a `// SAFETY:` comment explaining the invariant.
- wayflow-proto must stay no_std. Do not add std-only dependencies to it.

---

## Build

```bash
# Dev build (Linux, in nix develop shell)
nix develop
cargo build --workspace

# Check everything compiles
cargo check --workspace

# Run tests
cargo nextest run --workspace   # preferred (faster, better output)
cargo test --workspace          # fallback

# Cross-compile checks (requires cross)
cross check --target x86_64-apple-darwin
cross check --target x86_64-pc-windows-gnu

# Run the CLI
WAYFLOW_LOG=debug cargo run -p wayflow-cli -- server
WAYFLOW_LOG=debug WAYLAND_DISPLAY=wayland-test cargo run -p wayflow-cli -- client --server 127.0.0.1
```

The nix `devShell` provides `libei`, `wayland`, `wayland-protocols`, and `weston`
for the nested compositor test loop.

---

## Current status (as of scaffold)

| Component            | Status      |
|----------------------|-------------|
| wayflow-proto        | complete (types + framing) |
| wayflow-core transport | complete (framing, TLS scaffolding) |
| wayflow-core config  | complete    |
| wayflow-cli          | scaffolded (no-op) |
| server loop          | scaffolded (TCP listen, no TLS/handshake yet) |
| client loop          | scaffolded (no-op) |
| Linux/Wayland capture (ashpd) | TODO |
| Linux/Wayland inject (reis)   | TODO |
| macOS capture/inject (rdev)   | TODO |
| Windows capture/inject (rdev) | TODO |
| Clipboard (all platforms)     | TODO |
| Tauri GUI            | not yet scaffolded -- use `cargo tauri init` in apps/wayflow-gui |

Phase 1 work: implement the server loop (TLS, handshake, event broadcast),
client loop (TLS, handshake, inject), and the Linux/Wayland backends end-to-end.
Test with the nested weston compositor before touching other platforms.
