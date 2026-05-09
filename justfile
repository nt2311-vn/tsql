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
    TSQLX_TEST_POSTGRES_URL=postgres://tsqlx:tsqlx@127.0.0.1:54329/tsqlx cargo test -p tsqlx-db --test postgres -- --ignored

# Smoke test: run the example SQL script against in-memory SQLite via the CLI.
smoke-sqlite:
    cargo run -p tsqlx -- exec --url sqlite::memory: --file examples/query.sql

# Smoke test the Postgres metadata fetchers. Requires `just up` first.
smoke-metadata:
    TSQLX_TEST_POSTGRES_URL=postgres://tsqlx:tsqlx@127.0.0.1:54329/tsqlx cargo test -p tsqlx-db --test metadata -- --ignored

# ─── Driver sandboxes ─────────────────────────────────────────────────────────
# Per-driver up/down recipes spin up a seeded sandbox so you can poke at tsqlx
# against any supported driver without remembering compose flags. The seed
# scripts in seed/ are portable, so every driver gets the same ERP dataset.

# Start the dockerized Postgres (localhost:54329) seeded from seed/.
postgres-up:
    {{docker_compose}} up -d --wait postgres
    @echo "postgres ready — try: tsqlx tui --url postgres://tsqlx:tsqlx@127.0.0.1:54329/tsqlx"

# Stop the dockerized Postgres and remove orphan containers (keeps the volume).
postgres-down:
    {{docker_compose}} down --remove-orphans

# Wipe the Postgres volume so the next `postgres-up` re-runs the seed scripts.
postgres-reseed:
    {{docker_compose}} down --volumes --remove-orphans
    just postgres-up

# Create + seed a local SQLite file (default: ./erp.db). Idempotent.
sqlite-up db="erp.db":
    rm -f {{db}}
    sqlite3 {{db}} < seed/01_schema.sql
    sqlite3 {{db}} < seed/02_data.sql
    @echo "sqlite ready — try: tsqlx tui --url sqlite:./{{db}}"

# Remove the local SQLite sandbox file (default: ./erp.db).
sqlite-down db="erp.db":
    rm -f {{db}}
    @echo "removed {{db}}"

# Start the dockerized MySQL (localhost:33069) seeded from seed/mysql/.
mysql-up:
    {{docker_compose}} up -d --wait mysql
    @echo "mysql ready — try: tsqlx tui --url mysql://tsqlx:tsqlx@127.0.0.1:33069/tsqlx"

# Stop just the MySQL container; leaves Postgres / MariaDB untouched.
mysql-down:
    {{docker_compose}} stop mysql
    {{docker_compose}} rm -f mysql

# Wipe the MySQL data so the next `mysql-up` re-runs the seed scripts.
mysql-reseed:
    {{docker_compose}} down --volumes --remove-orphans mysql
    just mysql-up

# Start the dockerized MariaDB (localhost:33079) using the same seed.
mariadb-up:
    {{docker_compose}} up -d --wait mariadb
    @echo "mariadb ready — try: tsqlx tui --url mariadb://tsqlx:tsqlx@127.0.0.1:33079/tsqlx"

# Stop just the MariaDB container.
mariadb-down:
    {{docker_compose}} stop mariadb
    {{docker_compose}} rm -f mariadb

# Bring up every driver sandbox at once.
drivers-up: postgres-up sqlite-up mysql-up mariadb-up

# Tear down every driver sandbox at once.
drivers-down: postgres-down sqlite-down mysql-down mariadb-down

# Backward-compatible aliases for the original Postgres-only recipes.
alias up := postgres-up
alias down := postgres-down
alias reseed := postgres-reseed
alias seed-sqlite := sqlite-up

# ─── Security ─────────────────────────────────────────────────────────────────

# Audit dependencies against the RustSec advisory database.
#
# RUSTSEC-2023-0071 (RSA timing sidechannel) has no upstream fix and
# only reaches us transitively through `sqlx-mysql`, which we don't
# enable. Ignore it explicitly so CI stays green; revisit when sqlx
# 0.9 publishes a patched MySQL driver.
audit:
    cargo audit --ignore RUSTSEC-2023-0071

# Run cargo audit and remind that TruffleHog/Gitleaks/Semgrep/Trivy live in CI.
security: audit
    @echo "Run GitHub security workflow for TruffleHog, Gitleaks, Semgrep, Trivy, and optional Snyk."

# ─── CI gates ─────────────────────────────────────────────────────────────────

# Run the full GitHub `ci` workflow locally: fmt-check, lint, test, audit.
ci: fmt-check lint test audit

# `just ci` plus the Postgres integration tests; needs `just up` beforehand.
ci-full: ci test-integration

# ─── Release build ────────────────────────────────────────────────────────────
# These recipes are about building the optimized `tsqlx` binary locally;
# nothing here touches git tags, crates.io, or GitHub. The CI publish
# workflow lives in `.github/workflows/release.yml` and is unrelated.

# Print the workspace version that release-* recipes embed in the binary.
release-version:
    @echo "tsqlx {{version}}"

# Pre-flight gates (fmt + clippy + tests + audit + smoke) before a release build.
release-check: fmt-check lint test audit smoke-sqlite

# Build the optimized release binary into `target/release/tsqlx`.
release-build:
    cargo build --release -p tsqlx

# Print release-binary status: existence, size, mtime, SHA-256, --version output.
release-status:
    @if [ ! -x target/release/tsqlx ]; then \
        echo "no release binary yet  (run \`just release-build\`)"; \
        exit 0; \
    fi
    @echo "── target/release/tsqlx ──"
    @ls -lh target/release/tsqlx | awk '{print "size  :", $5; print "mtime :", $6, $7, $8}'
    @printf "sha256: " && sha256sum target/release/tsqlx | awk '{print $1}'
    @printf "tsqlx  : " && target/release/tsqlx --version 2>/dev/null || echo "(binary did not respond to --version)"

# Build the release binary and print its status in one step (build + verify).
release: release-build release-status

# Build, then run the release binary; pass args after the recipe (default --help).
release-run *args="--help": release-build
    ./target/release/tsqlx {{args}}

# Install the release binary into ~/.cargo/bin/tsqlx (so `tsqlx` is on your PATH).
release-install:
    cargo install --path crates/tsqlx-app --force

# Remove the release build artifacts (keeps the debug target dir intact).
release-clean:
    rm -rf target/release
