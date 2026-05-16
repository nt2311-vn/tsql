# TSQLX

A fast, keyboard-first terminal database client for **PostgreSQL**, **SQLite**, **MySQL / MariaDB**, **Microsoft SQL Server**, and **Oracle**, built in Rust.

Runs on **Linux** (any libc) and **macOS** (Apple Silicon + Intel). Postgres / SQLite / MySQL travel through pure-Rust `sqlx` (rustls TLS); MSSQL travels through `tiberius` against the OS-native TLS stack (SecureTransport on macOS, SChannel on Windows, OpenSSL on Linux). The Oracle driver is opt-in (`--features oracle`) and links against Oracle Instant Client at runtime.

Just run `tsqlx` and you're at a connection picker. No flags. No GUI. No compromises.

```sh
tsqlx
```

It auto-loads `~/.config/tsqlx/config.toml`, lets you paste a fresh URL, drills down through schemas → tables → records, runs SQL with statement-aware execution, supports bracketed paste of whole multi-line scripts into the editor, and gives you a native pure-Rust ERD visualizer. All from inside your terminal.

---

## Install

### From crates.io (recommended)

```sh
cargo install tsqlx
```

This builds the `tsqlx` binary from source and drops it into `~/.cargo/bin/`. Make sure that directory is on your `PATH`.

To force a reinstall on top of an older version:

```sh
cargo install tsqlx --force
```

#### With the Oracle driver (opt-in)

The Oracle driver links against the proprietary Oracle Instant Client at runtime, so it is gated behind a Cargo feature:

```sh
cargo install tsqlx --features oracle
```

You must have **Oracle Instant Client 12.1 or newer** installed and discoverable via `LD_LIBRARY_PATH` (Linux) or `DYLD_LIBRARY_PATH` (macOS). Without the feature flag, the other four drivers (PostgreSQL, SQLite, MySQL/MariaDB, MS SQL Server) work out of the box.

### Prerequisites

- **Rust 1.85+** (`rustup update stable`)
- A C toolchain (`build-essential` on Debian/Ubuntu, Xcode CLI tools on macOS) — required by `ring`/`rustls` and `tiberius`.
- On Linux, OpenSSL headers are needed for the MSSQL driver's native TLS: `apt install libssl-dev pkg-config` (Debian/Ubuntu) or equivalent.

### From source

```sh
git clone https://github.com/nt2311-vn/tsqlx
cd tsqlx
cargo install --path crates/tsqlx-app
```

### Verify

```sh
tsqlx --version
tsqlx --help
```

---

## Status at a glance

```mermaid
%%{init: {"theme": "dark"}}%%
gantt
    title Roadmap status (✅ shipped · 🟡 next · ⏳ later)
    dateFormat  YYYY-MM-DD
    axisFormat  %b
    section Drivers
    PostgreSQL                  :done,    pg,   2026-01-01, 60d
    SQLite                      :done,    sl,   2026-01-15, 50d
    MySQL / MariaDB             :done,    my,   2026-04-15, 30d
    MS SQL Server (tiberius)    :active,  ms,   after my, 30d
    Oracle (oracle, opt-in)     :active,  or,   after ms, 30d
    section TUI
    Browser + 6 detail tabs     :done,    br,   2026-02-01, 70d
    Records grid (zebra, yank)  :done,    rg,   2026-02-20, 45d
    Pure-Rust ERD canvas        :done,    erd,  2026-04-01, 40d
    SQL editor                  :done,    ed,   2026-02-15, 70d
    Bracketed-paste multiline   :done,    bp,   2026-04-25, 14d
    Vertical scroll editor      :done,    vs,   2026-04-25, 14d
    section Polish
    /search filter (sidebar)    :done,    sf,   2026-05-10, 7d
    System clipboard (arboard)  :done,    cb,   2026-05-10, 7d
    Theme switcher              :done,    th,   2026-05-10, 7d
    SQL autocomplete            :         ac,   after th, 60d
```

