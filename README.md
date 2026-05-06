# TSQL

A fast, keyboard-first terminal database client for Postgres and SQLite, built in Rust.

Just run `tsql` to launch the TUI — it auto-loads saved connections from `~/.config/tsql/config.toml` and opens a connection picker. No flags needed.

## Features

- **Connection picker** — saved connections from config, or paste a new URL to connect instantly.
- **Database browser** — schemas, tables, and 6 detail tabs: Records, Columns, Indexes, Keys, Constraints, ERD.
- **Vim navigation** — `j/k` up/down, `l/Enter` drill in, `h` back, `g/G` top/bottom, `Tab` switch panes.
- **SQL editor** — `e` or `i` to open, paste multi-line SQL, `Ctrl+R` to execute.
- **Records viewer** — paginated (50 rows), `y` yank cell, `Y` yank row, `[/]` scroll columns.
- **ERD view** — schema-scoped FK relationship tree.
- **Catppuccin Mocha** — dark theme with PK/FK column coloring, NULL dimming, active highlights.
- **XDG config** — auto-loads `~/.config/tsql/config.toml` with `${ENV_VAR}` expansion for secrets.
- **Postgres + SQLite** — full metadata introspection (indexes, constraints, FKs) for both drivers.

## Quick Start

```sh
# Launch TUI (reads ~/.config/tsql/config.toml if it exists)
tsql

# Or connect directly
tsql tui --url postgres://user:pass@localhost/mydb
tsql tui --url sqlite:./local.db
```

## Configuration

Create `~/.config/tsql/config.toml`:

```toml
[editor]
tab_width = 4
indent = "spaces"
theme = "catppuccin-mocha"

[connections.prod]
driver = "postgres"
url = "${DATABASE_URL}"

[connections.local]
driver = "sqlite"
url = "sqlite:./dev.db"
```

Use environment variables for secrets. Never commit database passwords.

## CLI Commands

```sh
# Open TUI with connection picker
tsql

# Open TUI with a specific connection
tsql tui --url sqlite::memory:
tsql tui --config my.toml --connection prod

# Execute SQL from a file
tsql exec --url sqlite::memory: --file query.sql

# Validate a config file
tsql config check --config examples/tsql.toml
```

## Keyboard Shortcuts

| Mode | Key | Action |
|------|-----|--------|
| **All** | `q` | Quit (except when typing) |
| **All** | `Ctrl+C` | Force quit |
| **Connect** | `j/k` | Navigate saved connections |
| **Connect** | `Enter` | Connect to selected |
| **Connect** | `n` | New connection (paste URL) |
| **Connect** | `Tab` | Toggle driver (Postgres/SQLite) |
| **Browser** | `j/k` | Navigate sidebar / records |
| **Browser** | `l/Enter` | Expand schema or select table |
| **Browser** | `h` | Collapse / go back |
| **Browser** | `Tab` | Switch sidebar ↔ detail pane |
| **Browser** | `l/h` (detail) | Cycle detail tabs |
| **Browser** | `e` or `i` | Open SQL editor |
| **Browser** | `y` | Yank cell value |
| **Browser** | `Y` | Yank entire row (TSV) |
| **Editor** | `Ctrl+R` | Execute SQL |
| **Editor** | `Esc` | Back to browser |

## Development

```sh
mise install        # Install toolchain
just ci             # Full local CI (fmt, clippy, test, audit)
just test           # Run tests
just lint           # Clippy
just fmt            # Format
just smoke-sqlite   # Quick SQLite smoke test
```

## Sample ERP database

A small lite-ERP dataset (customers, products, sales orders, sales-order
items, work orders, invoices, payments) lives in `seed/`. The same SQL is
portable across both supported drivers.

```sh
# Postgres: seed scripts auto-run on first container start.
just up
tsql tui --url postgres://tsql:tsql@127.0.0.1:54329/tsql

# Re-run the seed (drops the volume, re-initializes).
just reseed

# SQLite: apply the same schema + data to a local file.
just seed-sqlite              # writes ./erp.db
tsql tui --url sqlite:./erp.db
```

## CI and Security

Pull requests must pass:

- `cargo fmt` check
- `cargo clippy -D warnings`
- Workspace tests (SQLite + Postgres integration)
- `cargo audit`
- Secret scanning (TruffleHog, Gitleaks)
- Semgrep and Trivy vulnerability scans

## Roadmap

### 0.2.0

- Connection pool reuse (hold pool in AppState)
- System clipboard for yank (`arboard`)
- Views and row counts in sidebar
- `/` search filter for tables and records
- Async DB calls with loading spinner
- MySQL / MariaDB driver

### Later

- SQL syntax highlighting
- SQL formatting and linting
- Driver-aware autocomplete
- MSSQL and Oracle drivers

## Release

Tag-based manual release to crates.io via GitHub Actions protected environment.

## License

Licensed under either MIT or Apache-2.0.
