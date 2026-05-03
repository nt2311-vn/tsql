# Changelog

All notable changes to this project will be documented in this file.

This project intends to follow Semantic Versioning and the Keep a Changelog format.

## [Unreleased]

### Added

- Hybrid CLI/TUI `0.1.0` MVP work.
- `tsql config check` for TOML configuration validation.
- `tsql exec` for executing SQL files or stdin against SQLite and Postgres.
- Minimal `tsql tui` Ratatui interface with Catppuccin Mocha styling.
- Multi-statement SQL splitting for pasted SQL scripts.
- SQLite integration test.
- Postgres Docker Compose and CI integration test support.
- Example TOML config and SQL script.
- Project knowledge files: `knowledge.aaak`, `graph.md`, `Vault/Journal`, and `Vault/Spec`.
- `just smoke-sqlite` and `just release-check` automation.
- README roadmap for post-`0.1.0` work.
- `ratatui` 0.30 upgrade to clear Trivy/cargo-audit transitive dependency findings.
- Initial Cargo workspace skeleton.
- Minimal Rust crates for app, core, database, SQL, and TUI boundaries.
- `.mise.toml` using Rust stable.
- `justfile` automation for formatting, linting, testing, auditing, security checks, and local CI parity.
- GitHub Actions CI workflow for format, clippy, tests, and dependency audit.
- GitHub Actions security workflow for secret scanning, Semgrep, Trivy, and optional Snyk.
- Manual tag-based crates.io release workflow.
- Branch protection documentation for the `main` branch.
- Initial public project README.