| Area                   | State        | Notes                                                          |
| ---------------------- | ------------ | -------------------------------------------------------------- |
| PostgreSQL driver      | ✅ Stable     | Full metadata: cols, indexes, PKs, FKs, CHECK constraints      |
| SQLite driver          | ✅ Stable     | PRAGMA-driven introspection; `:memory:` and file URLs          |
| MySQL / MariaDB driver | ✅ Stable     | `information_schema` introspection; CHECK on 8.0+/10.2+        |
| MS SQL Server driver   | 🟡 0.3.0      | `tiberius` + `bb8`; `sys.*` introspection; T-SQL `GO` batches  |
| Oracle driver          | 🟡 0.3.0      | `oracle` crate behind `--features oracle`; PL/SQL `/` batches  |
| TUI browser            | ✅ Stable     | Schemas → tables → 6 detail tabs                               |
| Records grid           | ✅ Stable     | Paginated 50/page, zebra rows, `y`/`Y` yank                    |
| SQL editor             | ✅ Stable     | Run all / run-current, history, `:w` `:e`, multiline           |
| **Bracketed paste**    | ✅ Stable     | Paste a whole `.sql` script in one event                       |
| **Vertical scroll**    | ✅ Stable     | Cursor-following auto-scroll; Ln:Col indicator                 |
| ERD visualizer         | ✅ Stable     | Pure-Rust focused graph (no external tools)                    |
| `.mmd` export          | ✅ Stable     | `y` on ERD tab → `<schema>.mmd` for GitHub/Notion              |
| Connection persist     | ✅ Stable     | `n` flow appends to `config.toml` with name prompt             |
| Catppuccin Mocha       | ✅ Stable     | Default theme; PK/FK/NULL aware                                |
| Theme switcher         | ✅ Stable     | `Ctrl+T` cycles 6 themes; persists to `config.toml`            |
| `/` search filter      | ✅ Stable     | Sidebar (schema/table names) + Records (any cell)              |
| System clipboard       | ✅ Stable     | `arboard` for `y`/`Y`; falls back to status bar when headless  |
| Connection pool reuse  | 🟡 Planned   | Pool already wired; needs caching layer                        |
| SQL autocomplete       | ⏳ Later      | Driver-aware identifier + keyword completion                   |

Legend: ✅ shipped · 🟡 in flight (next minor) · ⏳ later milestone

---

## Architecture

TSQLX is a small Rust workspace. Each crate has one job and depends only on the layers below it.

```mermaid
%%{init: {"theme": "dark"}}%%
graph TD
    subgraph cli["tsqlx-app · binary"]
        A[main.rs<br/>clap entry]
    end
    subgraph tui["tsqlx-tui · TUI engine"]
        B[Browser / Editor / Connect]
        B2[Pure-Rust ERD canvas]
        B3[Statement editor + history]
        B4[Bracketed-paste handler]
    end
    subgraph sql["tsqlx-sql · statement splitter"]
        C[SqlDocument<br/>boundary-aware tokens]
    end
    subgraph db["tsqlx-db · driver layer"]
        D[Pool::Postgres / Sqlite / MySql / Mssql / Oracle]
        D2[Schema introspection]
        D3[StatementOutput streaming]
        D4[Driver-specific cell decoders]
    end
    subgraph core["tsqlx-core · config + types"]
        E[ConnectionConfig<br/>XDG loader / writer]
        E2[DriverKind / env expansion]
    end

    A --> B
    B --> B2
    B --> B3
    B --> B4
    B --> C
    B --> D
    B --> E
    C --> D
    D --> E
```

Why split this way?

- **`tsqlx-core`** has no DB or UI deps. Cheap to test, easy to embed.
- **`tsqlx-db`** is the only crate that touches `sqlx`. Driver dialects live here.
- **`tsqlx-sql`** statement splitting is pure parsing — no IO. Used by the editor's "run current statement" feature.
- **`tsqlx-tui`** owns rendering and event handling. Async DB tasks send messages back through an mpsc channel, so the event loop never blocks.
- **`tsqlx-app`** is just a thin CLI wrapper around the library crates.

---

## Driver matrix

```mermaid
%%{init: {"theme": "dark"}}%%
flowchart LR
    URL[connection URL] --> Detect{DriverKind::from_url}
    Detect -->|postgres:// or postgresql://| PG[(PostgreSQL)]
    Detect -->|sqlite:...| SL[(SQLite)]
    Detect -->|mysql:// or mariadb://| MY[(MySQL / MariaDB)]
    Detect -->|mssql:// sqlserver:// tds://| MS[(MS SQL Server)]
    Detect -->|oracle://| OR[(Oracle)]
    PG --> PoolPG[Pool::Postgres - sqlx-postgres]
    SL --> PoolSL[Pool::Sqlite - sqlx-sqlite]
    MY --> PoolMY[Pool::MySql - sqlx-mysql]
    MS --> PoolMS[Pool::Mssql - tiberius + bb8]
    OR --> PoolOR[Pool::Oracle - oracle crate]
    PoolPG --> Meta[Schema introspection]
    PoolSL --> Meta
    PoolMY --> Meta
    PoolMS --> Meta
    PoolOR --> Meta
    Meta --> TUI[Same TUI views for every driver]
```

