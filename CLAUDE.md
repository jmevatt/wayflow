# wayflow — Claude Code guide

Wayland-native keyboard/mouse sharing across machines. Rust, async (tokio), TLS transport (postcard wire protocol).

---

## Workspace layout

```
crates/
  wayflow-proto/    wire types only — no_std compatible, no logic
  wayflow-core/     TLS transport, config, HID keymap
  wayflow-server/   input capture backends + server loop
  wayflow-client/   input injection backends + client loop
  wayflow-tray/     system-tray supervisor (tao + tray-icon, no GUI framework)
apps/
  wayflow-cli/      single `wayflow` binary — subcommands: server / client / tray
packaging/          systemd units, .desktop, macOS Info.plist
```

---

## Common commands

| Task | Command |
|------|---------|
| Build | `make build` |
| Check (no warnings) | `make check` |
| Test (must be 100% pass) | `make test` |
| Clippy | `make lint` |
| Format | `make fmt` |
| Run server | `make run-server` |
| Run client | `make run-client SERVER=192.168.1.x` |
| Hot-reload server | `make dev-server` |
| Hot-reload client | `make dev-client SERVER=192.168.1.x` |
| Package installers | `make package` |

---

## Dev loop — Linux Wayland (nested compositor)

```bash
make weston-start          # starts weston on wayland-test socket
make dev-wayland-server    # server in this shell
make dev-wayland-client    # client in another shell (wayland-test display)
```

---

## Hard rules (from CONTRIBUTING.md)

- **Tests:** `cargo test --workspace` must be 100% clean. No new code without covering tests.
- **Warnings:** zero. `make check` and `make lint` must be clean.
- **Platform-specific code** (`#[cfg(target_os = ...)]`) lives only in `backend/` modules — never in protocol/trait layers.
- **Wire protocol changes** in `wayflow-proto` require a `PROTOCOL_VERSION` bump.
- **No new deps** without opening an issue first. Prefer pure Rust; avoid C libs.

---

## Code conventions

- No comments unless the WHY is genuinely non-obvious to a Rust reader.
- No `unwrap()` / `expect()` in library crates — propagate with `?`.
- No `println!` / `eprintln!` — use `tracing::{debug,info,warn,error}`.
- No blocking I/O on async task threads — use `tokio::task::spawn_blocking`.
- `unsafe` requires a `// SAFETY:` comment documenting the invariant.
- Commit messages: imperative mood, concise (`add`, `fix`, `refactor`).

---

## Wire protocol

- All messages: postcard-serialized, 4-byte little-endian length prefix, over TLS.
- Keycodes on the wire: USB HID usage codes (keyboard page `0x07`).
- Each platform translates to/from its native representation in the backend layer.

---

## Infrastructure

- This machine is **helicon** (wayflow server, Linux/Wayland, CachyOS)
- System packages managed via `pacman`/`paru` — dev deps already installed (see `flake.nix` for the canonical list)
- Build tooling: `sccache` (compiler cache) + `mold` (linker) configured in `~/.cargo/config.toml`

---

## Logging

Set `WAYFLOW_LOG=debug` (or `trace`, `info`, `warn`, `error`) to control log level.
Structured JSON logs are emitted when `WAYFLOW_LOG_JSON=1` is set.

---

## Platform status (as of project start)

| Feature | Linux (Wayland) | macOS | Windows |
|---------|:-:|:-:|:-:|
| Client inject (mouse + keyboard) | scaffolded | working | stub |
| Server capture | working | TODO | TODO |
| Clipboard sync | text/images | text/images | — |
| TLS transport + handshake | complete | complete | complete |
