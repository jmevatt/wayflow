# wayflow

Wayland-native keyboard and mouse sharing over the network. One keyboard and mouse, multiple machines -- Linux, macOS, and Windows.

Think Synergy/Barrier/Deskflow, rewritten from scratch in Rust with a clean async architecture and no Qt.

> **Status:** Early development. macOS client injection works. Server-side input capture is in progress. Not yet ready for daily use.

---

## Why another one?

Deskflow (the Synergy fork) carries 20 years of X11 assumptions, uses wl-copy subprocesses for clipboard, and its Qt threading constraints cause crashes on Wayland. Wayflow starts clean:

- Wayland-native: libei/EIS for injection, XDG RemoteDesktop portal for capture
- No subprocesses for clipboard -- native Wayland/macOS/Windows clipboard APIs
- Async throughout: tokio, no blocking I/O on event threads
- Clean wire protocol: postcard over TLS, not the legacy Synergy format

---

## Platform status

| Feature | Linux (Wayland) | macOS | Windows |
|---------|:-:|:-:|:-:|
| Client inject (mouse + keyboard) | stub | **working** | stub |
| Server capture | stub | stub | stub |
| Clipboard sync | text/images | text/images | - |
| TLS transport + handshake | complete | complete | complete |

---

## Building

### Prerequisites

**Linux (NixOS / nix):**
```
nix develop
cargo build --workspace
```

**macOS / Windows:**
```
# Install Rust: https://rustup.rs
cargo build --workspace
```

No system libraries required on macOS or Windows -- all deps are pure Rust or bundled.

On Linux you need `libwayland-dev` (or the Nix `wayland` package).

### Run

```
# On the machine with the keyboard and mouse (server mode):
WAYFLOW_LOG=debug wayflow server

# On the machine to be controlled (client mode):
WAYFLOW_LOG=debug wayflow client --server 192.168.1.x

# Tray UI: menu-bar/system-tray icon that supervises a wayflow server or client.
# Single binary, same on Linux/macOS/Windows.
wayflow tray
```

On macOS, the client will prompt for Accessibility permission the first time it injects input. Grant it in System Settings -> Privacy & Security -> Accessibility.

The tray app is a tiny supervisor: pick `Start server` or `Start client` from the menu, and it spawns the right `wayflow` subcommand as a child. Status, edit-config-in-`$EDITOR`, and quit also live in the menu. To make a proper menu-bar app on macOS (no dock icon), wrap the binary with `./scripts/bundle-mac.sh` and `open target/Wayflow.app`. On Linux you need a status bar that hosts StatusNotifierItem (KDE/GNOME-with-AppIndicator/waybar with `tray` module).

---

## Configuration

Wayflow looks for a config file at the platform config dir (e.g. `~/.config/wayflow/config.toml` on Linux, `~/Library/Application Support/wayflow/config.toml` on macOS). If no file exists, it uses defaults.

```toml
[server]
name = "helicon"       # hostname of the server machine
port = 24800           # default

[[clients]]
name   = "trantor"     # hostname the client sends in its Hello
edge   = "Right"       # trantor is to the right of helicon
offset = 0             # pixel offset along the perpendicular axis
```

`edge` can be `Top`, `Bottom`, `Left`, or `Right`.

---

## Architecture

```
crates/
  wayflow-proto/    -- wire types only, no_std compatible
  wayflow-core/     -- TLS transport, config, HID keymap
  wayflow-server/   -- input capture backends + server loop
  wayflow-client/   -- input injection backends + client loop
apps/
  wayflow-cli/      -- single binary: `wayflow server` / `wayflow client`
```

All messages are postcard-serialized with a 4-byte little-endian length prefix over a TLS connection. Keycodes on the wire are USB HID usage codes (keyboard page 0x07) -- each platform translates to/from its native representation.

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

---

## Security

To report a vulnerability, see [SECURITY.md](SECURITY.md).

---

## License

GPL-2.0. See [LICENSE](LICENSE).
