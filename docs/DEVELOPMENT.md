# Development Guide

This guide covers the repository workflow. The current checkout is part of a
build-repair and hardening pass, so commands and feature descriptions below do
not imply that the workspace currently compiles.

## Prerequisites

- Rust and `rustup` (toolchain version pinned in `rust-toolchain.toml`)
- Stellar CLI `23.4.1` — must match `STELLAR_CLI_VERSION` in `scripts/common.sh`
- Node.js 20 or newer for `oracle/`

The correct WASM target (`wasm32v1-none`) and Rust toolchain channel are declared
in `rust-toolchain.toml` and picked up automatically by `rustup`.

## Local Checks

Run `make ci` before pushing. It executes every check the CI workflow runs, in
the same order, so a local green build means CI will pass:

```bash
make ci
```

This single command covers: orphan-module check, formatting, compile check,
contract build, WASM size gate, docs sync, clippy, tests, doc tests, shellcheck,
and the full oracle pipeline (format, lint, typecheck, tests).

For faster, focused iteration during development:

```bash
make fmt          # cargo fmt check only
make lint         # fmt + clippy
make test         # cargo test --workspace
make doc-test     # cargo test --workspace --doc
make build        # stellar contract build
make wasm-sizes   # WASM size gate
make docs-sync    # regenerate + diff ERRORS.md and EVENTS.md
make shellcheck   # shellcheck scripts/*.sh
make oracle-ci    # full oracle pipeline
```

## Build Targets

Build all contracts through the Stellar CLI (same target the deploy scripts use):

```bash
make build   # stellar contract build → wasm32v1-none
```

To build individual packages:

```bash
stellar contract build -p raffle-factory
stellar contract build -p raffle-instance
```

Deployment and verification instructions are maintained in
[DEPLOYMENT.md](DEPLOYMENT.md). Storage tiers and TTL policy are maintained in
[STORAGE.md](STORAGE.md).

## Contribution Conventions

Use a descriptive branch prefix such as `feat/`, `fix/`, `docs/`, `test/`, or
`chore/`. Keep pull requests focused, document externally visible changes, and
update the relevant document in this directory rather than adding a temporary
root-level status or plan file.

See the root [CONTRIBUTING.md](../CONTRIBUTING.md) for the contribution and
review process.