| Capability                | Postgres                   | SQLite                  | MySQL / MariaDB             | MS SQL Server                       | Oracle                                |
| ------------------------- | -------------------------- | ----------------------- | --------------------------- | ----------------------------------- | ------------------------------------- |
| Multiple schemas          | ✅ `pg_namespace`           | One schema (`main`)     | ✅ Each DATABASE              | ✅ `sys.schemas`                      | ✅ Each `OWNER`                        |
| Columns + types           | ✅ `information_schema`     | ✅ `PRAGMA table_info`   | ✅ `information_schema`       | ✅ `sys.columns` + `sys.types`        | ✅ `ALL_TAB_COLUMNS`                   |
| Primary keys              | ✅                          | ✅                       | ✅                            | ✅ `sys.indexes is_primary_key`       | ✅ `ALL_CONSTRAINTS type='P'`          |
| Foreign keys              | ✅                          | ✅ `PRAGMA fk_list`      | ✅ Explicit `FOREIGN KEY` †   | ✅ `sys.foreign_keys`                 | ✅ `ALL_CONSTRAINTS type='R'`          |
| Composite FKs             | ✅                          | ✅                       | ✅                            | ✅                                    | ✅                                     |
| Indexes (multi-col)       | ✅ + access method          | ✅ btree                 | ✅ + index_type               | ✅ + `type_desc`                      | ✅                                     |
| CHECK constraints         | ✅ `pg_constraint`          | ⚠️ surfaced on column   | ✅ MySQL 8.0+ / MariaDB 10.2+ | ✅ `sys.check_constraints`            | ✅ `ALL_CONSTRAINTS type='C'`          |
| TIMESTAMP / DATE decoding | ✅ chrono                   | ✅ chrono                | ✅ chrono                     | ✅ chrono                             | ✅ chrono                              |
| NUMERIC / DECIMAL         | ✅ `BigDecimal`             | ✅                       | ✅ `BigDecimal`               | ✅ `Numeric`                          | ✅                                     |
| JSON                      | ✅                          | n/a                     | ✅                            | n/a (NVARCHAR(MAX) + `ISJSON`)       | ✅ `JSON` (21c+) or `CLOB`             |
| UUID                      | ✅                          | TEXT                    | TEXT                         | ✅ `uniqueidentifier`                 | RAW(16)                               |

† MySQL silently ignores inline `REFERENCES` clauses. Use explicit `CONSTRAINT … FOREIGN KEY` blocks (the bundled `seed/mysql/01_schema.sql` is already adapted).

**MS SQL Server URL format** — `mssql://user:pass@host:port/database?encrypt=on&trust_cert=true&instance=NAMED`. Schemes `sqlserver://` and `tds://` are accepted as aliases. T-SQL `GO` is recognised as a batch separator in scripts.

**Oracle URL format** — `oracle://user:pass@host:port/service_name`. Requires building with `--features oracle` and having Oracle Instant Client (≥ 19c) on the runtime `LD_LIBRARY_PATH` / `DYLD_LIBRARY_PATH`. PL/SQL `BEGIN…END;` blocks and `/`-on-its-own-line batch terminators are honoured.

---

## Runtime topology

The TUI runs a single Tokio runtime. Slow database queries are dispatched as background tasks; their results return through a channel that the event loop drains every frame:

