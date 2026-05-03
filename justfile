set shell := ["bash", "-uc"]

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

audit:
    cargo audit

security: audit
    @echo "Run GitHub security workflow for TruffleHog, Gitleaks, Semgrep, Trivy, and optional Snyk."

ci: fmt-check lint test audit
