.PHONY: build test lint fuzz fuzz-check clean deploy-testnet deploy-mainnet verify reproducible oracle-build oracle-test all ci

# `stellar contract build` targets wasm32v1-none — the same target the deploy
# scripts and CI use. Keep every build path going through it (issue #841); the
# artifact paths are defined once in scripts/common.sh.
build:
	stellar contract build

test:
	cargo test --workspace

lint:
	cargo fmt --all -- --check
	cargo clippy --all-targets --all-features -- -D warnings

FUZZ_TARGETS := fuzz_buy_ticket fuzz_finalize_raffle fuzz_winner_selection fuzz_refund_cancel fuzz_commit_reveal
FUZZ_TIME ?= 300

fuzz:
	@for target in $(FUZZ_TARGETS); do \
		echo "==> fuzzing $target (${FUZZ_TIME}s)"; \
		cargo fuzz run $target -- -max_total_time=$(FUZZ_TIME); \
	done

fuzz-check:
	cd fuzz && cargo fuzz build

deploy-testnet:
	./scripts/deploy-testnet.sh

deploy-mainnet:
	./scripts/deploy-mainnet.sh

verify:
	./scripts/verify.sh

reproducible:
	./scripts/build-reproducible.sh

clean:
	cargo clean

oracle-build:
	cd oracle && npm ci && npm run build

oracle-test:
	cd oracle && npm test

oracle-lint:
	cd oracle && npm run lint

all: lint test build

# Reproduce the full CI job list locally. Mirrors .github/workflows/ci.yml
# build_and_test + oracle_check jobs. Run this before pushing to catch any
# CI failure without waiting for a remote run.
#
# Intentionally excludes the fuzz-targets compile check (fuzz-check) and the
# coverage ratchet — those are slow and run on their own schedules in CI.
ci:
	python3 scripts/check_orphan_modules.py
	cargo check --workspace --all-targets
	cargo fmt --all -- --check
	cargo clippy --all-targets --all-features -- -D warnings
	stellar contract build
	python3 scripts/check_wasm_sizes.py
	python3 scripts/generate_error_docs.py
	git diff --exit-code docs/ERRORS.md
	python3 scripts/generate_event_docs.py
	git diff --exit-code docs/EVENTS.md
	cargo test --workspace
	cargo test --workspace --doc
	cd oracle && npm ci && npm run format:check && npm run lint && npm run typecheck && npm run test:ci
