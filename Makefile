.PHONY: build test lint fmt check orphan-check wasm-sizes docs-sync \
        doc-test shellcheck python-lint python-fmt-check python-test \
        fuzz clean deploy-testnet deploy-mainnet verify reproducible \
        oracle-build oracle-test oracle-lint oracle-fmt oracle-typecheck oracle-ci \
        all ci

# `stellar contract build` targets wasm32v1-none — the same target the deploy
# scripts and CI use. Keep every build path going through it (issue #841); the
# artifact paths are defined once in scripts/common.sh.
build:
	stellar contract build

check:
	cargo check --workspace --all-targets --all-features --exclude raffle-fuzz

fmt:
	cargo fmt --all -- --check

orphan-check:
	python3 scripts/check_orphan_modules.py

wasm-sizes:
	python3 scripts/check_wasm_sizes.py

docs-sync:
	python3 scripts/generate_error_docs.py
	git diff --exit-code docs/ERRORS.md || \
	  (echo "ERROR: docs/ERRORS.md is out of sync. Run 'python3 scripts/generate_error_docs.py' to update." && exit 1)
	python3 scripts/generate_event_docs.py
	git diff --exit-code docs/EVENTS.md || \
	  (echo "ERROR: docs/EVENTS.md is out of sync. Run 'python3 scripts/generate_event_docs.py' to update." && exit 1)

test:
	cargo test --workspace

doc-test:
	cargo test --workspace --doc

lint:
	cargo fmt --all -- --check
	cargo clippy --all-targets --all-features -- -D warnings

shellcheck:
	shellcheck scripts/*.sh

python-lint:
	ruff check scripts/

python-fmt-check:
	ruff format --check scripts/

python-test:
	python3 -m pytest scripts/tests/ -v

FUZZ_TARGETS := fuzz_buy_ticket fuzz_finalize_raffle fuzz_winner_selection fuzz_refund_cancel fuzz_commit_reveal
FUZZ_TIME ?= 300

fuzz:
	@for target in $(FUZZ_TARGETS); do \
		echo "==> fuzzing $$target ($${FUZZ_TIME}s)"; \
		cargo fuzz run $$target -- -max_total_time=$(FUZZ_TIME); \
	done

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

oracle-fmt:
	cd oracle && npm run format:check

oracle-typecheck:
	cd oracle && npm run typecheck

oracle-ci: oracle-fmt oracle-lint oracle-typecheck
	cd oracle && npm run test:ci

# Runs every check the CI workflow runs, in the same order.
# A new step in ci.yml must have a matching target here, and vice-versa.
ci: orphan-check fmt check build wasm-sizes docs-sync \
    lint test doc-test shellcheck \
    python-lint python-fmt-check python-test \
    oracle-ci

all: lint test build
