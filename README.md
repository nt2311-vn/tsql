# TSQL

A fast, keyboard-first terminal database client for **PostgreSQL** and **SQLite**, built in Rust.

Just run `tsql` and you're at a connection picker. No flags. No GUI. No compromises.

```sh
tsql
```

It auto-loads `~/.config/tsql/config.toml`, lets you paste a fresh URL, drills down through schemas → tables → records, runs SQL with statement-aware execution, and gives you a native pure-Rust ERD visualizer. All from inside your terminal.

---

## Status at a glance

| Area                 | State        | Notes                                                          |
| -------------------- | ------------ | -------------------------------------------------------------- |
| PostgreSQL driver    | ✅ Stable     | Full metadata: columns, indexes, PKs, FKs, constraints         |
| SQLite driver        | ✅ Stable     | PRAGMA-driven introspection; `:memory:` and file URLs          |
| TUI browser          | ✅ Stable     | Schemas → tables → 6 detail tabs                               |
| Records grid         | ✅ Stable     | Paginated 50/page, zebra rows, `y`/`Y` yank                    |
| SQL editor           | ✅ Stable     | Per-connection history, run all / run-current, `:w` `:e`       |
| ERD visualizer       | ✅ Stable     | **Pure-Rust focused graph** (no external tools)                |
| `.mmd` export        | ✅ Stable     | `y` on ERD tab writes `<schema>.mmd` for GitHub/Notion         |
| Connection persist   | ✅ Stable     | `n` flow appends to `config.toml` with name prompt             |
| Catppuccin Mocha     | ✅ Stable     | Only theme; PK/FK/NULL aware                                   |
| Theme switcher       | 🟡 Planned   | Toggle Frappe / Latte / custom                                 |
| MySQL / MariaDB      | 🟡 Planned   | Driver scaffold next                                           |
| MSSQL / Oracle       | ⏳ Later      | After MySQL is stable                                          |
| `/` search filter    | 🟡 Planned   | Across sidebar + records                                       |
| System clipboard     | 🟡 Planned   | `arboard` for `y`/`Y`                                          |
| Connection pool reuse| 🟡 Planned   | Pool already wired through `AppState`; needs caching layer     |
| SQL autocomplete     | ⏳ Later      | Driver-aware identifier + keyword completion                   |

Legend: ✅ shipped · 🟡 in flight (next minor) · ⏳ later milestone

---

## Architecture

TSQL is a small Rust workspace. Each crate has one job and depends only on the layers below it:

```mermaid
graph TD
    subgraph cli["tsql-app · binary"]
        A[main.rs<br/>clap entry]
    end
    subgraph tui["tsql-tui · TUI engine"]
        B[Browser / Editor / Connect]
        B2[Pure-Rust ERD canvas]
        B3[Statement editor + history]
    end
    subgraph sql["tsql-sql · statement splitter"]
        C[SqlDocument<br/>boundary-aware tokens]
    end
    subgraph db["tsql-db · driver layer"]
        D[Pool::Postgres / Pool::Sqlite]
        D2[Schema introspection]
        D3[StatementOutput streaming]
    end
    subgraph core["tsql-core · config + types"]
        E[ConnectionConfig<br/>XDG loader / writer]
        E2[DriverKind / env expansion]
    end

    A --> B
    B --> B2
    B --> B3
    B --> C
    B --> D
    B --> E
    C --> D
    D --> E
```

Why split this way?

- **`tsql-core`** has no DB or UI deps. Cheap to test, easy to embed.
- **`tsql-db`** is the only crate that touches `sqlx`. Driver dialects live here.
- **`tsql-sql`** statement splitting is pure parsing — no IO. Used by the editor's "run current statement" feature.
- **`tsql-tui`** owns rendering and event handling. Async DB tasks send messages back through an mpsc channel, so the event loop never blocks.
- **`tsql-app`** is just a thin CLI wrapper around the library crates.

---

## Runtime topology

The TUI runs a single Tokio runtime. Slow database queries are dispatched as background tasks; their results return through a channel that the event loop drains every frame:

