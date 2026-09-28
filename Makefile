.PHONY: build test lint fuzz clean deploy-testnet deploy-mainnet verify reproducible oracle-build oracle-test all

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

# Derive fuzz targets from fuzz/Cargo.toml so this list never drifts from what
# is actually declared.  The shell snippet greps every [[bin]] name = "…" line,
# strips the surrounding quotes and whitespace, then joins the results.
FUZZ_TARGETS := $(shell grep -A1 '^\[\[bin\]\]' fuzz/Cargo.toml \
                  | grep 'name\s*=' \
                  | sed 's/.*name\s*=\s*"\([^"]*\)".*/\1/')
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

all: lint test build
