# tsqlx — production-readiness status (as of v0.4.0)

This is an honest snapshot of where tsqlx stands versus a mature
desktop client (DBeaver Community is the natural comparator) and a
prioritised list of gaps. It is intended as **the** source of truth
for "is feature X done?" — if a row says ❌, it is not done, even if
adjacent rows are.

Last refresh: working tree at workspace `0.4.0`, plus the in-flight
stack PR #27 → PR #28 → PR #29.

---

## TL;DR

tsqlx is a **fast, keyboard-first terminal database client** that
already does well the 80% of work an experienced engineer does
inside a SQL editor: connect, browse, inspect schema/keys/indexes,
run queries (single statement, all statements, multi-line paste),
yank cells/rows, view a focused ERD, and export Mermaid. Boot time
is measured in milliseconds, the binary is one statically-linked
executable, the resident set is a few megabytes, and there is no
JVM or Electron in sight.

The "production-ready vs DBeaver" gap is **not** about the core
loop. It is about:

- **breadth of drivers** (we have 5; DBeaver claims ~80),
- **write-side ergonomics** (CRUD-from-grid, BLOB editing, JSON
  trees, geometry visualisation),
- **persisted query / table-state management** (saved queries,
  bookmarks, recent-files, ER-diagram exports as PNG/SVG),
- **server-side metadata depth** (triggers, sequences, functions,
  materialised views, partitions, tablespaces),
- **collaboration / DBA tools** (Erwin/PowerDesigner imports,
  Liquibase changelogs, session monitor, lock viewer).

We are not chasing DBeaver-Ultimate territory. We are chasing
"DBeaver Community on Postgres / MySQL / SQLite / MSSQL / Oracle,
but launches in 50 ms and lives inside `tmux`."

---

## Drivers

| Driver                | State                | Notes |
| --------------------- | -------------------- | ----- |
| PostgreSQL            | ✅ Stable             | Full metadata (cols, indexes, PK, FK, CHECK). `sqlx` pure-Rust + `rustls`. |
| SQLite                | ✅ Stable             | PRAGMA-driven introspection. `:memory:` + file URLs. |
| MySQL / MariaDB       | ✅ Stable             | `information_schema`; CHECK on 8.0+ / MariaDB 10.2+. |
| MS SQL Server         | 🟡 Active             | `tiberius` + `bb8-tiberius`; native TLS (SecureTransport / SChannel / OpenSSL). Tests gated on `MSSQL_URL`. Missing: TVPs, table-valued returns, MSSQL-specific introspection (extended properties, columnstore indexes). |
| Oracle (opt-in)       | 🟡 Active             | `--features oracle`; requires Oracle Instant Client 12.1+. Missing: PL/SQL package browser, server output capture, partition introspection. |
| DuckDB                | ❌ Not started        | Common ask; `duckdb-rs` would be a natural fit. |
| ClickHouse            | ❌ Not started        | `clickhouse-rs` exists; non-trivial because of column-store semantics. |
| Snowflake / Redshift  | ❌ Not started        | Cloud warehouses; would need OAuth flows. |

**Recommended next driver:** DuckDB. Single dependency, in-process,
huge analyst userbase, zero TLS complexity, and `duckdb-rs` is well
maintained. Cost: a few days plus introspection mapping.

## TUI surface

