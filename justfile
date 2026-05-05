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

# ─── Release ──────────────────────────────────────────────────────────────────

# Print the workspace version that the release recipes will use.
release-version:
    @echo "tsql {{version}}"

# Run every gate the release workflow runs PLUS a dry-run cargo package.
release-check: fmt-check lint test audit smoke-sqlite
    cargo package --workspace --allow-dirty --no-verify

# Build optimized release binaries into `target/release/` (does NOT publish).
release-build:
    cargo build --workspace --release

# Build the release binary then print its size and SHA-256 for verification.
release-binary: release-build
    @ls -lh target/release/tsql
    @sha256sum target/release/tsql

# Create and push the v{{version}} git tag, which is what triggers crates.io publish.
release-tag: release-check
    @if git rev-parse --verify --quiet "refs/tags/v{{version}}" >/dev/null; then \
        echo "tag v{{version}} already exists; bump the workspace version first"; \
        exit 1; \
    fi
    git tag -a "v{{version}}" -m "tsql v{{version}}"
    git push origin "v{{version}}"

# Show the last 5 release-workflow runs (newest first); needs `gh` authenticated.
release-status:
    @gh run list --workflow=release.yml --limit=5

# Tail the live logs of the most recent release-workflow run (Ctrl+C to detach).
release-watch:
    @gh run watch $(gh run list --workflow=release.yml --limit=1 --json databaseId --jq '.[0].databaseId')

# Trigger the release workflow without a tag (publish step is skipped, gates run).
release-dispatch:
    @gh workflow run release.yml

# Look up the latest published version of each workspace crate on crates.io.
release-published:
    @for crate in tsql-core tsql-sql tsql-db tsql-tui tsql; do \
        echo "── $crate ──"; \
        cargo search "$crate" --limit 1 || true; \
    done
