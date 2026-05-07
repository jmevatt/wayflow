# Wayflow Agent Guide

Wayflow is a ground-up Rust rewrite of keyboard/mouse sharing (KVM-over-network),
targeting Wayland-first Linux with cross-platform support for macOS and Windows.
Built by Jordan Evatt (jordan@evattlabs.com); developed primarily with agentic
coding tools.

This file is the contract for any agent (or human) opening a PR. Read it end-to-end
before touching code. CONTRIBUTING.md is the human-facing summary; this file is
the operational detail an agent needs to act safely on this repo.

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
- Cross-platform: Linux (Wayland + X11), macOS, Windows x86_64.
- Memory safe, async-native: tokio throughout; no thread-blocking I/O on the
  async runtime.
- Clean wire protocol: postcard over TLS, no legacy Synergy format.
- Single binary: `wayflow {server,client,tray}` subcommands. No Tauri/Qt/Electron.

---

## Workspace layout

```
crates/
  wayflow-proto/    wire types only -- no I/O, compiles no_std
  wayflow-core/     framed TLS transport, config, layout, keymap
  wayflow-server/   input capture backends + server loop + telemetry
  wayflow-client/   input injection backends + client loop
  wayflow-tray/     menu-bar/system-tray supervisor (tray-icon + tao)
apps/
  wayflow-cli/      single binary: `wayflow server | client | tray`
scripts/
  bundle-mac.sh     wrap the binary as a macOS .app (LSUIElement, menu-bar only)
.github/workflows/
  ci.yml            cargo test on ubuntu/macos/windows
```

Source line counts (orientation only, not load-bearing):

| Path                                        | Lines |
|---------------------------------------------|------:|
| crates/wayflow-server/src/server.rs         | ~1050 |
| crates/wayflow-core/src/keymap.rs           |  ~630 |
| crates/wayflow-server/src/backend/linux_wayland.rs | ~590 |
| crates/wayflow-client/src/client.rs         |  ~470 |
| crates/wayflow-client/src/backend/rdev_backend.rs | ~420 |

---

## Protocol

Current `PROTOCOL_VERSION = 3` in `wayflow-proto/src/lib.rs`. **Any change to a
wire type bumps this constant.** Mismatched versions are rejected at handshake.

### Framing

All messages are `postcard`-serialized and prefixed with a 4-byte little-endian
length. Max frame is 4 MiB (`MAX_MESSAGE_BYTES` in `wayflow-core/src/transport.rs`).
The TCP connection is wrapped with TLS (rustls + tokio-rustls). The server
generates a self-signed cert on first run (rcgen); clients use TOFU -- accept
on first connection, pin thereafter (`security: TOFU cert pinning, reject unknown
clients` is shipped, see `crates/wayflow-core/src/tls.rs`).

### Handshake

```
client -> server: C2S::Hello { version, name, screens }
server -> client: S2C::Hello { version, screens }
```

`name` is the client's hostname (from `gethostname(2)` or `$HOSTNAME`) and must
match a `[[clients]]` entry on the server. Unknown clients are rejected.

### Messages

Server -> Client (`S2C` enum):
  Hello(HelloS2C)                  version + advertised screens
  EnterScreen { x, y }             cursor entering this client at (x,y)
  LeaveScreen                      cursor left; client stops injecting
  MouseMoveAbs { x, y }            absolute coords within client screen
  MouseButton { button, pressed }
  Scroll { dx, dy }                logical pixel deltas (i16)
  KeyEvent { keycode, pressed, modifiers }   keycode is USB HID usage (page 0x07)
  ClipboardData(content)           server clipboard changed
  Ping

Client -> Server (`C2S` enum):
  Hello(HelloC2S)
  ClipboardData(content)
  Pong
  ScreenLayoutUpdate { screens }   monitor connect/disconnect on the client

### Coordinate system