```mermaid
%%{init: {"theme": "dark"}}%%
sequenceDiagram
    autonumber
    participant K as Keyboard
    participant L as run_loop
    participant P as paste handler
    participant U as draw(frame)
    participant T as tokio task
    participant DB as Pool (PG/SL/MY/MS/OR)

    K->>L: KeyEvent or Paste(text)
    alt key
        L->>L: handle_key (mutate AppState)
    else paste
        L->>P: handle_paste(text)
        P->>P: editor.insert_str at cursor
    end
    L->>T: spawn(load_table_info / records)
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

## SQL editor

The editor is statement-aware: it knows where each `;`-terminated statement starts and ends, even with strings, line comments, block comments, and Postgres `$tag$ … $tag$` dollar quotes in the way. That powers the "run only the statement under the cursor" shortcut without forcing you to select anything.

```mermaid
%%{init: {"theme": "dark"}}%%
flowchart TD
    Paste[Paste / type] --> Buffer[(editor: String)]
    Buffer --> Tokens[statement_range_at<br/>boundary-aware]
    Tokens --> Hl[highlight_line<br/>keywords / strings / numbers]
    Buffer --> Scroll[Auto-scroll<br/>keep cursor visible]
    Scroll --> Draw[Paragraph<br/>line gutter + body]
    Hl --> Draw
    Tokens --> RunCur[Ctrl+Enter<br/>execute current stmt]
    Buffer --> RunAll[Ctrl+R<br/>execute all stmts]
    RunCur --> Pool
    RunAll --> Pool
    Pool[(driver Pool)] --> Results[Results pane<br/>same grid as Records]
```

### What "multi-line paste" actually means

Pasting a 200-line `.sql` file used to deliver one `KeyEvent::Char(c)` per character — slow, and history saw each line as a separate edit. With **bracketed paste mode** enabled at startup, the terminal hands the whole clipboard over as a single `Event::Paste(String)`. We:

1. Normalise CRLF / stray CR to LF (Windows clipboards leave them in).
2. Insert at the cursor in one shot.
3. Update the status bar with `pasted N chars / M line(s)` so you know the dump landed.

Combined with the new **vertical auto-scroll**, you can paste an arbitrarily long script and the cursor / viewport stay in sync. The editor banner shows `[Ln 12:4 / 87]` so you always know where you are inside a long buffer.

| Editor key                       | Action                                              |
| -------------------------------- | --------------------------------------------------- |
| `Ctrl+R`                         | Run all statements                                  |
| `Ctrl+Enter` / `Alt+Enter`       | Run statement under cursor                          |
| `Ctrl+S`                         | Save buffer to its file (`:w <path>` retargets)     |
| `Ctrl+P` / `Ctrl+N`              | Browse persistent history                           |
| `Ctrl+A` / `Home`                | Line start                                          |
| `Ctrl+E` / `End`                 | Line end                                            |
| `Up` / `Down`                    | Vertical cursor (preserves column)                  |
| `Esc`                            | Back to browser                                     |
| *(any paste)*                    | Bracketed paste — multi-line, single event          |

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
- Tables on the **left** reference the centre table. Tables on the **right** are referenced by it.
- Arrow labels are FK column names. Arrows route orthogonally with box-drawing characters.
- Press `f` to fullscreen the chart, `j/k` to focus a different table, `Enter` to drill into it, `y` to dump a Mermaid `erDiagram` to `./<schema>.mmd`.

```mermaid
%%{init: {"theme": "dark"}}%%
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
# Launch TUI (reads ~/.config/tsqlx/config.toml if it exists)
tsqlx

# Or connect directly
tsqlx tui --url postgres://user:pass@localhost/mydb
tsqlx tui --url sqlite:./local.db
tsqlx tui --url mysql://tsqlx:tsqlx@127.0.0.1:33069/tsqlx
tsqlx tui --url mariadb://tsqlx:tsqlx@127.0.0.1:33079/tsqlx

# Run a script
tsqlx exec --url sqlite::memory: --file query.sql

# Validate a config
tsqlx config check --config examples/tsqlx.toml
```

## Configuration

```toml
# ~/.config/tsqlx/config.toml
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

[connections.staging-mysql]
driver = "mysql"
url = "${MYSQL_URL}"

[connections.legacy-mariadb]
driver = "mariadb"            # also accepted; alias for mysql
url = "mariadb://app:app@db:3306/legacy"
```

`${ENV_VAR}` placeholders are expanded at load time. Never commit passwords. The `n new connection` flow appends `[connections.<name>]` blocks for you so saved URLs survive restarts.

---

## Result-set export and SQL autoformat

Open a table, run a query, then drop into the command palette with `:`:

```
:export csv ./users.csv
:export json ./users.json                          # pretty-printed array
:export ndjson ./users.ndjson                      # one row per line
:export sql ./seed.sql                             # INSERT INTO "<current_table>" (...) VALUES (...);
:export sql ./seed.sql --table=public.users        # override the target name
```

CSV uses RFC-4180 quoting via the `csv` crate. JSON pretty mode emits an array of `{ column: value }` objects; NDJSON drops the array brackets and writes one object per line — pipe straight into `jq`. SQL INSERT double-quotes identifiers and single-quotes values, doubling embedded `'` characters. Parent directories are created on demand. The status bar reports row count + bytes.

