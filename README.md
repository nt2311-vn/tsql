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

This repository is preparing the `0.1.0` MVP. The goal is a usable terminal database browser (releasing to crates.io is deferred until the browser is mature).

Implemented:

- Cargo workspace skeleton.
- Rust stable toolchain configuration through `.mise.toml`.
- `justfile` automation.
- GitHub Actions for CI, security scanning, and manual release.
- Branch protection documentation.
- Changelog.
- TOML config loading with environment-variable expansion.
- SQLite and Postgres script execution.
- Multi-statement SQL splitting for pasted SQL scripts.
- Hybrid CLI plus minimal Ratatui TUI.
- **Database Introspection APIs** for schemas, tables, columns, indexes, keys, and constraints.

Planned for 0.1.0:

- **Richer TUI Browser**: Vim-style navigation (`h/j/k/l`).
- **Metadata Explorer**: Drill down from connection -> schema -> table -> details.
- **Record Browser**: Paginated record viewing with `y` to copy.
- **Schema ERD**: Relationship graph view within schema scopes.

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
just smoke-sqlite
just up
just test-integration
just down
just audit
just security
just release-check
```

## Usage

Validate a config file:

```sh
tsql config check --config examples/tsql.toml
```

Execute a SQL file against SQLite:

```sh
tsql exec --url sqlite::memory: --file examples/query.sql
```

Execute a SQL file through a named TOML connection:

```sh
TSQL_POSTGRES_URL=postgres://tsql:tsql@127.0.0.1:54329/tsql \
  tsql exec --config examples/tsql.toml --connection local_postgres --file examples/query.sql
```

Open the minimal TUI:

```sh
tsql tui --config examples/tsql.toml --connection local_sqlite
```

Inside the TUI:

- **Type or paste SQL** into the editor.
- **Run SQL** with `Ctrl+R`.
- **Quit** with `Esc` or `Ctrl+C`.

## Configuration

TSQL uses TOML for named connections and editor preferences:

```toml
[editor]
tab_width = 4
indent = "spaces"
theme = "catppuccin-mocha"

[connections.local_sqlite]
driver = "sqlite"
url = "sqlite::memory:"

[connections.local_postgres]
driver = "postgres"
url = "${TSQL_POSTGRES_URL}"
```

Use environment variables for secrets. Do not commit database passwords.

## CI and Security

Pull requests are expected to pass:

- Rust formatting check.
- Clippy linting with warnings denied.
- Workspace tests.
- SQLite and Postgres integration tests.
- Cargo dependency vulnerability audit.
- Secret scanning with TruffleHog and Gitleaks.
- Semgrep OSS scan.
- Trivy filesystem vulnerability scan.
- Snyk is documented as skipped for Cargo because Snyk CLI does not support Rust dependency scanning.

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

## Knowledge Base

Project knowledge is tracked in repository files so design context survives across sessions:

- `knowledge.aaak`: compact project knowledge and acceptance criteria.
- `graph.md`: architecture and workflow graph.
- `Vault/Journal`: implementation journal entries.
- `Vault/Spec`: versioned product and release specs.

## Roadmap

### 0.1.0

- Hybrid CLI/TUI MVP.
- SQLite and Postgres execution.
- TOML config with environment-variable expansion.
- Multi-statement pasted SQL support.
- Catppuccin Mocha Ratatui shell.
- CI-backed integration tests.

### 0.2.0

- Better TUI navigation and result table interactions.
- Safer terminal restoration and panic handling.
- Config discovery from platform config paths.
- Connection health checks.

### 0.3.0

- SQL formatting.
- SQL linting.
- Driver-aware autocomplete.
- Schema browser.

### Later

- MySQL and MariaDB.
- MSSQL.
- Oracle.
- Plugin or driver feature architecture.

## Release

Crates.io publication is manual and tag-based.

Expected release flow:

1. Create a version tag.
2. Trigger the release workflow manually.
3. Approve the protected `crates-io-release` environment.
4. Publish with `CARGO_REGISTRY_TOKEN`.

## License

Licensed under either MIT or Apache-2.0.
