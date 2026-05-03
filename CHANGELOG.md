# Changelog

All notable changes to this project will be documented in this file.

This project intends to follow Semantic Versioning and the Keep a Changelog format.

## [Unreleased]

### Added

- Initial Cargo workspace skeleton.
- Minimal Rust crates for app, core, database, SQL, and TUI boundaries.
- `.mise.toml` using Rust stable.
- `justfile` automation for formatting, linting, testing, auditing, security checks, and local CI parity.
- GitHub Actions CI workflow for format, clippy, tests, and dependency audit.
- GitHub Actions security workflow for secret scanning, Semgrep, Trivy, and optional Snyk.
- Manual tag-based crates.io release workflow.
- Branch protection documentation for the `main` branch.
- Initial public project README.
