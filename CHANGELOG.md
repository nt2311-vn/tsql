# Changelog

All notable changes to this project will be documented in this file.

This project intends to follow Semantic Versioning and the Keep a Changelog format.

## [Unreleased]

### Added

- **Vim repeat-count prefix in the editor.** Any digit sequence in
  Normal mode accumulates as a repeat count for the following
  motion / operator. `5j` moves down 5 lines, `10w` jumps 10 words
  forward, `3dd` deletes 3 lines, `3yy` yanks 3 lines, `2x` deletes
  2 chars, `3p` pastes the register 3 times. `0` alone is still the
  line-start motion; `30j` works because the second digit follows
  an already-accumulating count. Counts cap at 9999. `Esc` resets
  any pending count. Mode-entry keys (`i a I A o O v :`) consume
  and discard the count silently — `5i…<Esc>` does *not* yet
  replay the inserted text 5 times (vim's `Ni` semantics are out
  of scope for v1; called out in `vim.rs`).
- **Visual-mode selection highlight in the editor.** When `vim_mode`
  is `Visual`, the editor pane now paints the theme's selection
  background on every cell inside the active selection range, with
  byte-accurate boundaries: any syntax-highlight span that straddles
  the selection edge is split into `(pre, selected, post)` sub-spans
  so the highlight starts and ends exactly where the user expects.
  Multi-line selections are handled by checking the line's global
  byte range against the (normalised, anchor↔cursor) selection
  range on each rendered line.

- **Vim-style modal editing in the SQL editor.** Three modes —
  `NORMAL` (default), `INSERT`, `VISUAL` — surfaced in the editor
  title chip. Implemented keys:
  - Normal: `h j k l` move; `0` / `$` line start/end; `w` / `b`
    word forward/backward; `gg` buffer start, `G` buffer end;
    `i` / `a` / `I` / `A` / `o` / `O` enter Insert (with the
    appropriate cursor adjustment); `x` delete char under cursor;
    `dd` delete line; `yy` yank line; `p` paste; `v` enter Visual;
    `:` open command palette; any unrecognised key resets pending
    operator state.
  - Insert: types into the buffer; `Esc` returns to Normal; `Tab`
    opens the autocomplete popup when an identifier prefix is
    present, otherwise inserts the conventional 4-space soft-tab.
  - Visual: `h j k l` (and `0 $ w b`) extend selection; `y` yanks
    and returns to Normal; `d` deletes and returns to Normal;
    `Esc` returns to Normal.
  - All Ctrl- shortcuts (`Ctrl+R`, `Ctrl+Enter`, `Ctrl+S`,
    `Ctrl+P/N`) work in every mode.
  - `e` from Browser enters editor in `NORMAL`; `i` and the
    palette prefills (`:select`, `:insert`) enter in `INSERT`.