`ScreenInfo` carries `(name, x, y, width, height)` where `(x,y)` is the
monitor's top-left in the **virtual desktop** (signed; left-of/above-origin
monitors are valid). Edge detection is server-side: `wayflow-core::layout`
computes the union desktop and routes the cursor to the mapped client when it
crosses a configured edge. Within a screen, on-wire coords are top-left origin.

Keycodes on the wire are USB HID usage codes (keyboard page 0x07). Each platform
translates to/from its native representation in `wayflow-core::keymap` plus
backend-local helpers.

---

## Key crates and why

| Crate              | Used in              | Purpose                                      |
|--------------------|----------------------|----------------------------------------------|
| tokio              | everywhere except proto | async runtime                             |
| tokio-rustls       | core, server, client | async TLS over tokio streams                 |
| rustls             | core                 | TLS impl (ring backend, no OpenSSL)          |
| rcgen              | core                 | self-signed cert generation                  |
| postcard           | proto, core          | compact no_std-compatible serialization      |
| serde              | proto, core          | derive Serialize/Deserialize                 |
| thiserror          | library crates       | typed error enums                            |
| anyhow             | apps + binaries      | error propagation at call sites              |
| tracing            | everywhere           | structured logging                           |
| tracing-appender   | wayflow-cli          | rotating JSON file logs                      |
| clap               | wayflow-cli          | CLI argument parsing                         |
| dirs               | core, cli, tray      | platform config/data/cache dir paths         |
| toml               | core                 | config file parsing                          |
| libc               | wayflow-cli          | gethostname(2)                               |
| tray-icon          | wayflow-tray         | cross-platform tray icon                     |
| tao                | wayflow-tray         | event loop for tray                          |
| open               | wayflow-tray         | open config in $EDITOR                       |

Linux-only crates pulled in via `cfg(target_os = "linux")`: `ashpd`, `reis`,
`smithay-clipboard`. macOS/Windows use `rdev`, `core-graphics` (CGEvent on mac),
and `arboard`. None of these belong in `wayflow-proto`.

---

## Input backends

### Server -- capture (reads local input, sends to clients)

| Platform        | Crate/API                  | File                                              |
|-----------------|----------------------------|---------------------------------------------------|
| Linux/Wayland   | ashpd RemoteDesktop portal | crates/wayflow-server/src/backend/linux_wayland.rs |
| Linux/X11       | TODO                       | --                                                |
| macOS           | rdev / CGEventTap (TODO)   | crates/wayflow-server/src/backend/rdev_backend.rs |
| Windows         | rdev SetWindowsHookEx (TODO) | crates/wayflow-server/src/backend/rdev_backend.rs |

The Linux/Wayland flow:
1. Open a portal session with `RemoteDesktop::new()`.
2. `select_devices(Pointer | Keyboard)` and `start()` (compositor permission prompt).
3. `connect_to_eis()` returns an EIS file descriptor.
4. Wrap with `reis::EiClient`, drive an `InputEvent` loop on a dedicated thread.
5. On `LeaveScreen` (cursor returned to server), the router signals the backend
   to release the compositor grab; on the active client dying the portal session
   is also released (`server: release capture portal when active client dies`).

### Client -- injection (receives events from server, injects locally)

| Platform        | API                            | File                                              |
|-----------------|--------------------------------|---------------------------------------------------|
| Linux/Wayland   | reis (libei / EIS)             | crates/wayflow-client/src/backend/linux_wayland.rs |
| macOS           | CGEvent direct (working)       | crates/wayflow-client/src/backend/rdev_backend.rs |
| Windows         | rdev / windows crate (TODO)    | crates/wayflow-client/src/backend/rdev_backend.rs |

The mac path posts CGEvents directly (not via rdev) for keyboard, mouse buttons,
and tracks click count for `kCGMouseEventClickState`; motion + scroll currently
flow through helper paths. Multi-monitor + hot-swap layout updates are working;
the client re-queries display dimensions periodically and emits
`C2S::ScreenLayoutUpdate` on change. Auto-reconnect with backoff is in place;
TCP_NODELAY is set on both sides.