| Surface                 | State           | Notes |
| ----------------------- | --------------- | ----- |
| Connection picker       | ✅              | `~/.config/tsqlx/config.toml`, paste-URL flow, name+save, hash-named ad-hoc URLs. |
| Sidebar (schemas → tables) | ✅           | Per-driver introspection, lazy expand, `/` filter (Unreleased), `:select schema.table` palette. |
| Records grid            | ✅              | Zebra striping, horizontal column scroll, `]` / `[`, `y` cell, `Y` row TSV, `/` row filter. |
| Detail tabs             | ✅              | Records / Columns / Indexes / Keys / Constraints / ERD — `l/h` cycle, `1`–`6` jump. |
| Focused ERD             | ✅              | Hand-rolled box-drawing canvas; PR #29 lifts the 8-column cap. |
| Whole-schema ERD canvas | 🟢 (PR #28)     | Sugiyama layered layout, three zoom levels, mouse drag pan, scroll-wheel zoom. |
| SQL editor              | ✅              | Persistent history, statement-aware run, `Ctrl+R` all / `Ctrl+Enter` under cursor, `:w`. |
| Bracketed-paste multi-line | ✅           | Whole script in one `Event::Paste`. |
| Vertical-scroll editor  | ✅              | Cursor stays in viewport. |
| Theme switcher          | ✅              | 6 themes via `Ctrl+T`, persisted to config. |
| System clipboard        | ✅              | `arboard` lazy-init; status-bar fallback on headless. |
| Mouse capture           | 🟢 (PR #28, ERD canvas only) | Disabled outside canvas so terminal selection survives. |
| Autocomplete (SQL)      | ❌              | Roadmap. Per-keyword + per-schema-identifier completion is feasible but needs a parser-aware cursor model. |
| Syntax highlighting in editor | partial   | `editor::highlight_line` already exists. Multi-line SQL highlighting (string spans, comments) is rough. |
| Query plan viewer (`EXPLAIN`) | ❌        | DBeaver shows a tree. We'd need driver-specific plan parsing. Postgres' `EXPLAIN (FORMAT JSON)` is easy; MSSQL's is XML. |
| Saved queries / snippets | ❌             | DBeaver's "SQL Editor → Save". We have history but no named snippets. |
| Bookmarks / favourites  | ❌              | Star a table + jump straight to it. |
| Result-set tabs (multi-query) | ❌        | DBeaver opens each run in its own tab. We replace. |
| Inline cell edit / row insert / delete | ❌ | Read-only grid today. Significant scope — needs PK detection, optimistic locking, transaction display. |
| Data export (CSV / JSON / SQL INSERTs) | ❌ | We yank cell/row to clipboard. A `:export` palette + file picker is the obvious shape. |
| Import (CSV → table)    | ❌              | Per-driver `COPY` / `LOAD DATA` differences make this work. |

## ERD specifically (what the user asked about)

| Item                                  | Before PR stack | After PR stack |
| ------------------------------------- | --------------- | -------------- |
| Show all columns of focused table     | ❌ 8-col cap     | ✅ PR #29       |
| Per-card column scroll                | ❌              | ✅ PR #29 (`J`/`K`) |
| Whole-schema view                     | ❌ focused only  | ✅ PR #28       |
| Layered layout (parents L, children R) | ❌             | ✅ PR #28       |
| Pan keyboard                          | ❌              | ✅ PR #28 (hjkl + arrows + H/L) |
| Pan mouse drag                        | ❌ no mouse      | ✅ PR #28       |
| Scroll-wheel zoom                     | ❌              | ✅ PR #28       |
| Three zoom levels                     | ❌              | ✅ PR #28 (Collapsed / Compact / Full) |
| Per-edge lane allocation              | ❌ single mid-x  | ✅ PR #28       |
| Cycle-tolerant layout                 | n/a              | ✅ PR #28 (cycle break) |
| Centre-on-selected (`c`)              | n/a              | partial (resets to 0,0; placements surfaced for future use) |
| Back-edge routing for cycles          | ❌              | ❌ silently skipped — would need Manhattan A* around obstacles. |
| Mermaid export                        | ✅              | ✅              |
| PNG / SVG export                      | ❌              | ❌ Out of scope for a TUI; user can pipe `.mmd` through `mmdc`. |
| Manual drag-to-reposition card        | ❌              | ❌ Would conflict with the auto-layout invariant. Could be added as a `let user pin x,y` overlay. |

## Editor / SQL execution

| Item | State | Notes |
| --- | --- | --- |
| Per-connection persistent history | ✅ | `editor::history_path`. |
| Statement-aware run under cursor | ✅ | Dollar-quoted blocks, semicolons in comments, etc. handled. |
| Multi-statement run | ✅ | `Ctrl+R`. |
| Error display | partial | Status bar + `last_error`. No inline squiggle. |
| Autocomplete | ❌ | Big feature; needs schema-aware completion engine. |
| Format on save | ❌ | `sqlformatter`/`sleek` integration would be one command away. |
| Live-error / lint | ❌ | Could plug into a parser like `pg_query_rs`. |

## DBA / observability

| Item | State |
| --- | --- |
| Active sessions / kill query | ❌ |
| Lock viewer | ❌ |
| Trigger browser | ❌ |
| Function / procedure browser | ❌ |
| Sequence browser | ❌ |
| Materialised view refresh / inspect | ❌ |
| Backup / restore wizards | ❌ (out of scope) |
| Audit log of executed statements | partial (history) |

## Packaging / distribution

| Item | State |
| --- | --- |
| `cargo install tsqlx` | ✅ |
| Static Linux binary release | ❌ (would need a release workflow + cross-compile) |
| Homebrew tap | ❌ |
| MSI / scoop / winget | ❌ (no Windows binary release yet) |
| Reproducible build artefacts | ❌ |
| Pre-built artefacts in GH releases | ❌ |

## CI / quality gates (as of `main`)

- ✅ `cargo build --workspace` clean.
- ✅ `cargo test --workspace --lib` — 32 lib tests on `main`; **51** after PR #27 → PR #29.
- ✅ `cargo clippy --workspace --all-targets -- -D warnings` clean.
- ✅ `cargo fmt -- --check` clean.
- ✅ `cargo audit --no-fetch` clean (RustSec advisory DB pre-cloned).
- ✅ Dependabot wired (workspace bumps land via PR).
- 🟡 Integration tests gated on `POSTGRES_URL` / `MSSQL_URL` / `ORACLE_URL` env vars — they exist (`crates/tsqlx-db/tests/*.rs`) but only run when those env vars point at live DBs. Local Podman fixtures would let us run them in CI.
- ❌ Snapshot-style TUI tests (e.g. `insta` against ratatui buffers) — not yet.

## Recommended priority order for the next release window

1. **Land the in-flight ERD stack** (#27 → #28 → #29). Closes the
   biggest user-visible gap raised in the most recent feedback.
2. **DuckDB driver.** Easy win, big audience overlap (analysts).
3. **Inline cell edit / row insert / delete on the records grid.**
   This is what "production-ready" means to most desktop users.
   Requires PK detection (we already have it) and an
   optimistic-update flow.
4. **CSV / JSON / SQL INSERT export.** Cheap to ship and frequently
   requested.
5. **Pre-built static Linux binary in GH Releases.** Removes the
   `rustup` / `build-essential` friction from `cargo install`.
6. **Saved queries / snippets** with a `:save <name>` palette.
7. **SQL autocomplete** (schema-aware, keyword + identifier). Big
   project; do it last so the spec is informed by everything else.

Items deliberately **not** on this list because they trade
simplicity for breadth and we'd rather stay tight:

- Liquibase / Flyway changelog generation
- ERwin / PowerDesigner imports
- Cassandra / MongoDB / Redis / Elasticsearch drivers
- Visual query builder
- Tab groups, perspectives, layouts persistence

---

## Notes for the next contributor

- The TUI is one big file (`crates/tsqlx-tui/src/lib.rs`, ~4500
  lines). PR #27 starts the slow extraction by splitting `erd/`
  out. Continue that pattern — pull out `editor/` next (already a
  module, but only for highlight + history), then `records/` and
  `sidebar/`.
- Driver code lives in `crates/tsqlx-db/src/{lib,mssql,oracle}.rs`.
  PR a new driver as a separate file plus a `Pool` variant; reuse
  the `DatabaseOverview` / `TableInfo` shape so the TUI doesn't
  notice.
- ERD primitives are pure; new visualisations (e.g. a column-level
  dependency graph) can be built on top of `erd/primitives.rs` and
  `erd/layout.rs` without touching `AppState`.