Inside the editor, `:fmt` (alias `:format`) autoformats the buffer in place via `sqlformat`:

```sql
-- before :fmt
select id,name from users where active=1 order by name;

-- after :fmt (defaults: 2-space indent, lowercase keywords)
select
  id,
  name
from
  users
where
  active = 1
order by
  name;
```

Defaults are 2-space indent, **lowercase keywords** (set `[editor.format].uppercase_keywords = true` in `config.toml` if you want `SHOUTY CASE`), and one blank line between `;`-separated statements. The formatter is idempotent — running it twice produces the same output.

Syntax highlighting in the editor is driven by `sqlparser`'s tokenizer (not regex). Dollar-quoted bodies, multi-line block comments, hex/national/double-quoted string literals, escaped single quotes, and dialect type keywords (`INT`, `TEXT`, `JSONB`, `TIMESTAMPTZ`, …) all classify correctly, in every bundled theme.

---

## Vim modal editing & autocomplete

The SQL editor is **modal**. The current mode shows in the editor title chip: `[NORMAL]`, `[INSERT]`, `[VISUAL]`.

| From | Press | Result |
|---|---|---|
| Browser | `e` | enter editor in **Normal** |
| Browser | `i` | enter editor in **Insert** |
| Browser | `:select` / `:insert` | prefill template, enter in **Insert** |

### Normal mode

| Key | Action |
|---|---|
| `h j k l` / arrows | move cursor |
| `0` / `$` | line start / line end |
| `w` / `b` | next word / previous word |
| `gg` / `G` | buffer start / buffer end |
| `i` / `a` | enter Insert at / after cursor |
| `I` / `A` | enter Insert at line start / end |
| `o` / `O` | open line below / above, enter Insert |
| `x` | delete char under cursor |
| `dd` / `yy` | delete / yank current line |
| `p` | paste yank register |
| `v` | enter Visual |
| `:` | open command palette |
| `Esc` | return to Browser (resets pending count) |
| `1`–`9` then key | repeat the next motion / operator N times (e.g. `5j`, `10w`, `3dd`, `2x`, `3p`); counts cap at 9999 |

### Insert mode

Typing inserts. `Esc` returns to Normal. **`Tab`** opens the autocomplete popup when there's an identifier prefix at the cursor; otherwise it inserts a 4-space soft-tab.

### Visual mode

`h j k l 0 $ w b` extend the selection — the editor pane paints the theme's selection background on every cell inside the range so the user can see exactly what will be yanked or deleted. `y` yanks, `d` deletes (both return to Normal). `Esc` cancels.

### Autocomplete popup