The reis flow on Wayland:
1. Get an EIS fd from the compositor (via portal or direct socket).
2. `reis::EiClient::from_fd(fd)`.
3. Negotiate Seat + Pointer + Keyboard interfaces.
4. Inject with `pointer.motion_absolute(x, y)`, `keyboard.key(code, state)`, etc.

### Backend traits

- Capture: `wayflow_server::backend::CaptureBackend` -- one method, `start(...)`,
  takes the `InputEvent` channel, a release-grab signal, a `monitors_tx`
  watch channel, and a `Telemetry` handle; runs on a dedicated `std::thread`.
- Inject: `wayflow_client::backend::InjectBackend` -- `move_abs`, `mouse_button`,
  `scroll`, `key_event`, `screen_size`, `refresh_screen_size`. Implementations
  live behind `cfg` gates and are dispatched via `backend::create()`.

### Clipboard

| Platform        | Crate              | Notes                                   |
|-----------------|--------------------|-----------------------------------------|
| Linux/Wayland   | smithay-clipboard  | wraps wl_data_device_manager natively   |
| Linux/X11       | arboard            |                                         |
| macOS           | arboard            |                                         |
| Windows         | arboard            |                                         |

Do NOT spawn `wl-copy` / `wl-paste` / `xclip` / `pbcopy` subprocesses. The whole
point is native clipboard access.

---

## Config

Two files. Both default-pathed under `dirs::config_dir() / "wayflow"`.

`config.toml` (server):
```toml
[server]
name = "helicon"
port = 24800

[[clients]]
name   = "trantor"
edge   = "Right"          # Top | Bottom | Left | Right
offset = 0
# optional per-client modifier remap (HID-name keys, see config.rs)
modifier_map = { ctrl_left = "meta_left", meta_left = "ctrl_left" }
```

`client.toml` (client):
```toml
server = "helicon"
port   = 24800            # optional
```

The CLI `--config <path>` overrides the default. The server hot-reloads
config.toml on save (`server: hot-reload config on file save`). The tray app
exposes `Edit config` which opens the file in `$EDITOR` via `open`.

TLS certs and TOFU pin store live under `dirs::data_local_dir() / "wayflow"`.
Never hardcode paths and never put secrets in plaintext config.

---

## Running it

```bash
# Server (the machine sharing its keyboard + mouse)
WAYFLOW_LOG=debug cargo run -p wayflow-cli -- server

# Client (the machine being controlled)
WAYFLOW_LOG=debug cargo run -p wayflow-cli -- client --server 192.168.1.x

# Tray supervisor (spawns server or client as a child via current_exe())
cargo run -p wayflow-cli -- tray
```

`wayflow tray` requires a status-bar host on Linux: KDE, GNOME with the
AppIndicator extension, or waybar with the `tray` module. On macOS, wrap with
`./scripts/bundle-mac.sh` and `open target/Wayflow.app` to get a proper LSUIElement
menu-bar app (no dock icon).

### Logging

- `WAYFLOW_LOG` (e.g. `wayflow=debug,info`) controls the stderr layer; default `info`.
- `WAYFLOW_LOG_FILE` controls the rotating-JSON file layer; default `wayflow=debug,info`.
- Files land under `dirs::cache_dir() / "wayflow" / {server,client,tray}.log.<date>`,
  daily-rotated via `tracing-appender::rolling::daily`.
- Sending `SIGUSR1` to a running server dumps internal state + telemetry counters
  to the log (`server: SIGUSR1 state dump + telemetry counters + rotating log`).

---

## Testing

### Unit + integration

```bash
cargo test --workspace                # required to pass for any PR
cargo nextest run --workspace         # preferred runner if installed
```

