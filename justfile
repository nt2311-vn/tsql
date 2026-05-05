set shell := ["bash", "-uc"]

# `docker compose` (v2 plugin) and `docker-compose` (v1 standalone) take the
# same flags for our use, so pick whichever is available on PATH.
docker_compose := `if command -v docker-compose >/dev/null 2>&1; then echo docker-compose; else echo "docker compose"; fi`

# Workspace version is read from the root Cargo.toml. Used by release recipes
# to derive tag names like `v0.1.0`.
version := `awk -F'"' '/^version = "/ {print $2; exit}' Cargo.toml`

# Show this help (the default recipe). Lists every recipe with its docstring.
default:
    @just --list

# ─── Setup ────────────────────────────────────────────────────────────────────

# Install rustfmt, clippy, and cargo-audit (everything the other recipes assume).
setup:
    rustup component add rustfmt clippy
    cargo install cargo-audit --locked

# ─── Format and lint ──────────────────────────────────────────────────────────

# Auto-format every crate in the workspace with rustfmt.
fmt:
    cargo fmt --all

# Verify formatting without modifying files (mirrors the CI fmt job).
fmt-check:
    cargo fmt --all -- --check

# Run clippy across the workspace with -D warnings (every lint = build error).
lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# ─── Tests ────────────────────────────────────────────────────────────────────

# Run unit + SQLite integration tests (Postgres is gated, see test-integration).
test:
    cargo test --workspace --all-features

# Run the Postgres integration tests against the dockerized DB (run `just up` first).
test-integration:
    TSQL_TEST_POSTGRES_URL=postgres://tsql:tsql@127.0.0.1:54329/tsql cargo test -p tsql-db --test postgres -- --ignored

# Smoke test: run the example SQL script against in-memory SQLite via the CLI.
smoke-sqlite:
    cargo run -p tsql -- exec --url sqlite::memory: --file examples/query.sql

# Smoke test the Postgres metadata fetchers. Requires `just up` first.
smoke-metadata:
    TSQL_TEST_POSTGRES_URL=postgres://tsql:tsql@127.0.0.1:54329/tsql cargo test -p tsql-db --test metadata -- --ignored

# ─── Docker compose for tests ─────────────────────────────────────────────────

# Start the dockerized Postgres on localhost:54329 and wait for health-check OK.
up:
    {{docker_compose}} up -d --wait

# Stop the dockerized Postgres service and remove any orphan containers.
down:
    {{docker_compose}} down --remove-orphans

# ─── Security ─────────────────────────────────────────────────────────────────

# Audit dependencies against the RustSec advisory database.
audit:
    cargo audit

# Run cargo audit and remind that TruffleHog/Gitleaks/Semgrep/Trivy live in CI.
security: audit
    @echo "Run GitHub security workflow for TruffleHog, Gitleaks, Semgrep, Trivy, and optional Snyk."

# ─── CI gates ─────────────────────────────────────────────────────────────────

# Run the full GitHub `ci` workflow locally: fmt-check, lint, test, audit.
ci: fmt-check lint test audit

# `just ci` plus the Postgres integration tests; needs `just up` beforehand.
ci-full: ci test-integration

# ─── Release build ────────────────────────────────────────────────────────────
# These recipes are about building the optimized `tsql` binary locally;
# nothing here touches git tags, crates.io, or GitHub. The CI publish
# workflow lives in `.github/workflows/release.yml` and is unrelated.

# Print the workspace version that release-* recipes embed in the binary.
release-version:
    @echo "tsql {{version}}"

# Pre-flight gates (fmt + clippy + tests + audit + smoke) before a release build.
release-check: fmt-check lint test audit smoke-sqlite

# Build the optimized release binary into `target/release/tsql`.
release-build:
    cargo build --release -p tsql

# Print release-binary status: existence, size, mtime, SHA-256, --version output.
release-status:
    @if [ ! -x target/release/tsql ]; then \
        echo "no release binary yet  (run \`just release-build\`)"; \
        exit 0; \
    fi
    @echo "── target/release/tsql ──"
    @ls -lh target/release/tsql | awk '{print "size  :", $5; print "mtime :", $6, $7, $8}'
    @printf "sha256: " && sha256sum target/release/tsql | awk '{print $1}'
    @printf "tsql  : " && target/release/tsql --version 2>/dev/null || echo "(binary did not respond to --version)"

# Build the release binary and print its status in one step (build + verify).
release: release-build release-status

# Build, then run the release binary; pass args after the recipe (default --help).
release-run *args="--help": release-build
    ./target/release/tsql {{args}}

# Install the release binary into ~/.cargo/bin/tsql (so `tsql` is on your PATH).
release-install:
    cargo install --path crates/tsql-app --force

# Remove the release build artifacts (keeps the debug target dir intact).
release-clean:
    rm -rf target/release