The popup attaches just below the cursor (it flips above when there's no room, and slides left when it would overflow the pane). The candidate set depends on cursor context:

| Context | Candidates |
|---|---|
| Start of statement / after `;` | top-level keywords (`SELECT`, `INSERT`, `UPDATE`, `DELETE`, `WITH`, …) |
| After `FROM` / `JOIN` / `INTO` / `UPDATE` | tables in the active schema, qualified `schema.table` across schemas |
| After `<qualifier>.` | if `qualifier` is a schema → its tables; if a cached table → its columns |
| After `SELECT` projection / `WHERE` / `ON` / `GROUP BY` / `ORDER BY` | columns from the currently-loaded `TableInfo` and any pre-fetched neighbours |

Ranking: prefix match > substring > fuzzy subsequence, capped at 12 visible. `Up` / `Down` cycle, `Tab` or `Enter` accepts (replaces the prefix span), `Esc` dismisses. The icons in the popup are `K` keyword, `T` table, `c` column, `S` schema.

### First-run friendliness

On a fresh install with no saved connections, the URL-entry screen accepts **both `Esc` and `q` to quit** when the input is empty — no more being trapped without a database to connect to.

---

## Keyboard map

```mermaid
%%{init: {"theme": "dark"}}%%
stateDiagram-v2
    [*] --> Connect
    Connect --> Browser: Enter
    Browser --> Editor: e / i
    Editor --> Browser: Esc
    Browser --> Browser: Tab (sidebar/detail)\nl/h (drill / back)\n1-6 (jump tab)
    Editor --> Editor: Ctrl+R (run all)\nCtrl+Enter (run current)\nPaste (multi-line)\nCtrl+P/N (history)
    Browser --> [*]: q
```

| Mode        | Key             | Action                                              |
| ----------- | --------------- | --------------------------------------------------- |
| All         | `q`             | Quit (except when typing)                           |
| All         | `Ctrl+C`        | Force quit                                          |
| All         | `Ctrl+T`        | Cycle theme (saves to `config.toml`)                |
| Browser     | `/`             | Live filter: sidebar names or Records rows          |
| Connect     | `j/k`           | Navigate saved connections                          |
| Connect     | `Enter`         | Connect to selected                                 |
| Connect     | `n`             | New connection (paste URL, then name to persist)    |
| Connect     | `Tab`           | Cycle driver: Postgres → SQLite → MySQL             |
| Browser     | `j/k`           | Navigate sidebar / records                          |
| Browser     | `l/Enter`       | Expand schema or select table                       |
| Browser     | `h`             | Collapse / go back                                  |
| Browser     | `Tab`           | Switch sidebar ↔ detail pane                        |
| Browser     | `l/h` (detail)  | Cycle detail tabs                                   |
| Browser     | `1`–`6`         | Jump straight to a detail tab                       |
| Browser     | `Shift+X`       | Close the active table                              |
| Browser     | `e` or `i`      | Open SQL editor                                     |
| Browser     | `y`             | Copy cell to system clipboard (ERD tab: `.mmd` export) |
| Browser     | `Y`             | Copy entire row (TSV) to system clipboard           |
| Browser     | `:`             | Command palette (`:select`, `:export`, `:fmt`, `:w`, `:e`, `:help`, `:q`) |
| ERD         | `j/k`           | Focus a different table                             |
| ERD         | `Enter` / `o`   | Open the focused table                              |
| ERD         | `f`             | Toggle fullscreen schema map                        |
| Editor      | `Ctrl+R`        | Run all statements                                  |
| Editor      | `Ctrl+Enter`    | Run statement under cursor (also `Alt+Enter`)       |
| Editor      | `Ctrl+S`        | Save buffer (`:w <path>` retargets)                 |
| Editor      | `Ctrl+P/N`      | Persistent history                                  |
| Editor      | `Esc`           | Insert→Normal · Normal→back to browser              |
| Editor (NORMAL) | `i a I A o O` | enter Insert (variants per vim)                    |
| Editor (NORMAL) | `h j k l 0 $ w b gg G` | movement                                  |
| Editor (NORMAL) | `x dd yy p`   | delete char / line, yank line, paste              |
| Editor (NORMAL) | `v`           | enter Visual                                       |
| Editor (NORMAL) | `:`           | command palette                                    |
| Editor (INSERT) | `Tab`         | autocomplete popup (or 4-space indent)             |
| Editor (popup)  | `Up/Down`     | cycle suggestions                                  |
| Editor (popup)  | `Tab/Enter`   | accept                                             |
| Editor (popup)  | `Esc`         | dismiss                                            |
| Editor (VISUAL) | `y d`         | yank / delete selection                            |
| Connect picker  | `Esc/q`       | quit (no input needed)                             |

---

## Platforms

| OS                  | Build        | Test in CI       | Notes                                                 |
| ------------------- | ------------ | ---------------- | ----------------------------------------------------- |
| Linux (x86_64)      | ✅ supported | `ubuntu-latest`  | Primary dev target                                    |
| macOS (Apple Silicon, arm64) | ✅ supported | `macos-latest`   | Tested on every PR via `dtolnay/rust-toolchain@stable` |
| macOS (Intel, x86_64) | ✅ supported | not in CI matrix | Same code path as arm64 — built locally with `cargo build --target x86_64-apple-darwin` |
| Windows             | ⏳ untested   | not in CI        | `crossterm` Windows backend should work; `dirs` config-dir semantics differ — needs a dedicated pass |

### macOS notes

- **Config path.** tsqlx resolves `~/.config/tsqlx/config.toml` everywhere, including macOS — many CLI tools follow this convention now (helix, neovim, etc). If you'd rather use the macOS-native location, set `XDG_CONFIG_HOME=~/Library/Application\ Support` in your shell rc.
- **History path.** `XDG_DATA_HOME` honored if set; otherwise falls back to `~/.local/share/tsqlx/history/`.
- **Bracketed paste.** Tested on iTerm2, Terminal.app, Alacritty, WezTerm, and Ghostty. All deliver `Event::Paste` cleanly.
- **No system clipboard yet.** `y` / `Y` only update the status bar; an `arboard`-backed clipboard hook is on the 0.2.0 roadmap.

The CI `test (macos-latest)` job builds the workspace, runs `cargo test --workspace --all-features`, and exercises the SQLite + MySQL/MariaDB unit suites on Apple Silicon hosts. Postgres integration is Linux-only because GitHub-hosted runners only expose `services:` containers there — the Postgres metadata fetchers themselves are pure Rust and platform-independent.

## Sample ERP database

A small lite-ERP dataset (customers, products, sales orders, items, work orders, invoices, payments) lives in `seed/`. The same SQL is portable across all three drivers — perfect for trying the ERD view:

```sh
# Postgres
just postgres-up                                  # alias: just up
tsqlx tui --url postgres://tsqlx:tsqlx@127.0.0.1:54329/tsqlx
just postgres-down                                # alias: just down
just postgres-reseed                              # wipe volume + re-init

# SQLite
just sqlite-up                                    # alias: just seed-sqlite
tsqlx tui --url sqlite:./erp.db
just sqlite-down

# MySQL
just mysql-up
tsqlx tui --url mysql://tsqlx:tsqlx@127.0.0.1:33069/tsqlx
just mysql-down
just mysql-reseed                                 # wipe volume + re-init

# MariaDB (same wire protocol; same driver)
just mariadb-up
tsqlx tui --url mariadb://tsqlx:tsqlx@127.0.0.1:33079/tsqlx
just mariadb-down

# All four at once
just drivers-up
just drivers-down
```

The MySQL/MariaDB seed (`seed/mysql/`) is the same schema with two tweaks:

- `TEXT NOT NULL UNIQUE` → `VARCHAR(191) NOT NULL UNIQUE` (MySQL key-prefix limit).
- Inline `REFERENCES` promoted to explicit `CONSTRAINT … FOREIGN KEY` blocks (MySQL silently ignores inline references).

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
tsqlx/
├── crates/
│   ├── tsqlx-app/    binary entry (clap)
│   ├── tsqlx-tui/    TUI, ERD canvas, SQL editor, paste handler
│   ├── tsqlx-sql/    statement splitter
│   ├── tsqlx-db/     sqlx pool + introspection (pg / sqlite / mysql)
│   └── tsqlx-core/   config types, XDG loader
├── seed/            Postgres + SQLite ERP sample
│   └── mysql/       MySQL / MariaDB-flavoured copy of the same data
├── examples/        sample tsqlx.toml
└── docs/            ADRs and design notes
```

### CI gates

Pull requests must pass:

- `cargo fmt` check
- `cargo clippy -D warnings`
- Workspace tests (SQLite + Postgres integration)
- `cargo audit` (with one MySQL-related transitive RustSec ignore documented in `justfile`)
- Secret scanning (TruffleHog, Gitleaks)
- Semgrep and Trivy vulnerability scans

---

## Roadmap

### 0.3.0 (in flight)

- ✅ Project rename: `tsql` → `tsqlx` (`tsql` was taken on crates.io)
- 🟡 MS SQL Server driver via `tiberius` + `bb8`; T-SQL `GO` batches
- 🟡 Oracle driver via the `oracle` crate (opt-in `--features oracle`); PL/SQL `/` batches
- 🟡 `mssql://`, `sqlserver://`, `tds://`, `oracle://` URL schemes

### Later

- ~~`/` search filter (sidebar + records)~~ ✅ shipped
- ~~System clipboard via `arboard` for `y`/`Y`~~ ✅ shipped
- ~~Theme switcher (Frappe / Latte / custom)~~ ✅ shipped — 6 themes, `Ctrl+T` cycles
- Loading spinner for in-flight DB tasks

### 0.3.0

- MSSQL driver
- Oracle driver
- SQL syntax highlighting overhaul + formatter
- Driver-aware autocomplete

---

## Release

Tag-based manual release to crates.io via the protected GitHub Actions environment.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.
