.DEFAULT_GOAL := help

MAC        ?= mac
MAC_DIR    ?= code/wayflow
PROBE_PORT ?= 47821

.PHONY: help test lint fmt tunnel tunnel-status tunnel-down forward unforward \
        sync mac-build mac-test host client-build

help: ## Show available targets
	@grep -hE '^[a-z-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk -F':.*?## ' '{printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2}'

## --- build ------------------------------------------------------------------

test: ## Run the workspace test suite
	cargo test --workspace

lint: ## Clippy across all targets, warnings are errors
	cargo clippy --workspace --all-targets -- -D warnings

fmt: ## Format the workspace
	cargo fmt --all

## --- remote -----------------------------------------------------------------
#
# The master connection is opened once and reused. Every later `ssh $(MAC)` rides
# it, which cuts roughly 130ms of handshake off each command in a build/deploy/tail
# loop. If the master dies, `ControlMaster auto` silently builds a new one.

tunnel: ## Open the persistent SSH master to the mac
	@ssh -O check $(MAC) 2>/dev/null || ssh -N -f $(MAC)
	@ssh -O check $(MAC)

tunnel-status: ## Is the master alive?
	@ssh -O check $(MAC) || echo "no master; run 'make tunnel'"

tunnel-down: ## Close the master and every forward riding it
	@ssh -O exit $(MAC) 2>/dev/null || echo "no master to close"

# The probe binds 127.0.0.1 on the remote box and is reached only through here, so
# the captured keystroke stream is never exposed on the LAN in any form.
forward: tunnel ## Forward localhost:$(PROBE_PORT) to the mac's probe
	@ssh -O forward -L $(PROBE_PORT):127.0.0.1:$(PROBE_PORT) $(MAC) \
		&& echo "localhost:$(PROBE_PORT) -> $(MAC):127.0.0.1:$(PROBE_PORT)"

unforward: ## Drop the probe forward, leaving the master up
	@ssh -O cancel -L $(PROBE_PORT):127.0.0.1:$(PROBE_PORT) $(MAC) \
		&& echo "forward cancelled"

## --- sync -------------------------------------------------------------------
#
# One direction only: terminus is the source of truth and the mac is a build target.
# Bidirectional sync would need a daemon on the mac, which its endpoint protection
# treats as persistence, so this stays a push you can reason about.
#
# macOS ships openrsync (protocol 29), not GNU rsync 3.x, so the flags here stay
# conservative: no --filter, no --info. `target/` is excluded because build output is
# platform-specific and syncing it would clobber the native one.

sync: ## Push the workspace to the mac
	@rsync -az --delete \
		--exclude='target/' --exclude='.git/' --exclude='*.swp' \
		./ $(MAC):$(MAC_DIR)/
	@echo "synced -> $(MAC):$(MAC_DIR)"

mac-build: sync ## Sync, then build on the mac
	@ssh $(MAC) 'cd $(MAC_DIR) && cargo build'

mac-test: sync ## Sync, then run the test suite on the mac
	@ssh $(MAC) 'cd $(MAC_DIR) && cargo test --workspace'

## --- run --------------------------------------------------------------------

host: ## Watch the edge and forward input to the client (Ctrl+Esc force-releases)
	@cargo build -q -p wayflow-host && ./target/debug/wayflow-host edge $(PROBE_PORT)

client-build: sync ## Build the client on the mac, ready to launch there
	@ssh $(MAC) 'cd $(MAC_DIR) && cargo build -q -p wayflow-client && echo built'
	@echo "now run this in a mac GUI terminal (not over ssh -- TCC cannot prompt there):"
	@echo "  cd $(MAC_DIR) && ./target/debug/wayflow-client"