- **SQL autocomplete popup.** Pressing `Tab` in Insert mode opens a
  popup attached just below the cursor when an identifier prefix is
  present. Cursor-context detection (via the existing `sqlparser`
  tokenizer from PR #31) classifies the position as:
  - `Statement` → top-level keywords (SELECT, INSERT, UPDATE, …)
  - `AfterFrom` / `AfterJoin` / `AfterInto` / `AfterUpdate` →
    table names (qualified `schema.table` when across schemas)
  - `AfterDot("qual")` → if `qual` is a schema, its tables; if a
    cached table, its columns
  - `AfterSelectProjection` / `AfterWhere` / `AfterOn` /
    `AfterGroupBy` / `AfterOrderBy` → column names from cached
    `TableInfo`
  Candidates are ranked: prefix match (1000) → substring (500) →
  fuzzy subsequence (100), capped at 12 visible. `Up` / `Down`
  cycle, `Tab` or `Enter` accepts (replaces the prefix span),
  `Esc` dismisses. Empty popups never appear — the candidate
  source falls back to top-level keywords.
- **`Esc` and `q` quit from the first-run connection screen.** When
  there are no saved connections and the URL input is empty, both
  keys exit the app cleanly instead of trapping the user. The
  picker view's `Esc` always quits (mirroring `q`).

### Internal

- New module `tsqlx-sql::complete` (cursor-context detector +
  candidate ranker, pure).
- New module `tsqlx-tui::vim` (modal keymap router, pure — takes a
  `KeyEvent` plus pending operator state, returns the new
  `VimMode` plus a `Vec<VimAction>`).
- Editor key dispatch (`handle_editor_key`) routes every non-Ctrl
  key through `vim::handle_key`; a small in-lib `apply_vim_action`
  function translates each action into editor-buffer mutations.
- `editor::highlight_line` (already on the sqlparser tokenizer
  after PR #31) is unchanged here; the tokenizer is reused by the
  new autocomplete context detector with zero additional deps.

### Added

- **`:export <csv|json|ndjson|sql> <path> [--table=NAME]`.** Writes
  the active records grid to disk in any of four formats:
  - `csv` — RFC-4180 via the `csv` crate (header row from
    `rec.columns`, comma separator, RFC-4180 quoting).
  - `json` — pretty-printed array of `{ column: value }` objects.
  - `ndjson` — one JSON object per line, no enclosing array, ideal
    for `jq`/streaming sinks.
  - `sql` — single `INSERT INTO "table" ("c1","c2") VALUES (…), (…);`
    statement; identifiers double-quoted; values single-quoted with
    inner single quotes doubled. Table name defaults to
    `app.current_table` when `--table=NAME` is omitted.
  Status bar reports `exported N rows (B bytes) → path`. Parent
  directories are created on demand.
- **`:fmt` / `:format`.** Autoformat the editor buffer in place via
  `sqlformat`. Defaults: 2-space indent, lowercase keywords (`SHOUTY
  CASE` is opt-in via config), one blank line between `;`-separated
  statements. Idempotent (`fmt(fmt(x)) == fmt(x)`). The cursor is
  re-clamped to the nearest UTF-8 boundary after formatting.
- **Proper SQL syntax highlighting.** `editor::highlight_line` now
  delegates to `tsqlx_sql::classify_line`, which wraps the
  `sqlparser` tokenizer (`GenericDialect`, with
  `tokenize_with_location`). Dollar-quoted bodies, multi-line block
  comments (single-line range), national / hex / double-quoted
  string literals, escaped single quotes, and dialect type
  keywords (`INT`, `TEXT`, `JSONB`, `TIMESTAMPTZ`, …) all classify
  correctly. Token classes (`Keyword`, `Type`, `Identifier`,
  `StringLit`, `NumberLit`, `Comment`, `Operator`, `Punct`,
  `Plain`) map to existing theme colours so every theme in the
  registry just works without per-class palette additions.

### Internal

- New module `tsqlx-sql::highlight` (token classifier).
- New module `tsqlx-sql::format` (sqlformat wrapper with
  opinionated defaults).
- New module `tsqlx-tui::export` (CSV / JSON / NDJSON / SQL INSERT
  encoders).
- Workspace deps: `csv = "1.4"`, `sqlparser = "0.62"`,
  `sqlformat = "0.5"`, `serde_json = "1"`.
- Removed the hand-rolled `SQL_KEYWORDS` table and ad-hoc lexer in
  `editor.rs` — the sqlparser tokenizer handles every dialect we
  care about, sized at ~400 KB extra binary.



## [0.4.1] - 2026-05-12

### Added

- **Install section in README.** `cargo install tsqlx`, opt-in
  `--features oracle` (with Oracle Instant Client note), build
  prerequisites (Rust 1.85+, C toolchain, OpenSSL headers for
  the MSSQL driver), from-source flow, and a verify step.
- **`LICENSE-MIT` and `LICENSE-APACHE`** at the repo root so the
  `MIT OR Apache-2.0` metadata declared by every crate is backed
  by real files in both the repo and the published tarballs.

## [0.4.0] - 2026-05-11

### Added

- **Theme switcher.** Six built-in themes — Catppuccin Mocha,
  Macchiato, Frappe, and Latte alongside Tokyo Night and Gruvbox
  Dark. `Ctrl+T` cycles through them from any mode (Connect /
  Browser / Editor); the choice is written back to
  `[editor].theme` in `~/.config/tsqlx/config.toml` via
  `toml_edit` so comments, ordering, and `${ENV_VAR}` placeholders
  survive the round-trip.
- **`/` live search filter.** Pressing `/` in the Browser pane opens
  a vim-style filter prompt that updates the view per keystroke.
  Sidebar pane filters schemas + tables by case-insensitive
  substring; schemas with no matching child are hidden and matching
  schemas auto-expand. Records detail tab filters rows where any
  cell matches. Enter commits the filter and returns focus to
  navigation; Esc in the prompt clears the filter.
- **System clipboard via `arboard`.** `y` (cell or columns) and
  `Y` (row TSV) now hit the OS clipboard. Lazy-init on first yank
  so headless / no-`DISPLAY` sessions still launch the TUI; if the
  clipboard backend can't be reached the yank falls back to a
  status-bar preview so the user always sees what would have been
  copied. Linux pulls only the pure-Rust `wayland-data-control` +
  `x11rb` backends (no GTK).

### Fixed

- **`cargo audit` CI failure on `odpic-sys`.** The Oracle driver
  introduced in 0.3.0 pulls in `odpic-sys` transitively. A
  transient broken entry in the crates.io index periodically made
  `cargo audit`'s "Updating crates.io index" phase fail with
  `parse error: couldn't resolve dependency: odpic-sys`. The CI
  audit step now pre-clones the RustSec advisory database with a
  shallow `git clone` and runs `cargo audit --no-fetch`, which
  skips both the index refresh and cargo-audit's own DB fetch.
- **Stale `actions/checkout@v4` reference** in the Postgres
  integration job bumped to `@v6` to match the rest of the
  workflow.

### Dependencies

- Bump `crossterm` 0.28 → 0.29 (removes the duplicate version
  pulled in transitively by `ratatui`).
- Bump `toml` 0.8 → 1.x for the new spec-1.1 parser. The only
  surface we use (`from_str`, `de::Error`) is unchanged.
- Bump `tokio` 1.52.1 → 1.52.3 (patch).
- Add `arboard` 3.x (default features off, only the pure-Rust
  Linux backends enabled).
- Add `toml_edit` 0.22 for round-tripping the user's config file.

## [0.3.0] - 2026-05-10

### Added

- **Microsoft SQL Server driver.** Full TDS support via `tiberius`
  + `bb8-tiberius`. TLS is provided by the OS stack (SecureTransport
  on macOS, SChannel on Windows, OpenSSL on Linux) — `tiberius 0.12`
  pins `rustls 0.21` which transitively brings in vulnerable
  `rustls-webpki 0.101.x` (CVE-2026-0098/0099/0104), so we route
  through `native-tls` until tiberius bumps to rustls 0.23+.
  - URL formats: `mssql://`, `sqlserver://`, `tds://` (the last two
    are normalised to `mssql://` on connect). TOML `driver = "sqlserver"`
    and `driver = "tds"` are accepted as aliases.
  - Query-string options: `encrypt=on|off|required` (default `on`),
    `trust_cert=true|false` (default `false`), `instance=NAMED` for
    SQL Browser named-instance discovery.
  - Metadata introspection driven by `sys.schemas` / `sys.tables` /
    `sys.columns` / `sys.types` / `sys.indexes` / `sys.foreign_keys`.
    Distinguishes `is_primary_key` indexes; reports the index access
    method (`type_desc`) lower-cased.
  - Pagination via `OFFSET … ROWS FETCH NEXT … ROWS ONLY` with a
    `ORDER BY (SELECT NULL)` no-op so callers don't need a key.
  - **T-SQL `GO` batch separator.** New
    `tsqlx_sql::split_tsql_batches` peels SSMS-style batches off a
    script (case-insensitive, optional repeat count `GO 5`,
    string/comment-aware). `Pool::execute_script` for MSSQL hands
    each batch to `tiberius::Client::simple_query` so DDL chains
    that depend on prior batches work as in sqlcmd.
  - Docker sandbox: `mcr.microsoft.com/mssql/server:2022-latest` on
    `localhost:14330` (sa / Tsqlx_Pass1, Developer edition). Recipes
    `just mssql-up` / `just mssql-down` / `just test-mssql`.
  - 5 integration tests gated on `TSQLX_TEST_MSSQL_URL` (executes,
    overview, table_info, relationships, paginated records).
  - CI: new `mssql-integration` job mirroring the Postgres pattern.
- **Oracle Database driver (opt-in via `--features oracle`).** Built
  on the `oracle` crate (OCI bindings). All blocking calls run
  inside `tokio::task::spawn_blocking` so the rest of the async
  surface is unchanged.
  - URL format: `oracle://user:pass@host:port/service_name`. Without
    the feature, `DriverKind::Oracle` still parses but
    `Pool::connect` returns `DbError::Unsupported` so the binary
    builds on machines without Instant Client.
  - Metadata introspection driven by `ALL_TABLES` / `ALL_TAB_COLUMNS`
    / `ALL_CONSTRAINTS` / `ALL_CONS_COLUMNS`. PKs are reconstructed
    from `constraint_type='P'`, FKs from `'R'` with one extra round
    trip per FK to resolve the referenced table/columns.
  - Pagination via Oracle 12c+ `OFFSET … ROWS FETCH NEXT … ROWS ONLY`.
  - **PL/SQL `/` batch terminator.** New
    `tsqlx_sql::split_plsql_batches` recognises SQL*Plus `/`-on-its-
    own-line as a batch boundary, leaving `/` inside expressions
    (`SELECT a/b FROM t`) and block comments alone. `Pool::execute_
    script` strips the trailing `;` from non-PL/SQL batches (Oracle
    rejects it) but keeps it on `BEGIN…END;` blocks.
  - Docker sandbox: `gvenzl/oracle-free:23-slim-faststart` on
    `localhost:15210` (FREEPDB1 PDB, login `tsqlx`/`tsqlx_pass`).
    Deliberately excluded from `just drivers-up` because of its
    ~90-second cold-start cost. Recipes `just oracle-up` /
    `just oracle-down` / `just test-oracle`.
  - 5 integration tests gated on `TSQLX_TEST_ORACLE_URL` and
    `#![cfg(feature = "oracle")]` so `cargo test --workspace`
    without features still discovers and skips them.
- **Driver matrix table** in the README extended to MSSQL + Oracle
  with their introspection sources.

### Changed

- **Project rename `tsql` → `tsqlx`.** `tsql` was already taken on
  crates.io. All five workspace crates (`tsqlx-app`, `tsqlx-core`,
  `tsqlx-db`, `tsqlx-sql`, `tsqlx-tui`), the binary, the
  `tsqlx_*` Rust import paths, the `~/.config/tsqlx/` config dir,
  the `~/.local/share/tsqlx/history/` history dir, the
  `TSQLX_TEST_POSTGRES_URL` test env var, the docker-compose dev
  credentials, and the canonical `https://github.com/nt2311-vn/tsqlx`
  repo URL all moved together.
- **Driver-matrix mermaid in README.** GitHub's renderer silently
  fails when edge labels contain `<br/>` inside quoted strings (e.g.
  `-- "postgres://"<br/>"postgresql://" -->`). Rewrote the chart
  with single-line edge labels (`-->|postgres:// or postgresql://|`)
  and added MSSQL + Oracle pool nodes.

### Fixed

- **Docker healthcheck password.** The `mysqladmin -ptsql` /
  `mariadb-admin -ptsql` healthchecks weren't picked up by the
  `\btsql\b` rename regex because there's no word boundary between
  `-p` and the password fragment. Renamed to `-ptsqlx` so the
  healthcheck actually authenticates.

## [0.2.0] - 2026-05-09

### Added

- **MySQL / MariaDB driver.** Full `information_schema` introspection
  (cols, PKs, FKs, indexes, CHECK constraints) and a cell decoder
  covering signed/unsigned ints, `BigDecimal`, chrono date/time,
  JSON, and byte vectors. URLs starting with `mysql://` or
  `mariadb://` resolve to the same `Pool::MySql` variant; sqlx only
  speaks `mysql://` so `DriverKind::Mysql.normalise_url` rewrites
  `mariadb://` on connect. Dockerized `mysql:8.4` and `mariadb:11.4`
  sandboxes via `just mysql-up` / `just mariadb-up`, with a
  MySQL-flavoured ERP seed at `seed/mysql/` (sized VARCHARs +
  explicit `FOREIGN KEY` constraints since MySQL silently ignores
  inline `REFERENCES`).
- **Pure-Rust ERD visualizer.** Replaced the `mmdc` + `chafa`
  pipeline with a hand-rolled box-drawing canvas centred on the
  selected table. No external tools, no async render pipeline,
  microsecond redraws. Centre card shows full column list with
  `★` PK and `⚷` FK markers; side cards show 1-hop neighbours with
  FK column names labelling each connector arrow. Pane shrinks
  gracefully — drops the less-useful side, then both, before
  refusing to render. Side card width adapts to pane width
  (14/16/18 cells) so half-screen panes still show neighbours.
- **Multi-line SQL editor.**
  - Bracketed paste enabled at startup; pasting a whole `.sql`
    file arrives as one `Event::Paste(String)` instead of one key
    per character. CRLF / stray CR collapsed to LF.
  - Cursor-following vertical auto-scroll (`editor_scroll: Cell`)
    so an arbitrarily long buffer always shows the cursor row.
  - Editor banner shows `[Ln L:C / total]` so position is clear
    even in a 200-line buffer.
- **Half-width terminal layouts.**
  - Records grid: horizontal column window auto-following the
    focused column (`[`/`]` slides it). Min cell width 14 so the
    `YYYY-MM-DD` date prefix always fits. Body cells now
    left-aligned (`lcell`) so RFC3339 timestamps truncate cleanly
    from the right instead of getting both ends chopped by
    centre-alignment. Bottom-row scroll indicator shows
    `cols X–Y / N` when there are off-screen columns.
  - Columns / Indexes / Keys / Constraints tabs: percentage widths
    swapped for `Min(N) + Length(N)` so narrow panes show full
    names instead of clipping to four chars.
- **macOS support.** Already worked thanks to a pure-Rust
  dependency tree (`sqlx + runtime-tokio-rustls`, `ratatui`,
  `crossterm`, `dirs`); now exercised on every PR via a
  `[ubuntu-latest, macos-latest]` matrix in `ci.yml`. New
  cross-platform test in `tsqlx-core` exercises path resolution
  on Apple Silicon. README has a Platforms table and macOS notes.
- **Manual-trigger release workflow.** `release.yml` is now
  `workflow_dispatch` only with `dry_run`, `create_tag`, and
  `create_github_release` inputs. Pre-flight job verifies
  inter-crate version pins match the workspace, refuses to
  proceed if `vX.Y.Z` already exists, and runs fmt + clippy +
  test + audit. Publish job retries each `cargo publish` up to
  five times to handle sparse-index propagation lag. Final job
  creates the annotated tag and a GitHub Release with auto-notes.

### Changed

- **Editor key map.** `Up` / `Down` arrow keys now move the cursor
  vertically inside the editor (previously a no-op). `Ctrl+P` /
  `Ctrl+N` keep their history-recall role.
- **Driver toggle on the connect screen.** `Tab` cycles
  `Postgres → SQLite → MySQL → Postgres` so all three drivers are
  reachable without a chord.
- **README.** Rewritten with a status gantt chart, four-driver
  matrix table, runtime sequence diagram including the paste path,
  editor data flow chart, ERD render flow, and a Platforms section.

### Removed

- **`mmdc` + `chafa` pipeline** and all of its scaffolding:
  `ErdChart`, `ErdChartStatus`, `ErdChartError`,
  `parse_ansi_to_lines`, `apply_sgr`, `render_mermaid_with_chafa`,
  `maybe_spawn_chart_render`, `fnv1a64`. Dropped runtime `tempfile`
  dep and the unused `tokio` `process` feature.

### Fixed

- Records body cells centre-truncating values like RFC3339
  timestamps to garbage like `-01-05T09:15:00+0` (centre alignment
  chopped both ends). Now left-aligned so the meaningful prefix
  always wins.

## [0.1.0] - earlier

### Added

- **Persist new connections.** After typing a URL via `n new
  connection`, the connect screen prompts for a friendly name. Empty +
  Enter (or Esc) skips; otherwise the URL is appended to
  `~/.config/tsqlx/config.toml` so it shows in the picker next time.
  `tsqlx_core::append_connection` writes raw TOML so existing
  `${ENV_VAR}` placeholders and comments survive byte-for-byte. Name
  collisions are resolved with a `-N` numeric suffix.
- **Number-key tab navigation.** `1`-`6` jump straight to Records,
  Columns, Indexes, Keys, Constraints, and ERD. The tab labels now
  also display their hotkey (`1 Records`, `2 Columns`, …) so the
  binding is discoverable. `l`/`h` cycling stays intact for muscle
  memory.
- **Index type + PK visibility.** The Indexes tab now surfaces the
  access method (`BTREE`, `HASH`, `GIN`, `GIST`, `BRIN`, `SPGIST`)
  and a `PK` column with a `★` for the primary-key index. Postgres
  metadata now includes the PK index (previously filtered out) so the
  default btree backing each table is always visible. SQLite reports
  every regular index as btree (FTS/R*Tree live as virtual tables).
- **ERD: render the Mermaid chart inline via `mmdc` + `chafa`.**
  The Mermaid source by itself wasn't useful — users had to paste
  it into another tool to actually see anything. Now the chart
  pane at the top of the ERD tab calls out to the Mermaid CLI
  (`mmdc`) to rasterize the schema to a PNG and pipes that
  through `chafa` to convert it into colored Unicode half-blocks
  rendered directly inside the TUI.

  - **Pipeline.** `mermaid_erdiagram` → temp `.mmd` → `mmdc -t
    dark -b transparent -w 1600 -H 1200` → temp `.png` →
    `chafa --format=symbols --symbols=block+border --size=WxH`
    → bespoke SGR parser → `Vec<Line<'static>>`.
  - **Async + cached.** Render runs on a tokio task; result
    arrives as a new `DbMessage::ErdChart` variant. FNV-1a hash
    of the Mermaid source keys the cache so a redraw doesn't
    re-shell on every frame; pane size changes ≥2 cells re-trigger
    a render.
  - **Status states.** Idle / Rendering / Ready / Failed /
    MissingTool. When `mmdc` or `chafa` aren't on `PATH`, the
    pane shows an actionable install hint plus a `y` prompt for
    saving the source manually.
  - **Layout.** Vertical split on the ERD tab: banner row at top,
    chart pane (~60% height, full width), then the table list +
    per-table inspector below (~40%). Chart gets max horizontal
    real estate, which it actually needs.

  New deps: `tempfile` (runtime) and `tokio` `process` feature.
  External tools (`mmdc`, `chafa`) are optional — graceful fallback
  to the install hint when missing.
- **ERD: structured inspector + Mermaid export.** Two visual
  attempts at an in-terminal diagram (layered graph, then a
  dbdiagram-style card grid) hit hard limits — ASCII / box-drawing
  line routing never looked smooth, parallel edges crossed badly,
  and arrowheads rendered inconsistently across fonts. So the tab
  pivots to what TUIs actually do well: a structured two-pane
  inspector plus a copy-pastable Mermaid block.

  - **Tables pane (left).** Bordered list of every table in the
    active schema. `j`/`k` cycles, `Enter`/`o` opens the
    selected table as the active browser table.
  - **Inspector pane (right).** Four sections for the selected
    table:
    1. **Columns** — each row shows a `★` PK / `⚷` FK / blank
       badge, the column name, the type, and (for FK columns)
       an inline `→ other_table.col` reference.
    2. **References →** — outgoing FKs as styled rows
       (`local_col → other_table.col`).
    3. **Referenced by ←** — incoming FKs.
    4. **Mermaid** — a complete, ready-to-paste
       `\`\`\`mermaid … erDiagram … \`\`\`` block for the whole
       schema. PK / FK column roles tagged. Cardinality drawn as
       `}o--||` (many-to-one).
  - **`y` saves the Mermaid block to `./<schema>.mmd`** so it's a
    one-key step from the TUI to a real ERD in any Mermaid-aware
    viewer (GitHub, Notion, IDE preview, mermaid.live).

  Removes the old layered renderer, the dbdiagram card-grid
  renderer, and `ErdJump`/`jump_to_erd_target`. Keeps the
  `erd_table_info` cache (now feeding the inspector + Mermaid
  generator) and `spawn_erd_prefetch` (still kicked off on every
  ERD-tab entry path).
- **Decode timestamp / numeric / uuid / json cells properly.**
  Records previously rendered every `NUMERIC`, `TIMESTAMP`,
  `TIMESTAMPTZ`, `DATE`, `TIME`, `UUID`, `JSON`, and `JSONB` value as
  the literal placeholder `<timestamp>` / `<numeric>` / etc. — they
  looked like NULLs or seed bugs. Root cause: `postgres_cell` /
  `sqlite_cell` only tried `String` / `i64` / `f64` / `bool`, which
  sqlx rejects for those Postgres OIDs. Enabled the sqlx `chrono`,
  `bigdecimal`, `uuid`, and `json` features and added explicit
  decode branches: `BigDecimal` → plain decimal, `DateTime<Utc>` →
  RFC 3339, `NaiveDateTime` → `YYYY-MM-DD HH:MM:SS`, `Uuid` →
  hyphenated, `JsonValue` → compact JSON. Bytes still fall through
  to `0x…` hex.
- **Audit ignore for unfixable RSA advisory.** `RUSTSEC-2023-0071`
  (RSA timing sidechannel) reaches us only transitively through
  `sqlx-mysql` (we don't use MySQL) and has no upstream fix.
  Documented + ignored in `just audit` so CI stays green.
- **Hide Postgres-internal schemas.** The schema picker previously
  surfaced `pg_toast` (and `pg_temp_*` if present) because the query
  only excluded `information_schema` and `pg_catalog`. Now uses
  `NOT LIKE 'pg\_%'` so every internal schema disappears and only
  user schemas remain.
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
  stored under `~/.local/share/tsqlx/history/<name>.txt` (capped at 500
  deduped entries).
- **Narrower sidebar.** Browser sidebar is now 18% of terminal width
  (down from 24%), giving the detail pane more room.

### Added (earlier)

- **Postgres metadata integration tests** (`crates/tsqlx-db/tests/postgres.rs`):
  `postgres_overview_lists_tables_and_schemas`,
  `postgres_table_info_columns_and_pk`,
  `postgres_table_info_foreign_keys` (regression: catches the `FROM ,`
  syntax bug),
  `postgres_relationships_for_schema`, and
  `postgres_fetch_records_paginated`. Each test creates a unique
  throwaway schema so parallel runs cannot collide.
- **Reusable connection `Pool`** in `tsqlx-db`: `Pool::Postgres(PgPool)` /
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
- Database introspection APIs in `tsqlx-db` for SQLite and Postgres.
- `fetch_overview`, `fetch_table_info`, and `fetch_records` metadata loaders.
- `fetch_relationships` loader for schema-scoped ERD views.
- `just smoke-metadata` task for introspection verification.
- Hybrid CLI/TUI `0.1.0` MVP work.
- `tsqlx config check` for TOML configuration validation.
- `tsqlx exec` for executing SQL files or stdin against SQLite and Postgres.
- Minimal `tsqlx tui` Ratatui interface with Catppuccin Mocha styling.
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