```mermaid
sequenceDiagram
    autonumber
    participant K as Keyboard
    participant L as run_loop (event loop)
    participant U as draw(frame)
    participant T as tokio task
    participant DB as Pool (Postgres/SQLite)

    K->>L: KeyEvent
    L->>L: handle_key (mutate AppState)
    L->>T: spawn(load_table_info)
    T->>DB: introspection / query
    DB-->>T: rows
    T-->>L: DbMessage::TableInfo via mpsc
    L->>L: drain rx, apply to AppState
    L->>U: terminal.draw(|f| draw(f, app))
    U-->>L: frame committed
    L-->>K: poll(33ms)
```

Three guarantees fall out of this design:

1. **The UI never blocks.** Even on a 30-second analytical query, you can still navigate, switch tabs, and abort.
2. **Stale messages are dropped.** Each `DbMessage` carries the schema + table + offset it was launched for; if you've moved on, it's silently ignored.
3. **No global state.** Everything lives on `AppState`, threaded explicitly into each handler.

---

## ERD visualizer

The ERD tab gives you a **focused schema map** centred on whichever table you're highlighting:

```
┌─ Schema map  (focused on selected table) ─────────────────────────────────────┐
│                                                                               │
│  ┌────────────────┐                  ╭─ orders ──────╮     ┌──────────────┐   │
│  │ shipments      │── order_id ─────▶│ ★ id          │── customer_id ─────│──▶│ customers │
│  │ (order_id)     │                  │ ⚷ customer_id │                    └───────────┘   │
│  └────────────────┘                  │   amount      │                                    │
│                                      │   issue_date  │                                    │
│                                      ╰───────────────╯                                    │
│                                                                                           │
│  ←1 incoming   1 outgoing→   0 neighbours hidden                                          │
└───────────────────────────────────────────────────────────────────────────────────────────┘
```

- `★` marks primary-key columns, `⚷` marks foreign-key columns.
- Tables on the **left** are the ones referencing the centre table. Tables on the **right** are the ones the centre table references.
- The arrow label is the FK column name. Arrows route orthogonally with box-drawing characters.
- Press `f` to fullscreen the chart, `j/k` to focus a different table, `Enter` to drill into it, `y` to dump a Mermaid `erDiagram` to `./<schema>.mmd` for sharing on GitHub / Notion / mermaid.live.

### Why pure Rust, not Mermaid CLI?

The previous renderer shelled out to `mmdc` + `chafa`, rendered Mermaid → PNG → ANSI in a temp dir, and parsed the output back into ratatui spans. It worked, but:

- `mmdc` requires Node + a full Chromium under Puppeteer (~300 MB).
- `chafa` substitutions varied wildly across terminal fonts; the chart often looked like CJK soup.
- Two timeouts, one tempdir, one ANSI parser, and a render hash just to keep the cache coherent.
- First-run latency was 2–5 seconds, which broke the "fast TUI" promise.

The replacement is ~400 lines of Rust composing a `Vec<Vec<Cell>>` grid with box-drawing characters. It renders in microseconds, has zero external dependencies, and degrades gracefully when the pane is small.

```mermaid
flowchart LR
    A[Selected table<br/>+ relationships] --> B[render_focus_canvas]
    B --> C[Cell grid<br/>w x h]
    C --> D[draw_card · centre + neighbours]
    C --> E[draw_arrow · orthogonal box-drawing]
    C --> F[grid_to_lines]
    F --> G[ratatui Paragraph]
```

---

## Quick start

```sh
# Launch TUI (reads ~/.config/tsql/config.toml if it exists)
tsql

# Or connect directly
tsql tui --url postgres://user:pass@localhost/mydb
tsql tui --url sqlite:./local.db

# Run a script
tsql exec --url sqlite::memory: --file query.sql

# Validate a config
tsql config check --config examples/tsql.toml
```

## Configuration

```toml
# ~/.config/tsql/config.toml
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

`${ENV_VAR}` placeholders are expanded at load time. Never commit passwords. The `n new connection` flow appends `[connections.<name>]` blocks for you so saved URLs survive restarts.

---

## Keyboard map

```mermaid
stateDiagram-v2
    [*] --> Connect
    Connect --> Browser: Enter
    Browser --> Editor: e / i
    Editor --> Browser: Esc
    Browser --> Browser: Tab (sidebar/detail)\nl/h (drill / back)\n1-6 (jump tab)
    Editor --> Editor: Ctrl+R (run all)\nCtrl+Enter (run current)\nCtrl+P/N (history)
    Browser --> [*]: q
