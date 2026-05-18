.PHONY: build check test lint fmt clean \
        run-server run-client run-tray \
        dev-server dev-client \
        package

# ── build ──────────────────────────────────────────────────────────────────────

build:
	cargo build --workspace

build-release:
	cargo build --workspace --release

# ── checks ─────────────────────────────────────────────────────────────────────

check:
	cargo check --workspace

lint:
	cargo clippy --workspace -- -D warnings

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

test:
	cargo test --workspace

# ── run ────────────────────────────────────────────────────────────────────────

run-server:
	WAYFLOW_LOG=debug cargo run -p wayflow-cli -- server

run-client:
	WAYFLOW_LOG=debug cargo run -p wayflow-cli -- client --server $(SERVER)

run-tray:
	WAYFLOW_LOG=debug cargo run -p wayflow-cli -- tray

# ── dev (cargo-watch) ──────────────────────────────────────────────────────────

dev-server:
	cargo watch -w crates/wayflow-server -w crates/wayflow-core -w crates/wayflow-proto \
		-x 'run -p wayflow-cli -- server'

dev-client:
	cargo watch -w crates/wayflow-client -w crates/wayflow-core -w crates/wayflow-proto \
		-x 'run -p wayflow-cli -- client --server $(SERVER)'

# ── wayland dev loop (nested weston compositor) ────────────────────────────────

dev-wayland-server:
	WAYFLOW_LOG=debug cargo run -p wayflow-cli -- server

dev-wayland-client:
	WAYLAND_DISPLAY=wayland-test WAYFLOW_LOG=debug cargo run -p wayflow-cli -- client --server 127.0.0.1

weston-start:
	weston --socket=wayland-test &

# ── packaging (cargo-packager) ─────────────────────────────────────────────────

package:
	cargo packager --release

# ── misc ───────────────────────────────────────────────────────────────────────

clean:
	cargo clean
