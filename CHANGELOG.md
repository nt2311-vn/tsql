# Changelog

All notable changes to this project will be documented in this file.

This project intends to follow Semantic Versioning and the Keep a Changelog format.

## [Unreleased]

### Added

- **Persist new connections.** After typing a URL via `n new
  connection`, the connect screen prompts for a friendly name. Empty +
  Enter (or Esc) skips; otherwise the URL is appended to
  `~/.config/tsql/config.toml` so it shows in the picker next time.
  `tsql_core::append_connection` writes raw TOML so existing
  `${ENV_VAR}` placeholders and comments survive byte-for-byte. Name
  collisions are resolved with a `-N` numeric suffix.
- **Number-key tab navigation.** `1`-`6` jump straight to Records,
  Columns, Indexes, Keys, Constraints, and ERD. `l`/`h` cycling stays
  intact for muscle memory.
- **`Shift+X` closes the active table** and returns to the empty-detail
  placeholder, so you can pick another table without collapsing the
  schema first.
- **Records grid** — vertical column separators (`│`) and zebra-striped
  rows, plus a new `theme.row_alt_bg` colour. Same renderer is reused
  for the editor's results pane.
- **Editor upgrade.** Line-number gutter, basic SQL syntax highlighting
  (keywords / strings / numbers / comments), current-statement
  highlight that follows the cursor, `Ctrl+Enter` (and `Alt+Enter` as a
  terminal-compat fallback) runs only the statement under the cursor,
  `Ctrl+S` saves to the buffer's file, `:w [path]` and `:e <path>`
  palette commands for save/open, and per-connection persistent history
  stored under `~/.local/share/tsql/history/<name>.txt` (capped at 500
  deduped entries).
- **Narrower sidebar.** Browser sidebar is now 18% of terminal width
  (down from 24%), giving the detail pane more room.

### Added (earlier)

- **Postgres metadata integration tests** (`crates/tsql-db/tests/postgres.rs`):
  `postgres_overview_lists_tables_and_schemas`,
  `postgres_table_info_columns_and_pk`,
  `postgres_table_info_foreign_keys` (regression: catches the `FROM ,`
  syntax bug),
  `postgres_relationships_for_schema`, and
  `postgres_fetch_records_paginated`. Each test creates a unique
  throwaway schema so parallel runs cannot collide.
- **Reusable connection `Pool`** in `tsql-db`: `Pool::Postgres(PgPool)` /
  `Pool::Sqlite(SqlitePool)` with `connect`, `execute_script`,
  `fetch_overview`, `fetch_table_info`, `fetch_records`,
  `fetch_relationships` methods. The TUI now opens a pool once per
  connection and reuses it across all metadata calls and queries,
  eliminating the per-call connection handshake. URL-based public
  helpers stay as thin wrappers for the CLI and tests.
- **Non-blocking metadata loads in the TUI**: table info, records, and
  relationship fetches run on `tokio::spawn` tasks and stream results
  back through an `mpsc` channel that the event loop drains every
  ~33 ms. Stale results are dropped silently when the user navigates
  away mid-flight, so the UI never displays the wrong table.
- **Cursor-based SQL editor**: Left/Right/Up/Down/Home/End move the
  cursor (UTF-8 aware), Backspace and Delete edit at the cursor,
  inserts land at the cursor, and the terminal hardware cursor is
  positioned via `Frame::set_cursor_position`. **Query history**
  retains the 50 most recent successful submissions; Ctrl+P recalls
  older entries, Ctrl+N steps forward toward a fresh draft.
- **`:` command palette** in Browser mode with `:select`, `:insert`,
  `:describe`, `:indexes`, `:keys`, `:constraints`, `:erd`, `:help`,
  `:q` and short aliases. Identifier qualification adapts to the
  active driver (Postgres uses schema-qualified `"schema"."table"`,
  SQLite drops the schema prefix).
- **ERD jump-to-table**: `j`/`k` move a highlight bar through the
  foreign-key edges, `Enter` opens the referenced (target) table,
  `o` opens the owning (source) table. Sidebar selection follows the
  jump when the table is visible.
- Expanded `0.1.0` scope: TUI database browser with Vim navigation and ERD.
- Database introspection APIs in `tsql-db` for SQLite and Postgres.
- `fetch_overview`, `fetch_table_info`, and `fetch_records` metadata loaders.
- `fetch_relationships` loader for schema-scoped ERD views.
- `just smoke-metadata` task for introspection verification.
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
- Event-aware TruffleHog scanning so pushes to `main` do not compare identical refs.
- Initial Cargo workspace skeleton.
- Minimal Rust crates for app, core, database, SQL, and TUI boundaries.
- `.mise.toml` using Rust stable.
- `justfile` automation for formatting, linting, testing, auditing, security checks, and local CI parity.
- GitHub Actions CI workflow for format, clippy, tests, and dependency audit.
- GitHub Actions security workflow for secret scanning, Semgrep, Trivy, and optional Snyk.
- Manual tag-based crates.io release workflow.
- Branch protection documentation for the `main` branch.
- Initial public project README.
