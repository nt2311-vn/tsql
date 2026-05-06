# TSQL

A fast, keyboard-first terminal database client for Postgres and SQLite, built in Rust.

Just run `tsql` to launch the TUI — it auto-loads saved connections from `~/.config/tsql/config.toml` and opens a connection picker. No flags needed.

## Features

- **Connection picker** — saved connections from config; paste a new URL to connect, then name it to persist back to `~/.config/tsql/config.toml` automatically.
- **Database browser** — schemas, tables, and 6 detail tabs: Records, Columns, Indexes, Keys, Constraints, ERD. Jump tabs with `1`-`6`, close the active table with `Shift+X`.
- **Vim navigation** — `j/k` up/down, `l/Enter` drill in, `h` back, `g/G` top/bottom, `Tab` switch panes.
- **SQL editor** — `e` or `i` to open. Line-number gutter, basic syntax highlighting, current-statement highlight, `Ctrl+R` runs all, `Ctrl+Enter` runs only the statement under the cursor, `Ctrl+S` / `:w [path]` save, `:e <path>` open. Per-connection on-disk history, recalled with `Ctrl+P`/`Ctrl+N`.
- **Records viewer** — paginated (50 rows), zebra-striped grid with vertical column separators, `y` yank cell, `Y` yank row, `[/]` scroll columns.
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

When you connect via `n new connection`, tsql prompts you for a friendly
name and appends a `[connections.<name>]` block to the same file so the
URL is saved for next time. The writer appends raw text — existing
`${ENV_VAR}` placeholders, comments, and ordering are preserved.

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
| **Browser** | `1`-`6` | Jump straight to a detail tab |
| **Browser** | `Shift+X` | Close the active table |
| **Browser** | `e` or `i` | Open SQL editor |
| **Browser** | `y` | Yank cell value |
| **Browser** | `Y` | Yank entire row (TSV) |
| **Browser** | `:` | Open command palette (`:select`, `:w`, `:e`, `:help`, `:q`, …) |
| **Editor** | `Ctrl+R` | Run all statements |
| **Editor** | `Ctrl+Enter` | Run statement under cursor (also `Alt+Enter`) |
| **Editor** | `Ctrl+S` | Save buffer to its file (set via `:w <path>`) |
| **Editor** | `Ctrl+P` / `Ctrl+N` | Browse persistent history |
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

Each driver has its own up/down recipe so you can pick the sandbox you
want to poke at:

```sh
# Postgres (dockerized, seed scripts auto-run on first container start).
just postgres-up                                 # alias: just up
tsql tui --url postgres://tsql:tsql@127.0.0.1:54329/tsql
just postgres-down                               # alias: just down
just postgres-reseed                             # wipe volume + re-init

# SQLite (local file, default ./erp.db).
just sqlite-up                                   # alias: just seed-sqlite
tsql tui --url sqlite:./erp.db
just sqlite-down

# Bring every driver up or down at once.
just drivers-up
just drivers-down
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
