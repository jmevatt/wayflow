# Contributing to wayflow

Wayflow is an early-stage project. Contributions are welcome, but read this first.

---

## Before you start

Open an issue before writing code for anything non-trivial. The architecture is still
being defined and a PR without prior discussion is likely to be redesigned or rejected.

For small fixes (typos, test coverage gaps, obvious bugs) just send the PR.

---

## Requirements for every PR

**1. Tests must pass at 100%**
```
cargo test --workspace
```
No new code without covering tests. No reduction in test coverage. This is a hard requirement.

**2. No warnings**
```
cargo check --workspace
```
The build must be warning-free.

**3. Platform-specific code stays in `backend/`**
All `#[cfg(target_os = ...)]` code belongs in the relevant `backend/` module. The
trait definitions and protocol logic in `mod.rs`, `server.rs`, `client.rs` etc. must
be platform-agnostic.

**4. No new dependencies without discussion**
Dependency additions require an issue explaining why an existing dep can't cover the need.
Avoid adding C library dependencies -- prefer pure Rust crates where they exist and are
maintained.

**5. Wire protocol changes need a version bump**
Any change to `wayflow-proto` that alters serialized output must bump `PROTOCOL_VERSION`
in that crate.

---

## Code style

- No comments unless the WHY is genuinely non-obvious to a reader who knows Rust.
- No `unwrap()` or `expect()` in library crates. Use `?` and propagate.
- No `println!` / `eprintln!` -- use `tracing::{debug,info,warn,error}`.
- No blocking I/O on async task threads. Use `tokio::task::spawn_blocking`.
- No `unsafe` without a `// SAFETY:` comment documenting the invariant.
- Commit messages: imperative mood, concise (`add`, `fix`, `refactor` -- not `added`).

---

## Running tests locally

```
cargo test --workspace
```

For the Linux Wayland-to-Wayland dev loop (nested compositor):
```
nix develop
weston --socket=wayland-test &
WAYFLOW_LOG=debug cargo run -p wayflow-cli -- server &
WAYLAND_DISPLAY=wayland-test WAYFLOW_LOG=debug cargo run -p wayflow-cli -- client --server 127.0.0.1
```

---

## Submitting

1. Fork the repo
2. Create a branch: `git checkout -b my-feature`
3. Write code + tests
4. `cargo test --workspace` -- must be clean
5. Open a PR against `master`

---

## License

By submitting a PR, you agree that your contribution is licensed under GPL-2.0.
