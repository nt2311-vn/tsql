set shell := ["bash", "-uc"]
docker_compose := `if command -v docker-compose >/dev/null 2>&1; then echo docker-compose; else echo "docker compose"; fi`

default:
    just --list

setup:
    rustup component add rustfmt clippy
    cargo install cargo-audit --locked

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

test:
    cargo test --workspace --all-features

test-integration:
    TSQL_TEST_POSTGRES_URL=postgres://tsql:tsql@127.0.0.1:54329/tsql cargo test -p tsql-db --test postgres -- --ignored

smoke-sqlite:
    cargo run -p tsql -- exec --url sqlite::memory: --file examples/query.sql

smoke-metadata:
    TSQL_TEST_POSTGRES_URL=postgres://tsql:tsql@127.0.0.1:54329/tsql cargo test -p tsql-db --test metadata -- --ignored

up:
    {{docker_compose}} up -d --wait

down:
    {{docker_compose}} down --remove-orphans

audit:
    cargo audit

security: audit
    @echo "Run GitHub security workflow for TruffleHog, Gitleaks, Semgrep, Trivy, and optional Snyk."

ci: fmt-check lint test audit

ci-full: ci test-integration

release-check: fmt-check lint test audit smoke-sqlite
    cargo package --workspace --allow-dirty --no-verify