`wayflow-proto` MUST have a round-trip serde test for every wire type and every
enum variant -- see the existing `mod tests` in `proto/src/lib.rs` and follow
the pattern. `wayflow-core` tests cover framing, config round-trip, modifier
remap, and the layout edge math.

### Linux Wayland-to-Wayland (primary dev loop)

```bash
nix develop                                   # gets you libei, weston, deps
weston --socket=wayland-test &                # nested compositor as the "client"
WAYFLOW_LOG=debug cargo run -p wayflow-cli -- server &
WAYLAND_DISPLAY=wayland-test WAYFLOW_LOG=debug \
  cargo run -p wayflow-cli -- client --server 127.0.0.1
```

Move cursor to the configured edge of the server screen. Verify cursor and
keyboard appear inside the weston window and clipboard syncs both ways.

### Cross-platform

CI (`.github/workflows/ci.yml`) runs `cargo test --workspace` on
`ubuntu-latest`, `macos-latest`, and `windows-latest`. Linux installs
`libwayland-dev`. Anything that requires a real compositor, accessibility
permissions, or codesign needs hardware -- guard with `#[cfg(...)]` or
`#[ignore]` so CI stays green.

---

## Build

```bash
nix develop                          # Linux: gets you the toolchain + sys deps
cargo build --workspace
cargo check --workspace              # PR gate: must be warning-free

# macOS / Windows: install Rust via rustup, then cargo build --workspace.
# No system libs required off-Linux; tray-icon ships its own bindings.

# Cross-compile checks (optional, requires `cross`)
cross check --target x86_64-apple-darwin
cross check --target x86_64-pc-windows-gnu

# Release for menu-bar bundle on macOS
cargo build --release
./scripts/bundle-mac.sh && open target/Wayflow.app
```

The Nix `devShell` provides: Rust stable + clippy + rust-analyzer, libei, wayland,
wayland-protocols, weston (for the nested compositor), gtk3 + libayatana-appindicator
(StatusNotifierItem stack consumed by the tray), and `xdotool` (libxdo, transitive
dep of `tray-icon`). `LD_LIBRARY_PATH` is set so `libayatana-appindicator` is
findable at runtime (`tray-icon` dlopens it).

---

## Code conventions

### Error handling
- Library crates (`wayflow-proto`, `wayflow-core`, `wayflow-server`, `wayflow-client`,
  `wayflow-tray`): use `thiserror` for typed errors where they cross a public
  boundary. Inside a crate, `anyhow::Result` is fine for plumbing.
- Binary crate (`wayflow-cli`): `anyhow` end-to-end at entry points.
- Never `unwrap()` in library code. Use `?` and propagate.
- `expect()` only with a documented invariant that is genuinely impossible to violate.

### Async
- `tokio` throughout. No `std::thread::spawn` for I/O-bound work.
- Blocking syscalls / blocking C libs go in `tokio::task::spawn_blocking`.
- Capture backends run on a dedicated `std::thread` (they own a blocking event
  loop) and communicate over `mpsc` / `watch` channels -- this is the one
  documented exception.
- `tokio::sync::Mutex` / `RwLock` for shared async state; `std::sync::Mutex` only
  for short-lived guards that never await.

### Platform gating
- All platform-conditional code lives in `backend/` modules.
- Gate with `#[cfg(target_os = "linux")]` etc. at the module level.
- The `mod.rs` in each backend directory is platform-agnostic and defines the trait.
- Never put platform-specific code in the crate root or `lib.rs`.

### Logging
- `tracing`: `trace!`, `debug!`, `info!`, `warn!`, `error!`.
- No `println!` / `eprintln!` in library code.
- Span context for connections: `tracing::instrument` on connection handlers.
- Log level env vars are `WAYFLOW_LOG` (stderr) and `WAYFLOW_LOG_FILE` (file).

### wayflow-proto rules (hard)
- Compiles `#![no_std]` (uses `extern crate alloc`).
- Zero I/O. Zero platform deps. Zero network calls.
- Every message type / enum variant has a round-trip serde test.
- Any wire-format change bumps `PROTOCOL_VERSION`.