```

| Mode        | Key             | Action                                              |
| ----------- | --------------- | --------------------------------------------------- |
| All         | `q`             | Quit (except when typing)                           |
| All         | `Ctrl+C`        | Force quit                                          |
| Connect     | `j/k`           | Navigate saved connections                          |
| Connect     | `Enter`         | Connect to selected                                 |
| Connect     | `n`             | New connection (paste URL, then name to persist)    |
| Connect     | `Tab`           | Toggle driver (Postgres/SQLite)                     |
| Browser     | `j/k`           | Navigate sidebar / records                          |
| Browser     | `l/Enter`       | Expand schema or select table                       |
| Browser     | `h`             | Collapse / go back                                  |
| Browser     | `Tab`           | Switch sidebar ↔ detail pane                        |
| Browser     | `l/h` (detail)  | Cycle detail tabs                                   |
| Browser     | `1`–`6`         | Jump straight to a detail tab                       |
| Browser     | `Shift+X`       | Close the active table                              |
| Browser     | `e` or `i`      | Open SQL editor                                     |
| Browser     | `y`             | Yank cell value (or `.mmd` export on ERD tab)       |
| Browser     | `Y`             | Yank entire row (TSV)                               |
| Browser     | `:`             | Command palette (`:select`, `:w`, `:e`, `:help`, `:q`) |
| ERD         | `j/k`           | Focus a different table                             |
| ERD         | `Enter` / `o`   | Open the focused table                              |
| ERD         | `f`             | Toggle fullscreen schema map                        |
| Editor      | `Ctrl+R`        | Run all statements                                  |
| Editor      | `Ctrl+Enter`    | Run statement under cursor (also `Alt+Enter`)       |
| Editor      | `Ctrl+S`        | Save buffer to its file (`:w <path>` retargets)     |
| Editor      | `Ctrl+P/Ctrl+N` | Browse persistent history                           |
| Editor      | `Esc`           | Back to browser                                     |

---

## Sample ERP database

A small lite-ERP dataset (customers, products, sales orders, items, work orders, invoices, payments) lives in `seed/`. The same SQL is portable across both drivers — perfect for trying the ERD view:

```sh
# Postgres
just postgres-up                                  # alias: just up
tsql tui --url postgres://tsql:tsql@127.0.0.1:54329/tsql
just postgres-down                                # alias: just down
just postgres-reseed                              # wipe volume + re-init

# SQLite
just sqlite-up                                    # alias: just seed-sqlite
tsql tui --url sqlite:./erp.db
just sqlite-down

# Both
just drivers-up
just drivers-down
```

---

## Development

```sh
mise install        # toolchain
just ci             # fmt + clippy + test + audit
just test           # run tests only
just lint           # clippy
just fmt            # format
just smoke-sqlite   # quick SQLite smoke test
```

### Project layout

```
tsql/
├── crates/
│   ├── tsql-app/    binary entry (clap)
│   ├── tsql-tui/    TUI, ERD canvas, SQL editor
│   ├── tsql-sql/    statement splitter
│   ├── tsql-db/     sqlx pool + introspection
│   └── tsql-core/   config types, XDG loader
├── seed/            ERP sample dataset (pg + sqlite)
├── examples/        sample tsql.toml
└── docs/            ADRs and design notes
```

### CI gates

Pull requests must pass:

- `cargo fmt` check
- `cargo clippy -D warnings`
- Workspace tests (SQLite + Postgres integration)
- `cargo audit`
- Secret scanning (TruffleHog, Gitleaks)
- Semgrep and Trivy vulnerability scans

---

## Roadmap

### 0.2.0 (next)

- Connection pool reuse cached on `AppState`
- System clipboard via `arboard` for `y`/`Y`
- Views and row counts in sidebar
- `/` search filter (sidebar + records)
- Loading spinner for in-flight DB tasks

### Later

- MySQL / MariaDB driver
- MSSQL and Oracle drivers
- SQL syntax highlighting + formatter
- Driver-aware autocomplete

---

## Release

Tag-based manual release to crates.io via the protected GitHub Actions environment.

## License

Licensed under either MIT or Apache-2.0.
