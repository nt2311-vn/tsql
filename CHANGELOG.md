# Changelog

All notable changes to this project will be documented in this file.

This project intends to follow Semantic Versioning and the Keep a Changelog format.

## [Unreleased]

### Added

- **Lite ERP seed dataset** in `seed/` (`01_schema.sql`, `02_data.sql`):
  customers, products, sales orders, sales-order items, work orders,
  invoices, payments. The Postgres compose service mounts `seed/` at
  `/docker-entrypoint-initdb.d`, so the sandbox is ready to browse on
  first start. The SQL is portable across Postgres and SQLite so both
  drivers can be exercised against identical data.
- **Driver-explicit justfile recipes**: `postgres-up`, `postgres-down`,
  `postgres-reseed`, `sqlite-up [db]`, `sqlite-down [db]`, plus
  `drivers-up` / `drivers-down` to bring every sandbox up or down at
  once. Old `up` / `down` / `reseed` / `seed-sqlite` are kept as
  backward-compatible aliases.
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
