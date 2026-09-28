.PHONY: build check test test-doc lint fmt-check clippy orphan-check wasm-size \
        docs-errors docs-events shellcheck \
        ruff-lint ruff-fmt-check python-tests \
        fuzz clean deploy-testnet deploy-mainnet verify reproducible \
        oracle-build oracle-test oracle-test-ci oracle-lint oracle-fmt-check oracle-typecheck \
        all ci

# ---------------------------------------------------------------------------
# Rust / contract targets
# ---------------------------------------------------------------------------

# `stellar contract build` targets wasm32v1-none — the same target the deploy
# scripts and CI use. Keep every build path going through it (issue #841); the
# artifact paths are defined once in scripts/common.sh.
build:
	stellar contract build

# Fast compile gate (mirrors the `check` CI job).
check:
	cargo check --workspace --all-targets --all-features --exclude raffle-fuzz

# Run all workspace unit + integration tests.
test:
	cargo test --workspace

# Run workspace doc-tests separately (cargo test --workspace does not run them).
test-doc:
	cargo test --workspace --doc

# fmt-check and clippy are split so each can be called independently; `lint`
# keeps the original convenience alias that runs both together.
fmt-check:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all-targets --all-features -- -D warnings

lint: fmt-check clippy

# ---------------------------------------------------------------------------
# Python / doc-sync targets
# ---------------------------------------------------------------------------

orphan-check:
	python3 scripts/check_orphan_modules.py

wasm-size:
	python3 scripts/check_wasm_sizes.py

# Regenerate docs and fail if the committed file is out of sync.
docs-errors:
	python3 scripts/generate_error_docs.py
	git diff --exit-code docs/ERRORS.md

docs-events:
	python3 scripts/generate_event_docs.py
	git diff --exit-code docs/EVENTS.md

# ---------------------------------------------------------------------------
# Shell script checks
# ---------------------------------------------------------------------------

shellcheck:
	shellcheck scripts/*.sh

# ---------------------------------------------------------------------------
# Python lint and tests
# ---------------------------------------------------------------------------

# Lint and style-check all Python scripts with ruff (config in pyproject.toml).
ruff-lint:
	ruff check scripts/

ruff-fmt-check:
	ruff format --check scripts/

# Run the scripts unit-test suite.
python-tests:
	python3 -m pytest scripts/tests/ -v

# ---------------------------------------------------------------------------
# Oracle targets
# ---------------------------------------------------------------------------

oracle-build:
	cd oracle && npm ci && npm run build

oracle-test:
	cd oracle && npm test

# CI uses `test:ci` (adds --ci flag + coverage); keep oracle-test for local use.
oracle-test-ci:
	cd oracle && npm run test:ci

oracle-lint:
	cd oracle && npm run lint

oracle-fmt-check:
	cd oracle && npm run format:check

oracle-typecheck:
	cd oracle && npm run typecheck

# ---------------------------------------------------------------------------
# Fuzzing
# ---------------------------------------------------------------------------

FUZZ_TARGETS := fuzz_buy_ticket fuzz_finalize_raffle fuzz_winner_selection fuzz_refund_cancel fuzz_commit_reveal
FUZZ_TIME ?= 300

fuzz:
	@for target in $(FUZZ_TARGETS); do \
		echo "==> fuzzing $$target ($${FUZZ_TIME}s)"; \
		cargo fuzz run $$target -- -max_total_time=$(FUZZ_TIME); \
	done

# ---------------------------------------------------------------------------
# Deploy / utility
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# Convenience aliases
# ---------------------------------------------------------------------------

# Original convenience target — kept for existing muscle memory.
all: lint test build

# ---------------------------------------------------------------------------
# ci — mirrors .github/workflows/ci.yml build_and_test + shellcheck +
#       oracle_check jobs exactly and in the same order.
#
# Run this before every push to catch CI failures locally.
#
# Steps omitted here because they cannot run without CI infrastructure:
#   - Rust/Node cache warm-up       (Swatinem/rust-cache, actions/setup-node)
#   - WASM size PR summary          (writes to $GITHUB_STEP_SUMMARY, PR-only)
#   - Coverage ratchet              (needs cargo-llvm-cov; run separately)
#   - Upload artifact steps
# ---------------------------------------------------------------------------
ci: check fmt-check orphan-check build wasm-size docs-errors docs-events \
    clippy test test-doc shellcheck \
    ruff-lint ruff-fmt-check python-tests \
    oracle-fmt-check oracle-lint oracle-typecheck oracle-test-ci