### Style
- ASCII-only in source: no em-dash / curly quotes / unicode arrows. Use `--`,
  `"`, `->`. Code round-trips through grep, terminals, and clipboards more often
  than you think.
- No comments unless the WHY is non-obvious. Don't restate what the code does.
- Commit messages: imperative mood, scoped prefix when natural
  (`server: ...`, `client: ...`, `proto: ...`, `cleanup: ...`). Match the
  existing log -- `git log --oneline -20` for examples.

---

## What NOT to do

- Do not spawn subprocesses for clipboard (`wl-copy`, `wl-paste`, `xclip`,
  `pbcopy`, `pbpaste`). Use the native crates listed above.
- Do not add Qt, GTK, or any C++ GUI framework. The tray uses `tray-icon` + `tao`
  (which links GTK3 on Linux as a system dep -- that is acceptable; do not pull
  in gtk-rs / glib bindings ourselves).
- Do not re-introduce a Tauri or Electron GUI; the tray supervisor replaces it.
- Do not use `std::process::Command` to call system tools for input injection.
- Do not add synchronous blocking I/O on a tokio task. Use `spawn_blocking`.
- Do not put platform-specific code outside of `backend/` modules.
- Do not use `unsafe` without a `// SAFETY:` comment explaining the invariant.
- `wayflow-proto` must stay `no_std`. Do not add `std`-only deps to it.
- Do not add a new dependency without first opening an issue. The deps list in
  `Cargo.toml` is intentionally small.
- Do not change wire types without bumping `PROTOCOL_VERSION`.

---

## Contribution flow (PRs and agentic coding)

1. Open an issue first for anything non-trivial. Architecture is still in flux;
   a PR without prior discussion is likely to be redesigned or rejected.
2. Fork, branch (`git checkout -b feature/foo`).
3. Implement the change. Keep it scoped -- one PR, one concern.
4. Local gates (must all pass):
   - `cargo test --workspace`
   - `cargo check --workspace`  (no warnings)
   - For wire changes: `PROTOCOL_VERSION` bumped + a round-trip test added.
5. Commit messages in the existing imperative style. ASCII only.
6. PR against `master`. CI runs the full test matrix on Linux/macOS/Windows.
7. By submitting, you agree your contribution is licensed GPL-2.0.

Agentic coders specifically: this guide is the single source of truth for what
"green" means on this repo. If you find a rule here that conflicts with what
the code actually does, that is a bug in the doc -- update both this file and
the code in the same PR rather than silently following the code.

---

## Current status snapshot

This rots fast; verify against `git log --oneline` before relying on it.

| Component                              | Status                                       |
|----------------------------------------|----------------------------------------------|
| wayflow-proto (v3 wire types)          | complete; round-trip tested                  |
| wayflow-core transport (TLS + framing) | complete                                     |
| wayflow-core config (server + client)  | complete; hot-reload on save                 |
| wayflow-core layout (multi-monitor)    | complete                                     |
| wayflow-core keymap                    | complete                                     |
| TLS handshake + TOFU cert pinning      | complete                                     |
| wayflow-cli (server/client/tray)       | complete                                     |
| wayflow-tray (supervisor + menu)       | complete                                     |
| Linux/Wayland capture (ashpd portal)   | working; multi-monitor; release-on-disconnect |
| Linux/Wayland inject (reis)            | scaffolded                                   |
| macOS client inject (CGEvent direct)   | working (mouse, keyboard, scroll, clicks)    |
| macOS server capture                   | TODO                                         |
| Windows capture/inject (rdev)          | TODO                                         |
| Clipboard (all platforms)              | protocol-ready; wiring TODO                  |
| X11 backends (server + client)         | TODO                                         |

If you take an open `TODO` row, open an issue first to discuss the approach so
we can align on backend trait shape, telemetry hooks, and test strategy before
a large patch lands.
