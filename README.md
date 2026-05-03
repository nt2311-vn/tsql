# TSQL

TSQL is a Rust-based terminal database client for people who want a fast, reliable, keyboard-first SQL workspace that treats pasted SQL files correctly.

The project starts with a minimal foundation: a Cargo workspace, strict CI/CD, security scanning, and a small Rust skeleton. The first product milestone will focus on a Ratatui query editor that preserves multi-line pasted SQL and supports SQLite and Postgres execution.

## Goals

- **Reliable SQL editing**: preserve pasted SQL files, multi-line statements, comments, and driver-specific syntax.
- **Terminal-first UI**: build a modern Ratatui interface with Catppuccin Mocha as the default dark theme.
- **Driver-aware behavior**: support SQLite and Postgres first, then MySQL, MariaDB, Oracle, and MSSQL.
- **TOML configuration**: declare named database connections, editor behavior, indentation, theme, and autocomplete preferences in config files.
- **Zero-trust development**: use least-privilege CI permissions, mandatory tests, linting, formatting, dependency checks, secret scanning, and vulnerability scanning.
- **Open-source release discipline**: publish manually from protected tags only.

## Current Status

This repository is in the project scaffold phase.

Implemented foundation:

- Cargo workspace skeleton.
- Rust stable toolchain configuration through `.mise.toml`.
- `justfile` automation.
- GitHub Actions for CI, security scanning, and manual release.
- Branch protection documentation.
- Changelog.

Planned next:

- Ratatui application shell.
- Paste-safe SQL editor.
- SQLite and Postgres connection support.
- TOML configuration loader.
- SQL statement splitting, formatting, linting, and autocomplete boundaries.

## Workspace Layout

```text
.
├── crates
│   ├── tsql-app
│   ├── tsql-core
│   ├── tsql-db
│   ├── tsql-sql
│   └── tsql-tui
├── docs
├── .github
│   └── workflows
├── .mise.toml
├── justfile
├── Cargo.toml
├── CHANGELOG.md
└── README.md
```

## Development

Install tools with `mise`:

```sh
mise install
```

Run the local CI checks:

```sh
just ci
```

Useful commands:

```sh
just fmt
just fmt-check
just lint
just test
just audit
just security
```

## CI and Security

Pull requests are expected to pass:

- Rust formatting check.
- Clippy linting with warnings denied.
- Workspace tests.
- Cargo dependency vulnerability audit.
- Secret scanning with TruffleHog and Gitleaks.
- Semgrep OSS scan.
- Trivy filesystem vulnerability scan.
- Optional Snyk scan when `SNYK_TOKEN` is configured.

Workflow permissions are intentionally narrow by default.

## Branch Protection

The repository owner should protect `main` with the rules described in `docs/branch-protection.md`.

Recommended policy:

- No direct pushes to `main`.
- Pull requests required before merge.
- Required owner review.
- Required CI and security checks.
- No force pushes.
- Manual release approval through a protected environment.

## Release

Crates.io publication is manual and tag-based.

Expected release flow:

1. Create a version tag.
2. Trigger the release workflow manually.
3. Approve the protected `crates-io-release` environment.
4. Publish with `CARGO_REGISTRY_TOKEN`.

## License

Licensed under either MIT or Apache-2.0.
