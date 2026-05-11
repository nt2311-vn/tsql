use std::collections::{HashMap, HashSet};
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::Duration;

mod editor;
use editor::{append_history, highlight_line, history_path, load_history, statement_range_at};

use anyhow::Result;
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, List, ListItem, ListState, Paragraph, Row, Table, TableState,
    Tabs, Wrap,
};
use ratatui::{Frame, Terminal};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tsqlx_core::{
    append_connection, default_config_path, set_editor_theme, ConnectionConfig, DriverKind,
};
use tsqlx_db::{DatabaseOverview, Pool, RelationshipEdge, StatementOutput, TableInfo};
use tsqlx_sql::SqlDocument;

// ─── Background DB messages ──────────────────────────────────────────────────
//
// Metadata loads run on tokio tasks so the event loop can keep drawing and
// processing input while a slow database is responding. Tasks send their
// results back through this channel; `run_loop` drains the receiver between
// every key poll and applies the messages to `AppState`.

#[derive(Debug)]
enum DbMessage {
    TableInfo {
        schema: String,
        table: String,
        result: Result<TableInfo, String>,
    },
    Records {
        schema: String,
        table: String,
        offset: usize,
        result: Result<StatementOutput, String>,
    },
    Relationships {
        schema: String,
        result: Result<Vec<RelationshipEdge>, String>,
    },
    /// Pre-fetch of one schema-scoped `TableInfo` populated for the
    /// ERD card view. Independent of the active table so it does not
    /// race with `TableInfo` for the foreground view.
    ErdTableInfo {
        schema: String,
        table: String,
        result: Result<TableInfo, String>,
    },
}

// ─── Theme ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub name: &'static str,
    pub bg: Color,
    pub fg: Color,
    pub accent: Color,
    pub accent2: Color,
    pub success: Color,
    pub error: Color,
    pub warning: Color,
    pub muted: Color,
    pub sel_bg: Color,
    pub sel_fg: Color,
    pub border: Color,
    pub active_border: Color,
    /// Alternating row background for the records grid (zebra striping).
    /// Sits between `bg` and `sel_bg` in lightness.
    pub row_alt_bg: Color,
}

impl Theme {
    #[must_use]
    pub const fn catppuccin_mocha() -> Self {
        Self {
            name: "catppuccin-mocha",
            bg: Color::Rgb(30, 30, 46),
            fg: Color::Rgb(205, 214, 244),
            accent: Color::Rgb(137, 180, 250),
            accent2: Color::Rgb(203, 166, 247),
            success: Color::Rgb(166, 227, 161),
            error: Color::Rgb(243, 139, 168),
            warning: Color::Rgb(249, 226, 175),
            muted: Color::Rgb(108, 112, 134),
            sel_bg: Color::Rgb(69, 71, 90),
            sel_fg: Color::Rgb(205, 214, 244),
            border: Color::Rgb(69, 71, 90),
            active_border: Color::Rgb(137, 180, 250),
            row_alt_bg: Color::Rgb(36, 36, 56),
        }
    }

    #[must_use]
    pub const fn catppuccin_macchiato() -> Self {
        Self {
            name: "catppuccin-macchiato",
            bg: Color::Rgb(36, 39, 58),
            fg: Color::Rgb(202, 211, 245),
            accent: Color::Rgb(138, 173, 244),
            accent2: Color::Rgb(198, 160, 246),
            success: Color::Rgb(166, 218, 149),
            error: Color::Rgb(237, 135, 150),
            warning: Color::Rgb(238, 212, 159),
            muted: Color::Rgb(110, 115, 141),
            sel_bg: Color::Rgb(73, 77, 100),
            sel_fg: Color::Rgb(202, 211, 245),
            border: Color::Rgb(73, 77, 100),
            active_border: Color::Rgb(138, 173, 244),
            row_alt_bg: Color::Rgb(42, 45, 66),
        }
    }

    #[must_use]
    pub const fn catppuccin_frappe() -> Self {
        Self {
            name: "catppuccin-frappe",
            bg: Color::Rgb(48, 52, 70),
            fg: Color::Rgb(198, 208, 245),
            accent: Color::Rgb(140, 170, 238),
            accent2: Color::Rgb(202, 158, 230),
            success: Color::Rgb(166, 209, 137),
            error: Color::Rgb(231, 130, 132),
            warning: Color::Rgb(229, 200, 144),
            muted: Color::Rgb(115, 121, 148),
            sel_bg: Color::Rgb(81, 87, 109),
            sel_fg: Color::Rgb(198, 208, 245),
            border: Color::Rgb(81, 87, 109),
            active_border: Color::Rgb(140, 170, 238),
            row_alt_bg: Color::Rgb(55, 59, 80),
        }
    }

    #[must_use]
    pub const fn catppuccin_latte() -> Self {
        Self {
            name: "catppuccin-latte",
            bg: Color::Rgb(239, 241, 245),
            fg: Color::Rgb(76, 79, 105),
            accent: Color::Rgb(30, 102, 245),
            accent2: Color::Rgb(136, 57, 239),
            success: Color::Rgb(64, 160, 43),
            error: Color::Rgb(210, 15, 57),
            warning: Color::Rgb(223, 142, 29),
            muted: Color::Rgb(140, 143, 161),
            sel_bg: Color::Rgb(204, 208, 218),
            sel_fg: Color::Rgb(76, 79, 105),
            border: Color::Rgb(188, 192, 204),
            active_border: Color::Rgb(30, 102, 245),
            row_alt_bg: Color::Rgb(230, 233, 239),
        }
    }

    #[must_use]
    pub const fn tokyo_night() -> Self {
        Self {
            name: "tokyo-night",
            bg: Color::Rgb(26, 27, 38),
            fg: Color::Rgb(192, 202, 245),
            accent: Color::Rgb(122, 162, 247),
            accent2: Color::Rgb(187, 154, 247),
            success: Color::Rgb(158, 206, 106),
            error: Color::Rgb(247, 118, 142),
            warning: Color::Rgb(224, 175, 104),
            muted: Color::Rgb(86, 95, 137),
            sel_bg: Color::Rgb(40, 52, 87),
            sel_fg: Color::Rgb(192, 202, 245),
            border: Color::Rgb(59, 66, 97),
            active_border: Color::Rgb(122, 162, 247),
            row_alt_bg: Color::Rgb(31, 35, 53),
        }
    }

    #[must_use]
    pub const fn gruvbox_dark() -> Self {
        Self {
            name: "gruvbox-dark",
            bg: Color::Rgb(40, 40, 40),
            fg: Color::Rgb(235, 219, 178),
            accent: Color::Rgb(131, 165, 152),
            accent2: Color::Rgb(211, 134, 155),
            success: Color::Rgb(184, 187, 38),
            error: Color::Rgb(251, 73, 52),
            warning: Color::Rgb(250, 189, 47),
            muted: Color::Rgb(146, 131, 116),
            sel_bg: Color::Rgb(60, 56, 54),
            sel_fg: Color::Rgb(235, 219, 178),
            border: Color::Rgb(80, 73, 69),
            active_border: Color::Rgb(250, 189, 47),
            row_alt_bg: Color::Rgb(50, 48, 47),
        }
    }

    /// All themes shipped with tsqlx, in `Ctrl+T` cycle order.
    #[must_use]
    pub fn all() -> &'static [fn() -> Self] {
        const ALL: &[fn() -> Theme] = &[
            Theme::catppuccin_mocha,
            Theme::catppuccin_macchiato,
            Theme::catppuccin_frappe,
            Theme::catppuccin_latte,
            Theme::tokyo_night,
            Theme::gruvbox_dark,
        ];
        ALL
    }

    /// Lookup by config name; unknown names fall back to the default theme.
    #[must_use]
    pub fn by_name(name: &str) -> Self {
        Self::all()
            .iter()
            .map(|f| f())
            .find(|t| t.name == name)
            .unwrap_or_else(Self::catppuccin_mocha)
    }

    /// Next theme in the cycle.
    #[must_use]
    pub fn next_in_cycle(self) -> Self {
        let all = Self::all();
        let idx = all
            .iter()
            .map(|f| f())
            .position(|t| t.name == self.name)
            .unwrap_or(0);
        all[(idx + 1) % all.len()]()
    }
}

// ─── Mode / pane / tab enums ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppMode {
    Connect,
    Browser,
    Editor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectFocus {
    Picker,
    NewUrl,
    /// Prompt for a friendly name after a successful new-URL connect, so
    /// the connection can be persisted to `~/.config/tsqlx/config.toml`.
    /// Empty input + Enter (or Esc) skips saving.
    NameNew,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserPane {
    Sidebar,
    Detail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailTab {
    Records,
    Columns,
    Indexes,
    Keys,
    Constraints,
    Erd,
}

const ALL_TABS: &[DetailTab] = &[
    DetailTab::Records,
    DetailTab::Columns,
    DetailTab::Indexes,
    DetailTab::Keys,
    DetailTab::Constraints,
    DetailTab::Erd,
];

impl DetailTab {
    fn label(self) -> &'static str {
        match self {
            Self::Records => "Records",
            Self::Columns => "Columns",
            Self::Indexes => "Indexes",
            Self::Keys => "Keys",
            Self::Constraints => "Constraints",
            Self::Erd => "ERD",
        }
    }

    /// Tab label prefixed with its 1-based hotkey, e.g. `"1 Records"`.
    /// The number matches the `1`-`6` keyboard shortcut, so users can
    /// see which key jumps where.
    fn hotkey_label(self) -> String {
        format!("{} {}", self.index() + 1, self.label())
    }

    fn index(self) -> usize {
        ALL_TABS.iter().position(|t| *t == self).unwrap_or(0)
    }

    fn next(self) -> Self {
        ALL_TABS[(self.index() + 1) % ALL_TABS.len()]
    }

    fn prev(self) -> Self {
        ALL_TABS[(self.index() + ALL_TABS.len() - 1) % ALL_TABS.len()]
    }
}

// ─── Sidebar tree entries ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum SidebarEntry {
    Schema { name: String, expanded: bool },
    Table { schema: String, name: String },
}

impl SidebarEntry {
    fn display(&self) -> String {
        match self {
            Self::Schema { name, expanded } => {
                format!("{}  {}", if *expanded { "▼" } else { "▶" }, name)
            }
            Self::Table { name, .. } => format!("    {}  {}", "└", name),
        }
    }
}

// ─── Application state ────────────────────────────────────────────────────────

struct AppState {
    driver: DriverKind,
    url: String,
    pool: Option<Pool>,
    saved_connections: Vec<(String, ConnectionConfig)>,
    connect_idx: usize,
    connect_focus: ConnectFocus,
    connect_input: String,
    /// URL of the just-connected ad-hoc session, held while we prompt the
    /// user for a name to persist it under. `None` outside the name flow.
    pending_save: Option<(DriverKind, String)>,
    /// Override for the config file path (test hook). When `None` we use
    /// `tsqlx_core::default_config_path()`.
    config_path_override: Option<PathBuf>,
    theme: Theme,
    mode: AppMode,
    pane: BrowserPane,
    sidebar: Vec<SidebarEntry>,
    sidebar_idx: usize,
    sidebar_list_state: ListState,
    overview: Option<DatabaseOverview>,
    detail_tab: DetailTab,
    current_schema: String,
    current_table: String,
    table_info: Option<TableInfo>,
    records: Option<StatementOutput>,
    record_offset: usize,
    record_row: usize,
    record_col: usize,
    /// First visible record column. `Cell` so `draw_records` can slide
    /// the horizontal window without taking `&mut`. Reset on each
    /// table load so column counts changing don't leave a stale offset.
    record_col_offset: std::cell::Cell<usize>,
    record_table_state: TableState,
    relationships: Vec<RelationshipEdge>,
    /// Row index into `relationships` selected on the ERD tab. Used by
    /// j/k navigation and the Enter / `o` jump-to-table shortcuts.
    erd_selected: usize,
    /// Schema-scoped `TableInfo` cache feeding the ERD inspector +
    /// Mermaid generator. Cleared on schema change. Tables that have
    /// not yet been pre-fetched render as `(loading…)` stubs.
    erd_table_info: HashMap<String, TableInfo>,
    /// When true the ERD chart pane fills the whole detail body —
    /// banner stays, but the table list + per-table inspector hide.
    /// Toggled with `f` while on the ERD tab.
    erd_chart_fullscreen: bool,
    editor: String,
    /// Byte index of the cursor within `editor`. Always sits on a UTF-8
    /// char boundary.
    editor_cursor: usize,
    /// First visible line in the editor pane (vertical scroll offset).
    /// Stored in a `Cell` so `draw_editor` can adjust it on the fly to
    /// keep the cursor inside the viewport without taking `&mut self`.
    editor_scroll: std::cell::Cell<usize>,
    /// Optional file backing the editor buffer (`Ctrl+S` writes here,
    /// `:w <path>` retargets it). `None` means in-memory only.
    editor_path: Option<PathBuf>,
    /// On-disk history file path for the active connection. Computed
    /// after a successful connect so each saved connection (and
    /// hash-named ad-hoc URL) gets its own history.
    history_path: Option<PathBuf>,
    /// Last `MAX_HISTORY` successfully-submitted queries, newest last.
    history: Vec<String>,
    /// Index into `history` while the user is browsing it with
    /// Ctrl+P/Ctrl+N. `None` means the editor holds a fresh draft.
    history_idx: Option<usize>,
    status: String,
    last_error: Option<String>,
    /// Channel used by spawned db tasks to send results back to the event
    /// loop. `Option` because it is wired up in `run`/`run_connect` after
    /// state construction.
    tx: Option<UnboundedSender<DbMessage>>,
    /// Number of in-flight metadata loads. Drives the spinner/status bar.
    pending: usize,
    /// Command palette buffer. `Some(":select")` when the user has pressed
    /// `:` in Browser mode and is typing a command; `None` otherwise. The
    /// status bar swaps for a `:`-prefixed prompt while this is `Some`.
    command_input: Option<String>,
}

impl AppState {
    fn new(driver: DriverKind, url: String) -> Self {
        let mut ls = ListState::default();
        ls.select(Some(0));
        let mut ts = TableState::default();
        ts.select(Some(0));
        Self {
            driver,
            connect_input: url.clone(),
            url,
            pool: None,
            saved_connections: Vec::new(),
            connect_idx: 0,
            connect_focus: ConnectFocus::Picker,
            pending_save: None,
            config_path_override: None,
            theme: Theme::catppuccin_mocha(),
            mode: AppMode::Browser,
            pane: BrowserPane::Sidebar,
            sidebar: Vec::new(),
            sidebar_idx: 0,
            sidebar_list_state: ls,
            overview: None,
            detail_tab: DetailTab::Records,
            current_schema: String::new(),
            current_table: String::new(),
            table_info: None,
            records: None,
            record_offset: 0,
            record_row: 0,
            record_col: 0,
            record_col_offset: std::cell::Cell::new(0),
            record_table_state: ts,
            relationships: Vec::new(),
            erd_selected: 0,
            erd_table_info: HashMap::new(),
            erd_chart_fullscreen: false,
            editor: String::new(),
            editor_cursor: 0,
            editor_scroll: std::cell::Cell::new(0),
            editor_path: None,
            history_path: None,
            history: Vec::new(),
            history_idx: None,
            status: "Loading database overview…".to_owned(),
            last_error: None,
            tx: None,
            pending: 0,
            command_input: None,
        }
    }

    fn selected_cell(&self) -> Option<String> {
        self.records
            .as_ref()
            .and_then(|r| r.rows.get(self.record_row))
            .and_then(|row| row.get(self.record_col))
            .cloned()
    }

    fn selected_row_tsv(&self) -> Option<String> {
        self.records
            .as_ref()
            .and_then(|r| r.rows.get(self.record_row))
            .map(|row| row.join("\t"))
    }
}

// ─── Sidebar rebuild ──────────────────────────────────────────────────────────

fn rebuild_sidebar(app: &mut AppState, overview: &DatabaseOverview) {
    app.sidebar.clear();
    for schema in &overview.schemas {
        let expanded = app.current_schema == schema.name;
        app.sidebar.push(SidebarEntry::Schema {
            name: schema.name.clone(),
            expanded,
        });
        if expanded {
            for table in &schema.tables {
                app.sidebar.push(SidebarEntry::Table {
                    schema: schema.name.clone(),
                    name: table.clone(),
                });
            }
        }
    }
    let len = app.sidebar.len();
    if len > 0 {
        app.sidebar_idx = app.sidebar_idx.min(len - 1);
    } else {
        app.sidebar_idx = 0;
    }
    app.sidebar_list_state.select(Some(app.sidebar_idx));
}

// ─── Public entry point ───────────────────────────────────────────────────────

/// Startup customisation plumbed from `~/.config/tsqlx/config.toml`.
#[derive(Debug, Clone, Default)]
pub struct StartupOptions {
    /// Theme name from `[editor].theme` (e.g. `"catppuccin-mocha"`).
    /// `None` keeps the default theme.
    pub theme: Option<String>,
    /// Path to the active `config.toml`, used when persisting runtime
    /// theme changes. `None` disables persistence.
    pub config_path: Option<PathBuf>,
}

pub async fn run(driver: DriverKind, url: String) -> Result<()> {
    run_with_options(driver, url, StartupOptions::default()).await
}

pub async fn run_with_options(driver: DriverKind, url: String, opts: StartupOptions) -> Result<()> {
    let mut terminal = setup_terminal()?;
    let mut app = AppState::new(driver, url);
    if let Some(name) = opts.theme.as_deref() {
        app.theme = Theme::by_name(name);
    }
    app.config_path_override = opts.config_path;
    let (tx, rx) = unbounded_channel();
    app.tx = Some(tx);

    match open_pool_and_overview(&mut app).await {
        Ok(ov) => {
            rebuild_sidebar(&mut app, &ov);
            app.overview = Some(ov);
            wire_history(&mut app).await;
            app.status = nav_hint();
        }
        Err(e) => {
            app.last_error = Some(e.to_string());
            app.status = format!("error loading overview: {e}");
        }
    }

    let result = run_loop(&mut terminal, &mut app, rx).await;
    restore_terminal(&mut terminal)?;
    result
}

pub async fn run_connect(connections: Vec<(String, ConnectionConfig)>) -> Result<()> {
    run_connect_with_options(connections, StartupOptions::default()).await
}

pub async fn run_connect_with_options(
    connections: Vec<(String, ConnectionConfig)>,
    opts: StartupOptions,
) -> Result<()> {
    let mut terminal = setup_terminal()?;
    let mut app = AppState::new(DriverKind::Postgres, String::new());
    if let Some(name) = opts.theme.as_deref() {
        app.theme = Theme::by_name(name);
    }
    app.config_path_override = opts.config_path;
    let (tx, rx) = unbounded_channel();
    app.tx = Some(tx);
    app.mode = AppMode::Connect;
    app.saved_connections = connections;
    app.connect_input = String::new();
    if app.saved_connections.is_empty() {
        app.connect_focus = ConnectFocus::NewUrl;
        app.status = "Paste connection URL  Tab toggle driver  Enter connect  q quit".to_owned();
    } else {
        app.connect_focus = ConnectFocus::Picker;
        app.status = "j/k navigate  Enter connect  n new connection  q quit".to_owned();
    }
    let result = run_loop(&mut terminal, &mut app, rx).await;
    restore_terminal(&mut terminal)?;
    result
}

fn nav_hint() -> String {
    "j/k nav  l/Enter expand  h collapse  Tab pane  1-6 tabs  X close  e editor  q quit".to_owned()
}

// ─── Event loop ───────────────────────────────────────────────────────────────

async fn run_loop(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<Stdout>>,
    app: &mut AppState,
    mut rx: UnboundedReceiver<DbMessage>,
) -> Result<()> {
    loop {
        terminal.draw(|f| draw(f, app))?;

        // Drain any messages from background db tasks. try_recv is
        // non-blocking, so this stays cooperative with the input poll
        // below and we never stall the UI on a slow database.
        while let Ok(msg) = rx.try_recv() {
            apply_db_message(app, msg);
        }

        // Short poll keeps the UI responsive (~30 fps) while still letting
        // the runtime schedule background tasks.
        if !event::poll(Duration::from_millis(33))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if handle_key(app, key).await? => break,
            Event::Paste(text) => handle_paste(app, &text),
            _ => {}
        }
    }
    Ok(())
}

/// Bracketed-paste handler. Routes the pasted text to whichever buffer
/// is currently active so users can dump a multi-line SQL script into
/// the editor or paste a connection URL into the picker without losing
/// newlines or churning through the per-key insert path.
fn handle_paste(app: &mut AppState, text: &str) {
    // Strip the optional CR from CRLF terminators — pastes from Windows
    // clipboards or DOS-line files would otherwise leave stray `\r` in
    // the buffer that look like garbage in the gutter.
    let cleaned = text.replace("\r\n", "\n").replace('\r', "\n");
    match app.mode {
        AppMode::Editor => {
            editor_insert_str(app, &cleaned);
            app.history_idx = None;
            let lines = cleaned.matches('\n').count() + 1;
            app.status = format!("pasted {} chars / {} line(s)", cleaned.len(), lines);
        }
        AppMode::Connect
            if matches!(
                app.connect_focus,
                ConnectFocus::NewUrl | ConnectFocus::NameNew
            ) =>
        {
            // URL / name fields are single-line — collapse newlines.
            app.connect_input.push_str(&cleaned.replace('\n', ""));
        }
        _ => {
            if let Some(buf) = app.command_input.as_mut() {
                buf.push_str(&cleaned.replace('\n', " "));
            }
        }
    }
}

/// Apply the result of a background metadata fetch to `AppState`.
/// Stale messages (for a table the user has navigated away from) are
/// dropped so the UI never displays the wrong data.
fn apply_db_message(app: &mut AppState, msg: DbMessage) {
    app.pending = app.pending.saturating_sub(1);
    match msg {
        DbMessage::TableInfo {
            schema,
            table,
            result,
        } => {
            if schema != app.current_schema || table != app.current_table {
                return;
            }
            match result {
                Ok(info) => {
                    app.table_info = Some(info);
                    app.detail_tab = DetailTab::Records;
                    app.pane = BrowserPane::Detail;
                }
                Err(e) => {
                    app.last_error = Some(e.clone());
                    app.status = format!("error loading {table}: {e}");
                }
            }
        }
        DbMessage::Records {
            schema,
            table,
            offset,
            result,
        } => {
            if schema != app.current_schema
                || table != app.current_table
                || offset != app.record_offset
            {
                return;
            }
            match result {
                Ok(out) => {
                    let rows = out.rows.len();
                    app.records = Some(out);
                    app.record_row = 0;
                    app.record_table_state.select(Some(0));
                    app.status = format!(
                        " {schema}.{table}  offset {offset}  {rows} rows  \
                         j/k rows  [/] cols  y cell  Y row  l/h tabs"
                    );
                }
                Err(e) => {
                    app.last_error = Some(e.clone());
                    app.status = format!("error loading rows: {e}");
                }
            }
        }
        DbMessage::ErdTableInfo {
            schema,
            table,
            result,
        } => {
            if schema != app.current_schema {
                return;
            }
            if let Ok(info) = result {
                app.erd_table_info.insert(table, info);
            }
        }
        DbMessage::Relationships { schema, result } => {
            if schema != app.current_schema {
                return;
            }
            match result {
                Ok(mut rels) => {
                    // Stable sort by source table so flat j/k navigation
                    // matches the visual grouping in `draw_erd`.
                    rels.sort_by(|a, b| a.from_table.cmp(&b.from_table));
                    app.relationships = rels;
                    app.erd_selected = 0;
                    app.status = format!(
                        "ERD  schema: {schema}  {} relationship(s)  \
                         j/k select  Enter \u{2192} target  o \u{2192} source",
                        app.relationships.len()
                    );
                }
                Err(e) => {
                    app.status = format!("ERD error: {e}");
                }
            }
        }
    }
}

/// Advance to the next theme in the registry, persist it to the user's
/// `config.toml`, and surface the result in the status bar. A failed
/// save keeps the new theme for the session and reports the error.
async fn cycle_theme(app: &mut AppState) {
    let next = app.theme.next_in_cycle();
    app.theme = next;
    let path = app
        .config_path_override
        .clone()
        .unwrap_or_else(default_config_path);
    match set_editor_theme(&path, next.name).await {
        Ok(()) => app.status = format!("theme: {} (saved)", next.name),
        Err(e) => app.status = format!("theme: {} (save failed: {e})", next.name),
    }
}

async fn handle_key(app: &mut AppState, key: KeyEvent) -> Result<bool> {
    // Ctrl+C is always a hard quit, even from text-entry contexts.
    if (key.code, key.modifiers) == (KeyCode::Char('c'), KeyModifiers::CONTROL) {
        return Ok(true);
    }

    // The command palette steals all input while open.
    if app.command_input.is_some() {
        return handle_command_key(app, key).await;
    }

    // Ctrl+T cycles the theme from any mode and persists the choice.
    if (key.code, key.modifiers) == (KeyCode::Char('t'), KeyModifiers::CONTROL) {
        cycle_theme(app).await;
        return Ok(false);
    }

    match (key.code, key.modifiers) {
        // q quits from any non-text-entry mode
        (KeyCode::Char('q'), KeyModifiers::NONE)
            if app.mode != AppMode::Editor
                && !(app.mode == AppMode::Connect
                    && matches!(
                        app.connect_focus,
                        ConnectFocus::NewUrl | ConnectFocus::NameNew
                    )) =>
        {
            return Ok(true);
        }
        // `:` opens the command palette from Browser mode only. The editor
        // wants `:` to land in the buffer; the connect screen has its own
        // text entry.
        (KeyCode::Char(':'), KeyModifiers::NONE) if app.mode == AppMode::Browser => {
            app.command_input = Some(String::new());
            return Ok(false);
        }
        _ => {}
    }

    match app.mode {
        AppMode::Connect => handle_connect_key(app, key).await,
        AppMode::Editor => handle_editor_key(app, key).await,
        AppMode::Browser => handle_browser_key(app, key).await,
    }
}

/// Handle key input while the `:` command palette is open. Returns
/// `Ok(true)` only on `:q`/`:quit` to terminate the program.
async fn handle_command_key(app: &mut AppState, key: KeyEvent) -> Result<bool> {
    let buf = app
        .command_input
        .as_mut()
        .expect("command palette must be open");
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) => {
            app.command_input = None;
            app.status = nav_hint();
        }
        (KeyCode::Backspace, _) if buf.pop().is_none() => {
            // Backspace on an empty buffer closes the palette.
            app.command_input = None;
            app.status = nav_hint();
        }
        (KeyCode::Backspace, _) => {}
        (KeyCode::Enter, _) => {
            let cmd = buf.trim().to_owned();
            app.command_input = None;
            return run_command(app, &cmd).await;
        }
        (KeyCode::Char(ch), m) if !m.contains(KeyModifiers::CONTROL) => {
            buf.push(ch);
        }
        _ => {}
    }
    Ok(false)
}

/// Execute a command typed into the `:` palette. Unknown commands set
/// `last_error` and stay in Browser mode.
async fn run_command(app: &mut AppState, command: &str) -> Result<bool> {
    let cmd = command.trim();
    let (head, rest) = cmd.split_once(' ').unwrap_or((cmd, ""));
    let rest = rest.trim();
    match head {
        "" => {}
        "q" | "quit" => return Ok(true),
        "select" => prefill_editor_with_select(app),
        "insert" => prefill_editor_with_insert(app),
        "describe" | "desc" | "cols" | "columns" => switch_detail_tab(app, DetailTab::Columns),
        "indexes" | "idx" => switch_detail_tab(app, DetailTab::Indexes),
        "keys" => switch_detail_tab(app, DetailTab::Keys),
        "constraints" | "ck" => switch_detail_tab(app, DetailTab::Constraints),
        "erd" | "rel" | "relationships" => {
            switch_detail_tab(app, DetailTab::Erd);
            if app.relationships.is_empty() && !app.current_schema.is_empty() {
                let schema = app.current_schema.clone();
                spawn_relationships(app, schema);
            }
            spawn_erd_prefetch(app);
        }
        // File IO
        "w" | "write" | "save" => {
            let target = if rest.is_empty() {
                None
            } else {
                Some(PathBuf::from(rest))
            };
            save_editor_buffer(app, target).await;
        }
        "e" | "edit" | "open" => {
            if rest.is_empty() {
                app.last_error = Some("usage: :e <path>".to_owned());
                app.status = ":e needs a path".to_owned();
            } else {
                open_editor_buffer(app, PathBuf::from(rest)).await;
            }
        }
        "help" | "h" | "?" => {
            app.status = ":select :insert :describe :indexes :keys :constraints :erd \
                          :w [path] :e <path> :help :q"
                .to_owned();
        }
        other => {
            app.last_error = Some(format!("unknown command: :{other}  (try :help)"));
            app.status = format!("unknown command: :{other}");
        }
    }
    Ok(false)
}

fn editor_hint() -> String {
    "Ctrl+R run all  Ctrl+Enter run current  Ctrl+S save  Ctrl+O open  \
     Ctrl+P/N history  Esc browser"
        .to_owned()
}

/// Clear the active table selection so the detail pane returns to its
/// empty placeholder. Triggered by Shift+X in Browser mode.
fn close_current_table(app: &mut AppState) {
    app.current_table.clear();
    app.table_info = None;
    app.records = None;
    app.relationships.clear();
    app.record_offset = 0;
    app.record_row = 0;
    app.record_col = 0;
    app.record_col_offset.set(0);
    app.record_table_state.select(Some(0));
    app.pane = BrowserPane::Sidebar;
    app.detail_tab = DetailTab::Records;
    app.last_error = None;
    app.status = "table closed — pick another from the sidebar".to_owned();
}

fn switch_detail_tab(app: &mut AppState, tab: DetailTab) {
    if app.current_table.is_empty() {
        app.status = "select a table first".to_owned();
        return;
    }
    app.detail_tab = tab;
    app.pane = BrowserPane::Detail;
    app.status = format!("{}  ({})", tab.label(), app.current_table);
}

fn prefill_editor_with_select(app: &mut AppState) {
    if app.current_table.is_empty() {
        app.status = "select a table first".to_owned();
        return;
    }
    let qualified = qualified_table(app);
    app.editor = format!("SELECT * FROM {qualified} LIMIT 100;\n");
    app.editor_cursor = app.editor.len();
    app.history_idx = None;
    app.mode = AppMode::Editor;
    app.status = "editor: Ctrl+R run  Esc browser".to_owned();
}

fn prefill_editor_with_insert(app: &mut AppState) {
    if app.current_table.is_empty() {
        app.status = "select a table first".to_owned();
        return;
    }
    let qualified = qualified_table(app);
    let cols: Vec<String> = app
        .table_info
        .as_ref()
        .map(|info| info.columns.iter().map(|c| c.name.clone()).collect())
        .unwrap_or_default();
    let template = if cols.is_empty() {
        format!("INSERT INTO {qualified} VALUES (...);\n")
    } else {
        let names = cols.join(", ");
        let placeholders: Vec<String> = cols.iter().map(|c| format!(":{c}")).collect();
        format!(
            "INSERT INTO {qualified}\n  ({names})\nVALUES\n  ({});\n",
            placeholders.join(", ")
        )
    };
    app.editor = template;
    app.editor_cursor = app.editor.len();
    app.history_idx = None;
    app.mode = AppMode::Editor;
    app.status = "editor: replace placeholders, Ctrl+R run".to_owned();
}

/// Build a driver-appropriate qualified identifier. Sqlite has no schemas
/// outside `main`, so the schema prefix is dropped there to keep queries
/// portable.
fn qualified_table(app: &AppState) -> String {
    match app.driver {
        DriverKind::Postgres => format!("\"{}\".\"{}\"", app.current_schema, app.current_table),
        DriverKind::Sqlite => format!("\"{}\"", app.current_table),
        // MySQL identifiers use backticks; the schema is the active database.
        DriverKind::Mysql => format!("`{}`.`{}`", app.current_schema, app.current_table),
    }
}

async fn handle_connect_key(app: &mut AppState, key: KeyEvent) -> Result<bool> {
    match app.connect_focus {
        ConnectFocus::Picker => handle_picker_key(app, key).await,
        ConnectFocus::NewUrl => handle_new_url_key(app, key).await,
        ConnectFocus::NameNew => handle_name_new_key(app, key).await,
    }
}

async fn handle_picker_key(app: &mut AppState, key: KeyEvent) -> Result<bool> {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            let max = app.saved_connections.len().saturating_sub(1);
            app.connect_idx = (app.connect_idx + 1).min(max);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.connect_idx = app.connect_idx.saturating_sub(1);
        }
        KeyCode::Char('n') => {
            app.connect_focus = ConnectFocus::NewUrl;
            app.connect_input.clear();
            app.status = "Paste URL  Tab toggle driver  Enter connect  Esc back to list".to_owned();
        }
        KeyCode::Enter => {
            if let Some((_, conn)) = app.saved_connections.get(app.connect_idx).cloned() {
                app.driver = conn.driver;
                try_connect(app, conn.url).await;
            }
        }
        _ => {}
    }
    Ok(false)
}

async fn handle_new_url_key(app: &mut AppState, key: KeyEvent) -> Result<bool> {
    match key.code {
        KeyCode::Esc if !app.saved_connections.is_empty() => {
            app.connect_focus = ConnectFocus::Picker;
            app.status = "j/k navigate  Enter connect  n new connection  q quit".to_owned();
        }
        KeyCode::Tab => {
            // Cycle Postgres → SQLite → MySQL → Postgres so all drivers
            // are reachable from a single key with no shift / chord.
            app.driver = match app.driver {
                DriverKind::Postgres => DriverKind::Sqlite,
                DriverKind::Sqlite => DriverKind::Mysql,
                DriverKind::Mysql => DriverKind::Postgres,
            };
        }
        KeyCode::Backspace => {
            app.connect_input.pop();
        }
        KeyCode::Enter => {
            let url = app.connect_input.trim().to_owned();
            if url.is_empty() {
                app.status = "URL cannot be empty".to_owned();
                return Ok(false);
            }
            if let Ok(detected) = DriverKind::from_url(&url) {
                app.driver = detected;
            }
            try_connect(app, url).await;
        }
        KeyCode::Char(ch) => {
            app.connect_input.push(ch);
        }
        _ => {}
    }
    Ok(false)
}

async fn try_connect(app: &mut AppState, url: String) {
    let was_new_url = app.mode == AppMode::Connect && app.connect_focus == ConnectFocus::NewUrl;
    app.url = url;
    app.last_error = None;
    app.status = format!("Connecting to {}…", app.url);
    match open_pool_and_overview(app).await {
        Ok(ov) => {
            rebuild_sidebar(app, &ov);
            app.overview = Some(ov);
            // Wire up per-connection history. Saved-picker entries use
            // their config name; ad-hoc URLs fall back to a hashed label
            // until the user names them via NameNew.
            wire_history(app).await;
            // Ad-hoc URLs trigger a save prompt before dropping into the
            // browser. Saved-picker entries skip the prompt.
            if was_new_url {
                app.pending_save = Some((app.driver, app.url.clone()));
                app.connect_focus = ConnectFocus::NameNew;
                app.connect_input.clear();
                app.status = "Name this connection (Enter save, Esc skip)".to_owned();
            } else {
                app.mode = AppMode::Browser;
                app.status = nav_hint();
            }
        }
        Err(e) => {
            app.last_error = Some(e.to_string());
            app.status = format!("Connection failed: {e}");
        }
    }
}

/// Handle the post-connect 'name this connection' prompt. Enter with a
/// non-empty buffer persists to disk; Enter empty or Esc skips. Either
/// way we transition to Browser mode afterwards.
async fn handle_name_new_key(app: &mut AppState, key: KeyEvent) -> Result<bool> {
    match key.code {
        KeyCode::Esc => finish_save_prompt(app, None).await,
        KeyCode::Enter => {
            let name = app.connect_input.trim().to_owned();
            let chosen = if name.is_empty() { None } else { Some(name) };
            finish_save_prompt(app, chosen).await;
        }
        KeyCode::Backspace => {
            app.connect_input.pop();
        }
        KeyCode::Char(ch) if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' => {
            // Restrict to TOML-safe key chars so the user can't type a
            // name we'll have to reject on save.
            app.connect_input.push(ch);
        }
        _ => {}
    }
    Ok(false)
}

async fn finish_save_prompt(app: &mut AppState, name: Option<String>) {
    if let (Some(name), Some((driver, url))) = (name, app.pending_save.clone()) {
        let unique = unique_connection_name(&name, &app.saved_connections);
        let path = app
            .config_path_override
            .clone()
            .unwrap_or_else(default_config_path);
        let connection = ConnectionConfig {
            driver,
            url: url.clone(),
        };
        match append_connection(&path, &unique, &connection).await {
            Ok(()) => {
                app.saved_connections.push((unique.clone(), connection));
                app.saved_connections.sort_by(|a, b| a.0.cmp(&b.0));
                app.status = format!("saved connection '{unique}' to {}", path.display());
            }
            Err(e) => {
                app.last_error = Some(format!("could not save connection: {e}"));
                app.status = format!("save failed: {e}");
            }
        }
    } else {
        app.status = nav_hint();
    }
    app.pending_save = None;
    app.connect_input.clear();
    app.connect_focus = ConnectFocus::Picker;
    app.mode = AppMode::Browser;
}

/// If `desired` is already taken in the saved list, append `-2`, `-3`, …
/// until we land on a free key. The saved list is the source of truth
/// for in-memory state; the on-disk file may have additional entries
/// (created by other tsqlx sessions), but that's a corner case we accept.
fn unique_connection_name(desired: &str, saved: &[(String, ConnectionConfig)]) -> String {
    let taken: std::collections::HashSet<&str> = saved.iter().map(|(n, _)| n.as_str()).collect();
    if !taken.contains(desired) {
        return desired.to_owned();
    }
    for i in 2.. {
        let candidate = format!("{desired}-{i}");
        if !taken.contains(candidate.as_str()) {
            return candidate;
        }
    }
    unreachable!("infinite range exhausted")
}

/// Compute and load the on-disk history for the active connection. The
/// label is the saved-config name when the URL matches a known entry,
/// or a short hex hash of the URL otherwise. Entries are loaded into
/// `app.history` and capped at `MAX_HISTORY`.
async fn wire_history(app: &mut AppState) {
    let label = app
        .saved_connections
        .iter()
        .find(|(_, c)| c.url == app.url)
        .map(|(name, _)| name.clone())
        .unwrap_or_else(|| short_hash(&app.url));
    let path = history_path(&label);
    let loaded = load_history(&path, MAX_HISTORY).await;
    app.history = loaded;
    app.history_idx = None;
    app.history_path = Some(path);
}

/// 12-hex-char FNV-1a hash. Stable, deterministic, and never collides
/// with a saved-config name (which is restricted to alphanumerics).
fn short_hash(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("url_{:012x}", h & 0x0000_ffff_ffff_ffff)
}

/// Open a fresh `Pool` for the active driver/url and load the schema overview.
/// The pool is stored on `app.pool` so subsequent metadata calls reuse it.
async fn open_pool_and_overview(app: &mut AppState) -> Result<DatabaseOverview, tsqlx_db::DbError> {
    let pool = Pool::connect(app.driver, &app.url).await?;
    let overview = pool.fetch_overview().await?;
    app.pool = Some(pool);
    Ok(overview)
}

async fn handle_editor_key(app: &mut AppState, key: KeyEvent) -> Result<bool> {
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) => {
            app.mode = AppMode::Browser;
            app.status = nav_hint();
        }
        (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
            run_editor_query(app, RunScope::All).await;
        }
        // Ctrl+Enter (and Alt+Enter as a fallback for terminals that
        // don't deliver Ctrl+Enter distinctly) runs only the statement
        // under the cursor.
        (KeyCode::Enter, KeyModifiers::CONTROL) | (KeyCode::Enter, KeyModifiers::ALT) => {
            run_editor_query(app, RunScope::Current).await;
        }
        (KeyCode::Char('s'), KeyModifiers::CONTROL) => {
            save_editor_buffer(app, None).await;
        }
        // History recall (Ctrl+P / Ctrl+N). Up/Down stay free for line
        // navigation within multi-line queries.
        (KeyCode::Char('p'), KeyModifiers::CONTROL) => history_prev(app),
        (KeyCode::Char('n'), KeyModifiers::CONTROL) => history_next(app),
        // Cursor movement
        (KeyCode::Left, _) => {
            app.editor_cursor = prev_char_boundary(&app.editor, app.editor_cursor);
        }
        (KeyCode::Right, _) => {
            app.editor_cursor = next_char_boundary(&app.editor, app.editor_cursor);
        }
        (KeyCode::Up, _) => {
            app.editor_cursor = move_cursor_vertical(&app.editor, app.editor_cursor, -1);
        }
        (KeyCode::Down, _) => {
            app.editor_cursor = move_cursor_vertical(&app.editor, app.editor_cursor, 1);
        }
        (KeyCode::Home, _) | (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
            app.editor_cursor = line_start(&app.editor, app.editor_cursor);
        }
        (KeyCode::End, _) | (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
            app.editor_cursor = line_end(&app.editor, app.editor_cursor);
        }
        // Edits
        (KeyCode::Backspace, _) if app.editor_cursor > 0 => {
            let prev = prev_char_boundary(&app.editor, app.editor_cursor);
            app.editor.replace_range(prev..app.editor_cursor, "");
            app.editor_cursor = prev;
            app.history_idx = None;
        }
        (KeyCode::Delete, _) if app.editor_cursor < app.editor.len() => {
            let next = next_char_boundary(&app.editor, app.editor_cursor);
            app.editor.replace_range(app.editor_cursor..next, "");
            app.history_idx = None;
        }
        (KeyCode::Enter, _) => editor_insert_str(app, "\n"),
        (KeyCode::Tab, _) => editor_insert_str(app, "    "),
        (KeyCode::Char(ch), m) if !m.contains(KeyModifiers::CONTROL) => {
            let mut buf = [0u8; 4];
            let s = ch.encode_utf8(&mut buf);
            editor_insert_str(app, s);
        }
        _ => {}
    }
    Ok(false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunScope {
    All,
    Current,
}

async fn run_editor_query(app: &mut AppState, scope: RunScope) {
    let snippet = match scope {
        RunScope::All => app.editor.clone(),
        RunScope::Current => {
            let range = statement_range_at(&app.editor, app.editor_cursor);
            app.editor[range].to_owned()
        }
    };
    if snippet.trim().is_empty() {
        app.status = "nothing to run".to_owned();
        return;
    }
    app.last_error = None;
    app.status = match scope {
        RunScope::All => "executing all…".to_owned(),
        RunScope::Current => "executing current statement…".to_owned(),
    };
    let doc = SqlDocument::new(snippet.clone());
    let pool = app.pool.as_ref().expect("connected pool in editor mode");
    match pool.execute_script(&doc).await {
        Ok(out) => {
            if let Some(first) = out.statements.into_iter().next() {
                let rows = first.rows.len();
                app.records = Some(first);
                app.record_row = 0;
                app.record_col = 0;
                app.record_table_state.select(Some(0));
                app.status = format!("{rows} rows  Esc → browser");
            } else {
                app.status = "ok (no rows)  Esc → browser".to_owned();
            }
            push_history(app, snippet);
        }
        Err(e) => {
            app.last_error = Some(e.to_string());
            app.status = format!("error: {e}");
        }
    }
}

/// Save the editor buffer to disk. If `target` is `None`, write to the
/// existing `editor_path`. If both are missing, surface an error so the
/// user knows to use `:w <path>`.
async fn save_editor_buffer(app: &mut AppState, target: Option<PathBuf>) {
    let path = target.or_else(|| app.editor_path.clone());
    let Some(path) = path else {
        app.last_error = Some("no file: use :w <path> first".to_owned());
        app.status = "save: no file".to_owned();
        return;
    };
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                app.last_error = Some(format!("could not create dir: {e}"));
                app.status = format!("save failed: {e}");
                return;
            }
        }
    }
    match tokio::fs::write(&path, app.editor.as_bytes()).await {
        Ok(()) => {
            app.editor_path = Some(path.clone());
            app.status = format!("saved {}", path.display());
        }
        Err(e) => {
            app.last_error = Some(format!("save failed: {e}"));
            app.status = format!("save failed: {e}");
        }
    }
}

/// Open `path` and replace the editor buffer with its contents. Sets
/// `editor_path` so subsequent `Ctrl+S` writes back to the same file.
async fn open_editor_buffer(app: &mut AppState, path: PathBuf) {
    match tokio::fs::read_to_string(&path).await {
        Ok(text) => {
            app.editor = text;
            app.editor_cursor = app.editor.len().min(app.editor_cursor);
            app.editor_path = Some(path.clone());
            app.history_idx = None;
            app.mode = AppMode::Editor;
            app.status = format!("opened {}", path.display());
        }
        Err(e) => {
            app.last_error = Some(format!("open failed: {e}"));
            app.status = format!("open failed: {e}");
        }
    }
}

fn editor_insert_str(app: &mut AppState, s: &str) {
    app.editor.insert_str(app.editor_cursor, s);
    app.editor_cursor += s.len();
    app.history_idx = None;
}

const MAX_HISTORY: usize = 500;

fn push_history(app: &mut AppState, entry: String) {
    let trimmed = entry.trim().to_owned();
    if trimmed.is_empty() {
        return;
    }
    // De-duplicate adjacent: don't store the same query twice in a row.
    if app.history.last().map(String::as_str) == Some(trimmed.as_str()) {
        return;
    }
    app.history.push(trimmed.clone());
    if app.history.len() > MAX_HISTORY {
        let drop = app.history.len() - MAX_HISTORY;
        app.history.drain(..drop);
    }
    app.history_idx = None;
    if let Some(path) = app.history_path.clone() {
        // Best-effort: persist on a detached task so a slow disk doesn't
        // hold up the editor. Failures surface only as a missing entry
        // next session, which we accept.
        tokio::spawn(async move {
            let _ = append_history(&path, &trimmed).await;
        });
    }
}

fn history_prev(app: &mut AppState) {
    if app.history.is_empty() {
        return;
    }
    let next = match app.history_idx {
        None => app.history.len() - 1,
        Some(0) => 0,
        Some(i) => i - 1,
    };
    app.editor = app.history[next].clone();
    app.editor_cursor = app.editor.len();
    app.history_idx = Some(next);
    app.status = format!("history {}/{}", next + 1, app.history.len());
}

fn history_next(app: &mut AppState) {
    let Some(idx) = app.history_idx else {
        return;
    };
    if idx + 1 >= app.history.len() {
        // Step past the newest entry → blank draft.
        app.editor.clear();
        app.editor_cursor = 0;
        app.history_idx = None;
        app.status = "history: new draft".to_owned();
    } else {
        let next = idx + 1;
        app.editor = app.history[next].clone();
        app.editor_cursor = app.editor.len();
        app.history_idx = Some(next);
        app.status = format!("history {}/{}", next + 1, app.history.len());
    }
}

// ─── Editor cursor helpers ────────────────────────────────────────────────────

fn prev_char_boundary(s: &str, mut idx: usize) -> usize {
    if idx == 0 {
        return 0;
    }
    idx -= 1;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn next_char_boundary(s: &str, mut idx: usize) -> usize {
    let len = s.len();
    if idx >= len {
        return len;
    }
    idx += 1;
    while idx < len && !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

fn line_start(s: &str, idx: usize) -> usize {
    s[..idx].rfind('\n').map_or(0, |p| p + 1)
}

fn line_end(s: &str, idx: usize) -> usize {
    s[idx..].find('\n').map_or(s.len(), |p| idx + p)
}

/// Compute (line, column-in-chars) for a byte index.
fn line_col(s: &str, idx: usize) -> (usize, usize) {
    let prefix = &s[..idx];
    let line = prefix.bytes().filter(|b| *b == b'\n').count();
    let col = prefix
        .rsplit_once('\n')
        .map_or(prefix, |(_, after)| after)
        .chars()
        .count();
    (line, col)
}

/// Move cursor up (delta = -1) or down (delta = 1) one line, preserving
/// the visual column where possible.
fn move_cursor_vertical(s: &str, idx: usize, delta: isize) -> usize {
    let (line, col) = line_col(s, idx);
    let target_line = match delta {
        d if d < 0 && line == 0 => return idx,
        d if d < 0 => line - 1,
        _ => line + 1,
    };
    let lines: Vec<&str> = s.split('\n').collect();
    if target_line >= lines.len() {
        return idx;
    }
    // Build byte offset of target_line's start.
    let mut offset = 0usize;
    for line_text in lines.iter().take(target_line) {
        offset += line_text.len() + 1; // +1 for the '\n'
    }
    let target = lines[target_line];
    // Walk char-by-char on the target line up to `col` chars.
    let mut byte = 0usize;
    for (i, ch) in target.char_indices() {
        let chars_so_far = target[..i].chars().count();
        if chars_so_far >= col {
            return offset + i;
        }
        byte = i + ch.len_utf8();
    }
    offset + byte
}

async fn handle_browser_key(app: &mut AppState, key: KeyEvent) -> Result<bool> {
    match (key.code, key.modifiers) {
        (KeyCode::Tab, _) => {
            app.pane = match app.pane {
                BrowserPane::Sidebar => BrowserPane::Detail,
                BrowserPane::Detail => BrowserPane::Sidebar,
            };
        }
        (KeyCode::Char('e'), _) | (KeyCode::Char('i'), _) => {
            app.mode = AppMode::Editor;
            app.status = editor_hint();
        }
        // Shift+X closes the active table and returns the user to the
        // empty-detail placeholder so they can pick another table without
        // collapsing the schema first.
        (KeyCode::Char('X'), _) => {
            close_current_table(app);
        }
        // Number keys jump straight to a detail tab. Mirrors the
        // l/h cycling but lets the user land on any tab in one keystroke.
        (KeyCode::Char(ch), _) if matches!(ch, '1'..='6') => {
            let tab = match ch {
                '1' => DetailTab::Records,
                '2' => DetailTab::Columns,
                '3' => DetailTab::Indexes,
                '4' => DetailTab::Keys,
                '5' => DetailTab::Constraints,
                _ => DetailTab::Erd,
            };
            switch_detail_tab(app, tab);
            if tab == DetailTab::Erd
                && app.relationships.is_empty()
                && !app.current_schema.is_empty()
            {
                let schema = app.current_schema.clone();
                spawn_relationships(app, schema);
            }
            if app.detail_tab == DetailTab::Erd {
                spawn_erd_prefetch(app);
            }
        }
        _ => match app.pane {
            BrowserPane::Sidebar => {
                sidebar_key(app, key).await?;
            }
            BrowserPane::Detail => {
                detail_key(app, key).await?;
            }
        },
    }
    Ok(false)
}

async fn sidebar_key(app: &mut AppState, key: KeyEvent) -> Result<bool> {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            let max = app.sidebar.len().saturating_sub(1);
            app.sidebar_idx = (app.sidebar_idx + 1).min(max);
            app.sidebar_list_state.select(Some(app.sidebar_idx));
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.sidebar_idx = app.sidebar_idx.saturating_sub(1);
            app.sidebar_list_state.select(Some(app.sidebar_idx));
        }
        KeyCode::Char('g') => {
            app.sidebar_idx = 0;
            app.sidebar_list_state.select(Some(0));
        }
        KeyCode::Char('G') => {
            let last = app.sidebar.len().saturating_sub(1);
            app.sidebar_idx = last;
            app.sidebar_list_state.select(Some(last));
        }
        KeyCode::Enter | KeyCode::Char('l') => {
            if let Some(entry) = app.sidebar.get(app.sidebar_idx).cloned() {
                match entry {
                    SidebarEntry::Schema { name, expanded } => {
                        for e in &mut app.sidebar {
                            if let SidebarEntry::Schema {
                                expanded: ex,
                                name: en,
                            } = e
                            {
                                *ex = !expanded && *en == name;
                            }
                        }
                        app.current_schema = if expanded { String::new() } else { name };
                        if let Some(ov) = app.overview.clone() {
                            rebuild_sidebar(app, &ov);
                        }
                    }
                    SidebarEntry::Table { schema, name } => {
                        app.current_schema = schema.clone();
                        app.current_table = name.clone();
                        app.relationships.clear();
                        app.last_error = None;
                        app.status = format!("loading {schema}.{name}…");
                        load_table(app, &schema.clone(), &name.clone()).await;
                    }
                }
            }
        }
        KeyCode::Char('h') | KeyCode::Esc => {
            for e in &mut app.sidebar {
                if let SidebarEntry::Schema { expanded, .. } = e {
                    *expanded = false;
                }
            }
            app.current_schema.clear();
            if let Some(ov) = app.overview.clone() {
                rebuild_sidebar(app, &ov);
            }
        }
        _ => {}
    }
    Ok(false)
}

async fn detail_key(app: &mut AppState, key: KeyEvent) -> Result<bool> {
    match key.code {
        KeyCode::Char('l') | KeyCode::Right => {
            app.detail_tab = app.detail_tab.next();
            if app.detail_tab == DetailTab::Erd
                && app.relationships.is_empty()
                && !app.current_schema.is_empty()
            {
                let schema = app.current_schema.clone();
                app.status = format!("Loading ERD for {schema}…");
                spawn_relationships(app, schema);
            }
            if app.detail_tab == DetailTab::Erd {
                spawn_erd_prefetch(app);
            }
        }
        KeyCode::Char('h') | KeyCode::Left => {
            app.detail_tab = app.detail_tab.prev();
        }
        KeyCode::Char('j') | KeyCode::Down if app.detail_tab == DetailTab::Records => {
            if let Some(rec) = &app.records {
                let max = rec.rows.len().saturating_sub(1);
                if app.record_row < max {
                    app.record_row += 1;
                    app.record_table_state.select(Some(app.record_row));
                } else if rec.rows.len() >= 50 {
                    app.record_offset += 50;
                    let s = app.current_schema.clone();
                    let t = app.current_table.clone();
                    load_records_page(app, &s, &t);
                }
            }
        }
        KeyCode::Char('k') | KeyCode::Up if app.detail_tab == DetailTab::Records => {
            if app.record_row > 0 {
                app.record_row -= 1;
                app.record_table_state.select(Some(app.record_row));
            } else if app.record_offset > 0 {
                app.record_offset = app.record_offset.saturating_sub(50);
                let s = app.current_schema.clone();
                let t = app.current_table.clone();
                load_records_page(app, &s, &t);
            }
        }
        // ERD navigation: move the highlight through the table list.
        KeyCode::Char('j') | KeyCode::Down if app.detail_tab == DetailTab::Erd => {
            let len = erd_table_list(app).len();
            if len > 0 {
                app.erd_selected = (app.erd_selected + 1).min(len - 1);
            }
        }
        KeyCode::Char('k') | KeyCode::Up if app.detail_tab == DetailTab::Erd => {
            app.erd_selected = app.erd_selected.saturating_sub(1);
        }
        KeyCode::Enter if app.detail_tab == DetailTab::Erd => {
            open_erd_selected_table(app).await;
        }
        KeyCode::Char('o') if app.detail_tab == DetailTab::Erd => {
            open_erd_selected_table(app).await;
        }
        KeyCode::Char('f') if app.detail_tab == DetailTab::Erd => {
            app.erd_chart_fullscreen = !app.erd_chart_fullscreen;
            app.status = if app.erd_chart_fullscreen {
                "ERD chart: fullscreen (press f to exit)".to_owned()
            } else {
                "ERD chart: split view".to_owned()
            };
        }
        KeyCode::Char(']') if app.detail_tab == DetailTab::Records => {
            if let Some(rec) = &app.records {
                let max = rec.columns.len().saturating_sub(1);
                app.record_col = (app.record_col + 1).min(max);
            }
        }
        KeyCode::Char('[') if app.detail_tab == DetailTab::Records => {
            app.record_col = app.record_col.saturating_sub(1);
        }
        KeyCode::Char('y') => match app.detail_tab {
            DetailTab::Records => {
                if let Some(val) = app.selected_cell() {
                    app.status = format!("yanked: {val}");
                }
            }
            DetailTab::Columns => {
                if let Some(info) = &app.table_info {
                    let col_names: Vec<&str> =
                        info.columns.iter().map(|c| c.name.as_str()).collect();
                    app.status = format!("yanked columns: {}", col_names.join(", "));
                }
            }
            DetailTab::Erd => match save_mermaid_for_schema(app).await {
                Ok(path) => {
                    app.status = format!("wrote Mermaid ERD → {}", path.display());
                }
                Err(e) => {
                    app.last_error = Some(format!("save mermaid: {e}"));
                }
            },
            _ => {}
        },
        KeyCode::Char('Y') if app.detail_tab == DetailTab::Records => {
            if let Some(tsv) = app.selected_row_tsv() {
                app.status = format!("yanked row: {tsv}");
            }
        }
        KeyCode::Esc => {
            app.pane = BrowserPane::Sidebar;
            app.status = nav_hint();
        }
        _ => {}
    }
    Ok(false)
}

/// Open the table currently highlighted on the ERD tab as the active
/// browser table. Mirrors a sidebar `Enter`, but stays on the ERD tab
/// so the user can keep navigating relationships without losing their
/// place.
async fn open_erd_selected_table(app: &mut AppState) {
    let tables = erd_table_list(app);
    let Some(target) = tables
        .get(app.erd_selected.min(tables.len().saturating_sub(1)))
        .cloned()
    else {
        return;
    };
    let schema = app.current_schema.clone();
    select_sidebar_for(app, &target);
    load_table(app, &schema, &target).await;
}

/// If the sidebar contains an entry for `table`, point `sidebar_idx` at
/// it so the highlight follows the user. Silent no-op when the table is
/// not visible (e.g. its parent schema is collapsed) — `load_table`
/// already handles loading the metadata regardless.
fn select_sidebar_for(app: &mut AppState, table: &str) {
    for (i, entry) in app.sidebar.iter().enumerate() {
        if let SidebarEntry::Table { name, .. } = entry {
            if name == table {
                app.sidebar_idx = i;
                app.sidebar_list_state.select(Some(i));
                return;
            }
        }
    }
}

// ─── Data loaders ─────────────────────────────────────────────────────────────

/// Kick off a non-blocking load of `(schema.table)`'s metadata and first
/// records page. Marks the table as the active selection synchronously so
/// the sidebar updates immediately, then spawns two background tasks that
/// will deliver `TableInfo` and `Records` messages to the event loop.
async fn load_table(app: &mut AppState, schema: &str, table: &str) {
    // Preserve cached relationships and ERD card-info when jumping
    // inside the same schema (both are schema-scoped, so they stay
    // valid). On a schema change we drop them so we never render
    // edges or cards from a stale schema.
    if schema != app.current_schema {
        app.relationships.clear();
        app.erd_selected = 0;
        app.erd_table_info.clear();
    }
    app.current_schema = schema.to_owned();
    app.current_table = table.to_owned();
    app.record_offset = 0;
    app.table_info = None;
    app.records = None;
    app.status = format!("Loading {schema}.{table}…");

    spawn_table_info(app, schema.to_owned(), table.to_owned());
    spawn_records(app, schema.to_owned(), table.to_owned(), 0);
}

/// Spawn a non-blocking re-fetch of the active table's records at the
/// current `record_offset` (used after `n`/`p` paging key presses).
fn load_records_page(app: &mut AppState, schema: &str, table: &str) {
    spawn_records(app, schema.to_owned(), table.to_owned(), app.record_offset);
}

fn spawn_table_info(app: &mut AppState, schema: String, table: String) {
    let (Some(pool), Some(tx)) = (app.pool.clone(), app.tx.clone()) else {
        return;
    };
    app.pending += 1;
    tokio::spawn(async move {
        let result = pool
            .fetch_table_info(&schema, &table)
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(DbMessage::TableInfo {
            schema,
            table,
            result,
        });
    });
}

fn spawn_records(app: &mut AppState, schema: String, table: String, offset: usize) {
    let (Some(pool), Some(tx)) = (app.pool.clone(), app.tx.clone()) else {
        return;
    };
    app.pending += 1;
    tokio::spawn(async move {
        let result = pool
            .fetch_records(&schema, &table, 50, offset)
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(DbMessage::Records {
            schema,
            table,
            offset,
            result,
        });
    });
}

/// Pre-fetch a `TableInfo` for every table in the active schema so the
/// ERD card-grid renderer can show full column lists with PK/FK
/// badges. Skips tables already cached. Runs the queries concurrently.
fn spawn_erd_prefetch(app: &mut AppState) {
    let schema = app.current_schema.clone();
    if schema.is_empty() {
        return;
    }
    let tables: Vec<String> = match app.overview.as_ref() {
        Some(ov) => ov
            .schemas
            .iter()
            .find(|s| s.name == schema)
            .map(|si| si.tables.clone())
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let (Some(pool), Some(tx)) = (app.pool.clone(), app.tx.clone()) else {
        return;
    };
    for t in tables {
        if app.erd_table_info.contains_key(&t) {
            continue;
        }
        app.pending += 1;
        let pool = pool.clone();
        let tx = tx.clone();
        let s = schema.clone();
        tokio::spawn(async move {
            let result = pool
                .fetch_table_info(&s, &t)
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(DbMessage::ErdTableInfo {
                schema: s,
                table: t,
                result,
            });
        });
    }
}

fn spawn_relationships(app: &mut AppState, schema: String) {
    let (Some(pool), Some(tx)) = (app.pool.clone(), app.tx.clone()) else {
        return;
    };
    app.pending += 1;
    tokio::spawn(async move {
        let result = pool
            .fetch_relationships(&schema)
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(DbMessage::Relationships { schema, result });
    });
}

// ─── Draw root ────────────────────────────────────────────────────────────────

fn draw(f: &mut Frame<'_>, app: &AppState) {
    let area = f.area();
    let th = app.theme;

    f.render_widget(Block::default().style(Style::default().bg(th.bg)), area);

    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(f, app, root[0]);

    match app.mode {
        AppMode::Connect => draw_connect(f, app, root[1]),
        AppMode::Editor => draw_editor(f, app, root[1]),
        AppMode::Browser => {
            let body = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(18), Constraint::Percentage(82)])
                .split(root[1]);
            draw_sidebar(f, app, body[0]);
            draw_detail(f, app, body[1]);
        }
    }

    draw_status(f, app, root[2]);
}

// ─── Header ───────────────────────────────────────────────────────────────────

fn draw_connect(f: &mut Frame<'_>, app: &AppState, area: Rect) {
    let th = app.theme;

    let has_saved = !app.saved_connections.is_empty();
    let list_height = if has_saved {
        (app.saved_connections.len() as u16 + 2).min(area.height.saturating_sub(6))
    } else {
        0
    };

    let constraints = if has_saved {
        vec![
            Constraint::Length(list_height),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
        ]
    } else {
        vec![
            Constraint::Percentage(30),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
        ]
    };

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let (list_area, driver_area, url_area) = if has_saved {
        (Some(layout[0]), layout[2], layout[3])
    } else {
        (None, layout[1], layout[2])
    };

    // ── Saved connections list ──
    if let Some(list_area) = list_area {
        let items: Vec<ListItem> = app
            .saved_connections
            .iter()
            .enumerate()
            .map(|(i, (name, conn))| {
                let driver_badge = match conn.driver {
                    DriverKind::Postgres => "PG",
                    DriverKind::Sqlite => "SQ",
                    DriverKind::Mysql => "MY",
                };
                let sel = i == app.connect_idx && app.connect_focus == ConnectFocus::Picker;
                let st = if sel {
                    Style::default().fg(th.bg).bg(th.accent)
                } else {
                    Style::default().fg(th.fg)
                };
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!(" [{driver_badge}] "),
                        if sel {
                            Style::default()
                                .fg(th.bg)
                                .bg(th.accent)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(th.accent2).add_modifier(Modifier::BOLD)
                        },
                    ),
                    Span::styled(format!("{name}  "), st.add_modifier(Modifier::BOLD)),
                    Span::styled(
                        truncate_url(&conn.url, 50),
                        if sel {
                            Style::default().fg(th.bg).bg(th.accent)
                        } else {
                            Style::default().fg(th.muted)
                        },
                    ),
                ]))
            })
            .collect();

        let picker_border = if app.connect_focus == ConnectFocus::Picker {
            th.active_border
        } else {
            th.border
        };

        f.render_widget(
            List::new(items).block(
                Block::default()
                    .title(Span::styled(
                        "  Saved Connections  ",
                        Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                    ))
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(picker_border))
                    .style(Style::default().bg(th.bg)),
            ),
            list_area,
        );
    }

    // ── Driver toggle ──
    let driver_label = match app.driver {
        DriverKind::Postgres => "Postgres",
        DriverKind::Sqlite => "SQLite",
        DriverKind::Mysql => "MySQL/MariaDB",
    };

    f.render_widget(
        Paragraph::new(vec![Line::from(Span::styled(
            format!("  Driver: {driver_label}  (Tab to toggle)"),
            Style::default().fg(th.accent2).add_modifier(Modifier::BOLD),
        ))])
        .block(
            Block::default()
                .title(Span::styled("  Driver  ", Style::default().fg(th.muted)))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(th.border))
                .style(Style::default().bg(th.bg)),
        ),
        driver_area,
    );

    // ── URL input ──
    let url_border = if app.connect_focus == ConnectFocus::NewUrl {
        th.active_border
    } else {
        th.border
    };

    let in_name_prompt = app.connect_focus == ConnectFocus::NameNew;
    let url_display = if in_name_prompt && app.connect_input.is_empty() {
        Span::styled(
            "  letters / digits / _ / -    Enter save    Esc skip",
            Style::default().fg(th.muted),
        )
    } else if in_name_prompt {
        Span::styled(
            format!("  {}_", app.connect_input),
            Style::default().fg(th.fg),
        )
    } else if app.connect_input.is_empty() && app.connect_focus == ConnectFocus::NewUrl {
        Span::styled(
            "  e.g. postgres://user:pass@localhost/db  or  sqlite:./my.db",
            Style::default().fg(th.muted),
        )
    } else if app.connect_input.is_empty() {
        Span::styled(
            "  press n for new connection",
            Style::default().fg(th.muted),
        )
    } else {
        Span::styled(
            format!("  {}_", app.connect_input),
            Style::default().fg(th.fg),
        )
    };

    let url_title = if in_name_prompt {
        "  Name this connection  "
    } else if has_saved {
        "  New Connection (n)  "
    } else {
        "  Connection URL  "
    };

    let url_border = if in_name_prompt {
        th.active_border
    } else {
        url_border
    };

    f.render_widget(
        Paragraph::new(Line::from(url_display)).block(
            Block::default()
                .title(Span::styled(
                    url_title,
                    Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                ))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(url_border))
                .style(Style::default().bg(th.bg)),
        ),
        url_area,
    );
}

fn truncate_url(url: &str, max: usize) -> String {
    if url.len() <= max {
        url.to_owned()
    } else {
        format!("{}…", &url[..max - 1])
    }
}

fn draw_header(f: &mut Frame<'_>, app: &AppState, area: Rect) {
    let th = app.theme;
    let driver_label = match app.driver {
        DriverKind::Postgres => "PG",
        DriverKind::Sqlite => "SQ",
        DriverKind::Mysql => "MY",
    };
    let mode_badge = match app.mode {
        AppMode::Connect => " CONNECT ",
        AppMode::Browser => " BROWSER ",
        AppMode::Editor => " EDITOR  ",
    };
    let line = Line::from(vec![
        Span::styled(
            " TSQL ",
            Style::default()
                .fg(th.bg)
                .bg(th.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {driver_label} "),
            Style::default()
                .fg(th.bg)
                .bg(th.accent2)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(mode_badge, Style::default().fg(th.muted).bg(th.bg)),
        Span::styled(
            format!(" {}", app.url),
            Style::default().fg(th.muted).bg(th.bg),
        ),
    ]);
    f.render_widget(Paragraph::new(line).style(Style::default().bg(th.bg)), area);
}

// ─── Sidebar ──────────────────────────────────────────────────────────────────

fn draw_sidebar(f: &mut Frame<'_>, app: &AppState, area: Rect) {
    let th = app.theme;
    let active = app.pane == BrowserPane::Sidebar;
    let border_st = Style::default().fg(if active { th.active_border } else { th.border });
    let title_st = Style::default()
        .fg(if active { th.accent } else { th.muted })
        .add_modifier(Modifier::BOLD);

    let items: Vec<ListItem> = app
        .sidebar
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let is_cur_table = matches!(entry,
                SidebarEntry::Table { name, .. } if *name == app.current_table
            );
            let style = if i == app.sidebar_idx && active {
                Style::default()
                    .fg(th.sel_fg)
                    .bg(th.sel_bg)
                    .add_modifier(Modifier::BOLD)
            } else if is_cur_table {
                Style::default().fg(th.success).add_modifier(Modifier::BOLD)
            } else {
                match entry {
                    SidebarEntry::Schema { .. } => {
                        Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
                    }
                    SidebarEntry::Table { .. } => Style::default().fg(th.fg),
                }
            };
            ListItem::new(entry.display()).style(style)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .title(Span::styled(" Schemas ", title_st))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border_st)
            .style(Style::default().bg(th.bg)),
    );

    let mut ls = app.sidebar_list_state;
    f.render_stateful_widget(list, area, &mut ls);
}

// ─── Detail area ──────────────────────────────────────────────────────────────

fn draw_detail(f: &mut Frame<'_>, app: &AppState, area: Rect) {
    let th = app.theme;
    let active = app.pane == BrowserPane::Detail;
    let border_st = Style::default().fg(if active { th.active_border } else { th.border });

    if app.current_table.is_empty() {
        f.render_widget(
            Paragraph::new(vec![
                Line::default(),
                Line::from(Span::styled(
                    "  Select a table from the sidebar",
                    Style::default().fg(th.muted),
                )),
                Line::from(Span::styled(
                    "  l or Enter  to expand a schema",
                    Style::default().fg(th.muted),
                )),
                Line::from(Span::styled(
                    "  e            to open the SQL editor",
                    Style::default().fg(th.muted),
                )),
            ])
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(border_st)
                    .style(Style::default().bg(th.bg)),
            ),
            area,
        );
        return;
    }

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    // Tab bar
    let tab_titles: Vec<Line> = ALL_TABS
        .iter()
        .map(|tab| {
            let st = if *tab == app.detail_tab {
                Style::default()
                    .fg(th.accent)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default().fg(th.muted)
            };
            Line::from(Span::styled(tab.hotkey_label(), st))
        })
        .collect();

    f.render_widget(
        Tabs::new(tab_titles)
            .select(app.detail_tab.index())
            .block(
                Block::default()
                    .title(Span::styled(
                        format!("  {}.{}  ", app.current_schema, app.current_table),
                        Style::default().fg(th.accent2).add_modifier(Modifier::BOLD),
                    ))
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(border_st)
                    .style(Style::default().bg(th.bg)),
            )
            .style(Style::default().bg(th.bg))
            .divider(Span::styled(" │ ", Style::default().fg(th.border))),
        layout[0],
    );

    let content_block = Block::default()
        .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
        .border_type(BorderType::Rounded)
        .border_style(border_st)
        .style(Style::default().bg(th.bg));

    let inner = content_block.inner(layout[1]);
    f.render_widget(content_block, layout[1]);

    match app.detail_tab {
        DetailTab::Records => draw_records(f, app, inner),
        DetailTab::Columns => draw_columns(f, app, inner),
        DetailTab::Indexes => draw_indexes(f, app, inner),
        DetailTab::Keys => draw_keys(f, app, inner),
        DetailTab::Constraints => draw_constraints(f, app, inner),
        DetailTab::Erd => draw_erd(f, app, inner),
    }
}

// ─── Records tab ──────────────────────────────────────────────────────────────

fn draw_records(f: &mut Frame<'_>, app: &AppState, area: Rect) {
    let th = app.theme;
    let Some(rec) = &app.records else {
        f.render_widget(muted_para("  No records loaded.", th), area);
        return;
    };

    if rec.columns.is_empty() {
        f.render_widget(
            muted_para(&format!("  {} rows affected.", rec.rows_affected), th),
            area,
        );
        return;
    }

    // ── Horizontal column window ───────────────────────────────
    // At narrow widths (e.g. half-screen Tmux pane) splitting every
    // column equally turns each cell into 4–5 chars of garbage. Pick a
    // sensible minimum cell width (12 cells) and only render as many
    // columns as fit, with the focused column always visible. `[`/`]`
    // moves focus, the window slides to follow.
    let col_total = rec.columns.len();
    // 14 cells per column = enough to show a `YYYY-MM-DD` date or a
    // short numeric value with one space of padding. Anything narrower
    // and even the date prefix in an RFC3339 timestamp gets clipped.
    let min_cell: u16 = 14;
    let inner_w = area.width.max(1);
    let max_cols = ((inner_w + 1) / (min_cell + 1)).max(1) as usize;
    let visible = col_total.min(max_cols);

    // Slide the offset so `record_col` stays inside [offset, offset+visible).
    let mut col_off = app
        .record_col_offset
        .get()
        .min(col_total.saturating_sub(visible));
    if app.record_col < col_off {
        col_off = app.record_col;
    } else if app.record_col >= col_off + visible {
        col_off = app.record_col + 1 - visible;
    }
    col_off = col_off.min(col_total.saturating_sub(visible));
    app.record_col_offset.set(col_off);

    let col_end = (col_off + visible).min(col_total);
    let visible_count = col_end - col_off;
    let total_pct = 100u16;
    let sep_count = visible_count.saturating_sub(1) as u16;
    let data_pct = (total_pct.saturating_sub(sep_count) / visible_count.max(1) as u16).max(1);
    let widths: Vec<Constraint> = build_grid_widths(visible_count, data_pct);

    let header_cells: Vec<Cell<'_>> = interleave_separators(
        rec.columns[col_off..col_end]
            .iter()
            .enumerate()
            .map(|(off_idx, name)| {
                let ci = col_off + off_idx;
                let st = if ci == app.record_col {
                    Style::default()
                        .fg(th.accent)
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                } else {
                    Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
                };
                ccell(name.as_str(), st)
            }),
        th,
        /* row_bg */ th.sel_bg,
    );
    let header = Row::new(header_cells)
        .style(Style::default().bg(th.sel_bg))
        .height(1);

    let rows: Vec<Row> = rec
        .rows
        .iter()
        .enumerate()
        .map(|(ri, row)| {
            let row_bg = if ri == app.record_row {
                th.sel_bg
            } else if ri % 2 == 1 {
                th.row_alt_bg
            } else {
                th.bg
            };
            let cells = interleave_separators(
                row[col_off.min(row.len())..col_end.min(row.len())]
                    .iter()
                    .enumerate()
                    .map(|(off_idx, val)| {
                        let ci = col_off + off_idx;
                        let st = if ri == app.record_row && ci == app.record_col {
                            Style::default().fg(th.bg).bg(th.accent)
                        } else if ri == app.record_row {
                            Style::default().fg(th.sel_fg).bg(row_bg)
                        } else if val == "NULL" {
                            Style::default().fg(th.muted).bg(row_bg)
                        } else {
                            Style::default().fg(th.fg).bg(row_bg)
                        };
                        // Left-align body values so a too-wide cell
                        // truncates from the right (date prefix wins
                        // over timezone suffix). Headers stay centred.
                        lcell(format!(" {val}"), st)
                    }),
                th,
                row_bg,
            );
            Row::new(cells).style(Style::default().bg(row_bg)).height(1)
        })
        .collect();

    let mut ts = app.record_table_state;
    let table_area = if col_total > visible_count {
        // Reserve the bottom row for a scroll indicator so the user
        // knows there are columns out of view.
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(area);
        let hint = format!(
            "  cols {}–{} / {}   [/] scroll",
            col_off + 1,
            col_end,
            col_total
        );
        f.render_widget(
            Paragraph::new(Span::styled(hint, Style::default().fg(th.muted)))
                .style(Style::default().bg(th.bg)),
            split[1],
        );
        split[0]
    } else {
        area
    };
    f.render_stateful_widget(
        Table::new(rows, widths)
            .header(header)
            .row_highlight_style(Style::default().bg(th.sel_bg)),
        table_area,
        &mut ts,
    );
}

/// Build a centred `Cell` with the given style. Used by every detail
/// tab so headers and body values share the same alignment treatment.
fn ccell<'a>(content: impl Into<std::borrow::Cow<'a, str>>, style: Style) -> Cell<'a> {
    Cell::from(Line::from(Span::raw(content)).alignment(Alignment::Center)).style(style)
}

/// Build a left-aligned `Cell`. Used for record body values where the
/// content can be longer than the column width — left-alignment keeps
/// the meaningful prefix visible (e.g. `2026-01-05T…` instead of
/// `-01-05T09:15:00+0` produced by centre-truncation).
fn lcell<'a>(content: impl Into<std::borrow::Cow<'a, str>>, style: Style) -> Cell<'a> {
    Cell::from(Line::from(Span::raw(content)).alignment(Alignment::Left)).style(style)
}

/// Produce `[data, sep, data, sep, …, data]` width constraints for an
/// `n`-column grid. Each `data` slot gets `data_pct` percent, separators
/// are 1 cell wide. Returns just `[data]` when `n == 1`.
fn build_grid_widths(n: usize, data_pct: u16) -> Vec<Constraint> {
    if n == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(2 * n - 1);
    for i in 0..n {
        if i > 0 {
            out.push(Constraint::Length(1));
        }
        out.push(Constraint::Percentage(data_pct));
    }
    out
}

/// Interleave a `│` separator cell between every pair of data cells. The
/// separator inherits the row's background so zebra striping stays
/// consistent across the row.
fn interleave_separators<'a, I>(cells: I, th: Theme, row_bg: Color) -> Vec<Cell<'a>>
where
    I: IntoIterator<Item = Cell<'a>>,
{
    let collected: Vec<Cell<'a>> = cells.into_iter().collect();
    if collected.len() < 2 {
        return collected;
    }
    let mut out = Vec::with_capacity(2 * collected.len() - 1);
    let sep_style = Style::default().fg(th.border).bg(row_bg);
    let mut iter = collected.into_iter();
    if let Some(first) = iter.next() {
        out.push(first);
    }
    for cell in iter {
        out.push(Cell::from("│").style(sep_style));
        out.push(cell);
    }
    out
}

// ─── Columns tab ──────────────────────────────────────────────────────────────

fn draw_columns(f: &mut Frame<'_>, app: &AppState, area: Rect) {
    let th = app.theme;
    let Some(info) = &app.table_info else {
        f.render_widget(muted_para("  No table loaded.", th), area);
        return;
    };

    let pk_set: HashSet<&str> = info
        .primary_key
        .as_ref()
        .map(|pk| pk.column_names.iter().map(String::as_str).collect())
        .unwrap_or_default();

    let fk_set: HashSet<&str> = info
        .foreign_keys
        .iter()
        .flat_map(|fk| fk.column_names.iter().map(String::as_str))
        .collect();

    let head_st = Style::default().fg(th.accent).add_modifier(Modifier::BOLD);
    let header = Row::new(vec![
        ccell("Column", head_st),
        ccell("Type", head_st),
        ccell("PK", head_st),
        ccell("FK", head_st),
        ccell("Nullable", head_st),
        ccell("Default", head_st),
    ])
    .style(Style::default().bg(th.sel_bg));

    let rows: Vec<Row> = info
        .columns
        .iter()
        .map(|col| {
            let is_pk = pk_set.contains(col.name.as_str());
            let is_fk = fk_set.contains(col.name.as_str());
            let name_st = if is_pk {
                Style::default().fg(th.warning).add_modifier(Modifier::BOLD)
            } else if is_fk {
                Style::default().fg(th.accent2)
            } else {
                Style::default().fg(th.fg)
            };
            let null_st = Style::default().fg(if col.is_nullable {
                th.muted
            } else {
                th.success
            });
            Row::new(vec![
                ccell(col.name.as_str(), name_st),
                ccell(col.data_type.as_str(), Style::default().fg(th.accent2)),
                ccell(
                    if is_pk { "✓" } else { "" },
                    Style::default().fg(th.warning),
                ),
                ccell(
                    if is_fk { "✓" } else { "" },
                    Style::default().fg(th.accent2),
                ),
                ccell(if col.is_nullable { "yes" } else { "no" }, null_st),
                ccell(
                    col.default_value.as_deref().unwrap_or("—"),
                    Style::default().fg(th.muted),
                ),
            ])
        })
        .collect();

    // Mix of fixed-min lengths and percentages: at narrow widths the
    // PK/FK/Nullable cells shrink to their minimum (the symbol/word
    // they hold), leaving the percentage columns to absorb the rest.
    f.render_widget(
        Table::new(
            rows,
            [
                Constraint::Min(10),
                Constraint::Min(8),
                Constraint::Length(4),
                Constraint::Length(4),
                Constraint::Length(8),
                Constraint::Min(6),
            ],
        )
        .header(header),
        area,
    );
}

// ─── Indexes tab ──────────────────────────────────────────────────────────────

fn draw_indexes(f: &mut Frame<'_>, app: &AppState, area: Rect) {
    let th = app.theme;
    let Some(info) = &app.table_info else {
        f.render_widget(muted_para("  No table loaded.", th), area);
        return;
    };
    if info.indexes.is_empty() {
        f.render_widget(muted_para("  No indexes.", th), area);
        return;
    }

    let head_st = Style::default().fg(th.accent).add_modifier(Modifier::BOLD);
    let header = Row::new(vec![
        ccell("Index", head_st),
        ccell("Columns", head_st),
        ccell("Type", head_st),
        ccell("PK", head_st),
        ccell("Unique", head_st),
    ])
    .style(Style::default().bg(th.sel_bg));

    let rows: Vec<Row> = info
        .indexes
        .iter()
        .map(|idx| {
            // Type column: render the access method as an upper-case
            // tag. Primary-key indexes get a `★ btree` prefix in their
            // own column to make the default index unmistakable.
            let method_label = idx.method.to_uppercase();
            let method_style = match idx.method.as_str() {
                "btree" => Style::default().fg(th.accent),
                "hash" => Style::default().fg(th.warning),
                "gin" | "gist" | "spgist" | "brin" => Style::default().fg(th.success),
                _ => Style::default().fg(th.fg),
            };
            let pk_cell = if idx.is_primary {
                ccell(
                    "★",
                    Style::default().fg(th.warning).add_modifier(Modifier::BOLD),
                )
            } else {
                ccell("—", Style::default().fg(th.muted))
            };
            // Highlight the index name in warning when it backs the PK
            // so the eye lands on the default index first.
            let name_style = if idx.is_primary {
                Style::default().fg(th.warning).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(th.fg)
            };
            let unique_st = Style::default().fg(if idx.is_unique { th.success } else { th.muted });
            Row::new(vec![
                ccell(idx.name.as_str(), name_style),
                ccell(idx.column_names.join(", "), Style::default().fg(th.accent2)),
                ccell(method_label, method_style),
                pk_cell,
                ccell(if idx.is_unique { "✓" } else { "—" }, unique_st),
            ])
        })
        .collect();

    f.render_widget(
        Table::new(
            rows,
            [
                Constraint::Min(12),
                Constraint::Min(10),
                Constraint::Length(7),
                Constraint::Length(3),
                Constraint::Length(7),
            ],
        )
        .header(header),
        area,
    );
}

// ─── Keys tab ─────────────────────────────────────────────────────────────────

fn draw_keys(f: &mut Frame<'_>, app: &AppState, area: Rect) {
    let th = app.theme;
    let Some(info) = &app.table_info else {
        f.render_widget(muted_para("  No table loaded.", th), area);
        return;
    };

    if info.primary_key.is_none() && info.foreign_keys.is_empty() {
        f.render_widget(muted_para("  No keys defined.", th), area);
        return;
    }

    let head_st = Style::default().fg(th.accent).add_modifier(Modifier::BOLD);
    let header = Row::new(vec![
        ccell("Kind", head_st),
        ccell("Name", head_st),
        ccell("Columns", head_st),
        ccell("References", head_st),
    ])
    .style(Style::default().bg(th.sel_bg));

    let mut rows: Vec<Row> = Vec::new();

    if let Some(pk) = &info.primary_key {
        rows.push(Row::new(vec![
            ccell(
                "PK",
                Style::default().fg(th.warning).add_modifier(Modifier::BOLD),
            ),
            ccell(pk.name.as_str(), Style::default().fg(th.muted)),
            ccell(
                pk.column_names.join(", "),
                Style::default().fg(th.fg).add_modifier(Modifier::BOLD),
            ),
            ccell("—", Style::default().fg(th.muted)),
        ]));
    }

    for fk in &info.foreign_keys {
        let target = format!(
            "{}.{}",
            fk.referenced_table,
            fk.referenced_columns.join(", "),
        );
        rows.push(Row::new(vec![
            ccell(
                "FK",
                Style::default().fg(th.accent2).add_modifier(Modifier::BOLD),
            ),
            ccell(fk.name.as_str(), Style::default().fg(th.muted)),
            ccell(fk.column_names.join(", "), Style::default().fg(th.fg)),
            ccell(target, Style::default().fg(th.accent)),
        ]));
    }

    f.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(4),
                Constraint::Min(10),
                Constraint::Min(10),
                Constraint::Min(10),
            ],
        )
        .header(header),
        area,
    );
}

// ─── Constraints tab ──────────────────────────────────────────────────────────

fn draw_constraints(f: &mut Frame<'_>, app: &AppState, area: Rect) {
    let th = app.theme;
    let Some(info) = &app.table_info else {
        f.render_widget(muted_para("  No table loaded.", th), area);
        return;
    };
    if info.constraints.is_empty() {
        f.render_widget(muted_para("  No additional constraints.", th), area);
        return;
    }

    let header = Row::new(vec![
        Cell::from("Constraint").style(Style::default().fg(th.accent).add_modifier(Modifier::BOLD)),
        Cell::from("Definition").style(Style::default().fg(th.accent).add_modifier(Modifier::BOLD)),
    ])
    .style(Style::default().bg(th.sel_bg));

    let rows: Vec<Row> = info
        .constraints
        .iter()
        .map(|c| {
            Row::new(vec![
                Cell::from(c.name.as_str()).style(Style::default().fg(th.fg)),
                Cell::from(c.definition.as_str()).style(Style::default().fg(th.muted)),
            ])
        })
        .collect();

    f.render_widget(
        Table::new(rows, [Constraint::Min(12), Constraint::Min(20)]).header(header),
        area,
    );
}

// ─── ERD tab ──────────────────────────────────────────────────────────────────
//
// We tried two visual diagrams (a layered graph, then a dbdiagram-style
// card grid). Both fought the terminal: ASCII / box-drawing line
// routing never looked smooth, parallel edges crossed badly, and
// arrowheads renderered inconsistently across fonts. So we pivoted to
// what TUIs are actually good at: a structured inspector.
//
// Layout:
//
//   ┌── Tables ───┐   ┌── Inspector ─────────────────────────────┐
//   │  customers  │   │ orders                                   │
//   │  orders   ▶ │   │ Columns                                  │
//   │  products   │   │   ★ id              int4                 │
//   │             │   │   ⚷ customer_id     int4   → customers.id│
//   │             │   │     issue_date      date                 │
//   │             │   │ References →                             │
//   │             │   │   1. customer_id → customers.id          │
//   │             │   │ Referenced by ←                          │
//   │             │   │   (none)                                 │
//   │             │   │ Mermaid                                  │
//   │             │   │   ```mermaid                             │
//   │             │   │   erDiagram                              │
//   │             │   │     orders { … }                         │
//   │             │   │     customers { … }                      │
//   │             │   │     orders }o--|| customers : customer_id│
//   │             │   │   ```                                    │
//   └─────────────┘   └──────────────────────────────────────────┘
//
// Keys:
//   j/k        — move table selection
//   Enter / o  — open the selected table (load as the active table)
//   y          — write the Mermaid block for the whole schema to
//                `./<schema>.mmd`. The output renders as a real ERD
//                in any Mermaid-aware viewer (GitHub, Notion, IDE
//                preview, https://mermaid.live).

fn draw_erd(f: &mut Frame<'_>, app: &AppState, area: Rect) {
    let th = app.theme;
    let schema = &app.current_schema;
    let tables = erd_table_list(app);

    if tables.is_empty() {
        let hint = if schema.is_empty() {
            "  No schema selected."
        } else {
            "  No tables in this schema."
        };
        f.render_widget(muted_para(hint, th), area);
        return;
    }

    // Vertical split. Fullscreen mode (`f`) gives the chart the
    // entire body so the user can actually read box labels; default
    // mode keeps the table list + per-table inspector below.
    let layout = if app.erd_chart_fullscreen {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(0)])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Percentage(70),
                Constraint::Min(8),
            ])
            .split(area)
    };

    // Banner.
    let banner = vec![Line::from(vec![
        Span::styled(
            format!("  ERD  schema: {schema}  "),
            Style::default()
                .fg(th.bg)
                .bg(th.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "  {} table(s), {} edge(s)  j/k focus  Enter open  f fullscreen  y export .mmd  ",
                tables.len(),
                app.relationships.len(),
            ),
            Style::default().fg(th.muted),
        ),
    ])];
    f.render_widget(
        Paragraph::new(banner).style(Style::default().bg(th.bg)),
        layout[0],
    );

    draw_erd_chart_pane(f, app, layout[1]);

    if app.erd_chart_fullscreen {
        return;
    }

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(28), Constraint::Min(0)])
        .split(layout[2]);

    let selected = app.erd_selected.min(tables.len() - 1);
    draw_erd_table_list(f, app, &tables, selected, body[0]);
    draw_erd_inspector(f, app, &tables, selected, body[1]);
}

/// Render a focused, pure-Rust schema graph centred on the selected
/// table. No external tools, no async pipeline — just a hand-rolled
/// box-drawing canvas:
///
///   ┌─ neighbours that reference SELECTED ──┐  ╭── SELECTED ──╮  ┌─ tables SELECTED references ─┐
///   │  shipments  ─── ship_id ──────────────│──▶│ ★ id         │──── customer_id ──▶│  customers  │
///   │  invoices   ─── inv_id  ──────────────│──▶│ ⚷ customer_id│                    └─────────────┘
///   └───────────────────────────────────────┘  │   amount      │
///                                              ╰──────────────╯
///
/// FK column labels ride on top of the connecting line. PK columns get
/// `★`, FK columns `⚷`. When the pane is too narrow / short, neighbour
/// cards are dropped first; the centre card is always preserved.
fn draw_erd_chart_pane(f: &mut Frame<'_>, app: &AppState, area: Rect) {
    let th = app.theme;
    let block = Block::default()
        .title(Span::styled(
            "  Schema map  (focused on selected table)  ",
            Style::default().fg(th.accent2).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(th.border))
        .style(Style::default().bg(th.bg));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width < 24 || inner.height < 6 {
        let msg = vec![
            Line::default(),
            Line::from(Span::styled(
                "  pane too small — press f to fullscreen",
                Style::default().fg(th.muted),
            )),
        ];
        f.render_widget(Paragraph::new(msg).style(Style::default().bg(th.bg)), inner);
        return;
    }

    let tables = erd_table_list(app);
    let selected = app.erd_selected.min(tables.len().saturating_sub(1));
    let Some(centre_name) = tables.get(selected).cloned() else {
        return;
    };
    let centre_info = app.erd_table_info.get(&centre_name).cloned();

    // Outgoing FKs: this table → others.
    let outgoing: Vec<&RelationshipEdge> = app
        .relationships
        .iter()
        .filter(|e| e.from_table == centre_name)
        .collect();
    // Incoming FKs: others → this table.
    let incoming: Vec<&RelationshipEdge> = app
        .relationships
        .iter()
        .filter(|e| e.to_table == centre_name)
        .collect();

    let canvas = render_focus_canvas(
        inner.width,
        inner.height,
        &centre_name,
        centre_info.as_ref(),
        &incoming,
        &outgoing,
        th,
    );
    f.render_widget(
        Paragraph::new(canvas).style(Style::default().bg(th.bg)),
        inner,
    );
}

/// Single canvas cell. `bold` is the only modifier we need so far —
/// we keep the type tiny so the grid stays cache-friendly.
#[derive(Clone, Copy)]
struct Cell2 {
    ch: char,
    fg: Color,
    bold: bool,
}

impl Cell2 {
    fn space(th: Theme) -> Self {
        Self {
            ch: ' ',
            fg: th.fg,
            bold: false,
        }
    }
}

/// Build the focused-graph canvas. Returns one styled `Line` per row.
#[allow(clippy::too_many_arguments)]
fn render_focus_canvas(
    width: u16,
    height: u16,
    centre_name: &str,
    centre_info: Option<&TableInfo>,
    incoming: &[&RelationshipEdge],
    outgoing: &[&RelationshipEdge],
    th: Theme,
) -> Vec<Line<'static>> {
    let w = width as usize;
    let h = height as usize;
    let mut grid: Vec<Vec<Cell2>> = vec![vec![Cell2::space(th); w]; h];

    // ── centre card ────────────────────────────────────────────────
    // Build rows: header (table name) + horizontal rule + columns.
    let mut centre_rows: Vec<(String, Color, bool)> = Vec::new();
    centre_rows.push((centre_name.to_owned(), th.accent, true));
    if let Some(info) = centre_info {
        let pk_set: HashSet<&str> = info
            .primary_key
            .as_ref()
            .map(|pk| pk.column_names.iter().map(String::as_str).collect())
            .unwrap_or_default();
        let fk_set: HashSet<&str> = info
            .foreign_keys
            .iter()
            .flat_map(|fk| fk.column_names.iter().map(String::as_str))
            .collect();
        let name_w = info.columns.iter().map(|c| c.name.len()).max().unwrap_or(4);
        for c in info.columns.iter().take(8) {
            let (marker, color) = if pk_set.contains(c.name.as_str()) {
                ('★', th.warning)
            } else if fk_set.contains(c.name.as_str()) {
                ('⚷', th.accent2)
            } else {
                (' ', th.fg)
            };
            let label = format!("{marker} {:<w$}  {}", c.name, c.data_type, w = name_w);
            centre_rows.push((label, color, false));
        }
        if info.columns.len() > 8 {
            centre_rows.push((
                format!("… (+{} more)", info.columns.len() - 8),
                th.muted,
                false,
            ));
        }
    } else {
        centre_rows.push(("(loading…)".to_owned(), th.muted, false));
    }

    // Side card width scales with available width. We never go below
    // 14 (room for ~10 chars of table name) since otherwise the box
    // truncates short table names like `customers` mid-word.
    let side_box_w: usize = if w < 80 {
        14
    } else if w < 110 {
        16
    } else {
        18
    };
    let side_box_h: usize = 3;
    let arrow_pad: usize = 4;

    // Box width = max content + 2 padding. Cap so neighbours fit if at
    // all possible — at narrow widths we'd rather truncate the centre
    // card body than push neighbour cards off-screen.
    let max_centre_w = centre_rows
        .iter()
        .map(|(s, _, _)| s.chars().count())
        .max()
        .unwrap_or(8);
    // Reserve enough room for at least one side + arrow when w >= 50.
    let reserved = if w >= 50 {
        side_box_w + arrow_pad + 2
    } else {
        4
    };
    let centre_box_w = (max_centre_w + 4)
        .min(w.saturating_sub(reserved))
        .max(centre_name.chars().count() + 4);
    let centre_box_h = (centre_rows.len() + 2).min(h);

    // Decide how many neighbours we can show vertically with 1-row gap.
    let stack_capacity = (h.saturating_sub(1) / (side_box_h + 1)).max(1);
    let lefts: Vec<&&RelationshipEdge> = incoming.iter().take(stack_capacity).collect();
    let rights: Vec<&&RelationshipEdge> = outgoing.iter().take(stack_capacity).collect();

    // Lay out columns horizontally. If the pane is too narrow drop
    // neighbours first; centre always renders.
    let needed_w_both = side_box_w + arrow_pad + centre_box_w + arrow_pad + side_box_w;
    let needed_w_one = side_box_w + arrow_pad + centre_box_w;
    let (show_left, show_right) = if w >= needed_w_both {
        (!lefts.is_empty(), !rights.is_empty())
    } else if w >= needed_w_one {
        // Only one side fits — prefer outgoing (what the table depends on).
        if !rights.is_empty() {
            (false, true)
        } else {
            (!lefts.is_empty(), false)
        }
    } else {
        (false, false)
    };

    let centre_x = (w.saturating_sub(centre_box_w)) / 2;
    let centre_y = (h.saturating_sub(centre_box_h)) / 2;
    let left_x = 0usize;
    let right_x = w.saturating_sub(side_box_w);

    // Draw centre card.
    draw_card(
        &mut grid,
        centre_x,
        centre_y,
        centre_box_w,
        centre_box_h,
        &centre_rows,
        th.accent,
        th.accent,
        true,
        th,
    );

    // Helper: distribute n boxes evenly across the available height.
    let distribute = |n: usize| -> Vec<usize> {
        if n == 0 {
            return Vec::new();
        }
        let total_h = n * side_box_h + n.saturating_sub(1);
        if total_h >= h {
            (0..n).map(|i| i * (side_box_h + 1)).collect()
        } else {
            let start = (h - total_h) / 2;
            (0..n).map(|i| start + i * (side_box_h + 1)).collect()
        }
    };

    // Left side: incoming FKs ("X.fk_col → centre.pk").
    if show_left {
        let ys = distribute(lefts.len());
        for (i, edge) in lefts.iter().enumerate() {
            let y = ys[i];
            let rows = vec![
                (edge.from_table.clone(), th.accent, true),
                (
                    edge.from_columns
                        .join(",")
                        .chars()
                        .take(side_box_w - 4)
                        .collect::<String>(),
                    th.accent2,
                    false,
                ),
            ];
            draw_card(
                &mut grid, left_x, y, side_box_w, side_box_h, &rows, th.border, th.fg, false, th,
            );
            // Arrow from right edge of left box → left edge of centre,
            // anchored to the vertical mid of the source box and the
            // matching column row of the centre (or its header if we
            // can't find one).
            let src_y = y + side_box_h / 2;
            let dst_y = centre_y + 1 + locate_centre_row(&centre_rows, &edge.to_columns);
            let label = edge.from_columns.join(",");
            draw_arrow(
                &mut grid,
                left_x + side_box_w - 1,
                src_y,
                centre_x,
                dst_y,
                &label,
                th.accent2,
                false, // ► points right
                th,
            );
        }
    }

    // Right side: outgoing FKs ("centre.fk_col → Y.pk").
    if show_right {
        let ys = distribute(rights.len());
        for (i, edge) in rights.iter().enumerate() {
            let y = ys[i];
            let rows = vec![
                (edge.to_table.clone(), th.accent, true),
                (
                    edge.to_columns
                        .join(",")
                        .chars()
                        .take(side_box_w - 4)
                        .collect::<String>(),
                    th.warning,
                    false,
                ),
            ];
            draw_card(
                &mut grid, right_x, y, side_box_w, side_box_h, &rows, th.border, th.fg, false, th,
            );
            let src_y = centre_y + 1 + locate_centre_row(&centre_rows, &edge.from_columns);
            let dst_y = y + side_box_h / 2;
            let label = edge.from_columns.join(",");
            draw_arrow(
                &mut grid,
                centre_x + centre_box_w - 1,
                src_y,
                right_x,
                dst_y,
                &label,
                th.accent,
                false,
                th,
            );
        }
    }

    // ── footer hint ───────────────────────────────────────────────
    let hint_y = h.saturating_sub(1);
    let stats = format!(
        "  ←{} incoming   {} outgoing→   {} neighbours hidden",
        incoming.len(),
        outgoing.len(),
        incoming.len().saturating_sub(lefts.len()) + outgoing.len().saturating_sub(rights.len()),
    );
    put_text(&mut grid, 0, hint_y, &stats, th.muted, false);

    // ── convert grid → ratatui Lines ──────────────────────────────
    grid_to_lines(&grid, th)
}

/// Find the row offset in the centre card that matches the first
/// referenced column name, so the connecting arrow lands on it. Falls
/// back to the header row when no match is found.
fn locate_centre_row(rows: &[(String, Color, bool)], cols: &[String]) -> usize {
    if cols.is_empty() {
        return 0;
    }
    let target = &cols[0];
    for (i, (label, _, _)) in rows.iter().enumerate().skip(1) {
        // Centre rows look like "★ name  type". Match on whitespace-
        // delimited second token.
        let mut parts = label.split_whitespace();
        let _marker = parts.next();
        if let Some(name) = parts.next() {
            if name == target {
                return i;
            }
        }
    }
    0
}

/// Draw a rounded card with a header row + body lines. `header_color`
/// styles the title and (when `emphasise` is set) the border.
#[allow(clippy::too_many_arguments)]
fn draw_card(
    grid: &mut [Vec<Cell2>],
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    rows: &[(String, Color, bool)],
    border: Color,
    _body_color: Color,
    emphasise: bool,
    th: Theme,
) {
    if w < 4 || h < 3 {
        return;
    }
    let h_max = grid.len();
    let w_max = grid.first().map(Vec::len).unwrap_or(0);
    if y >= h_max || x >= w_max {
        return;
    }
    let h = h.min(h_max - y);
    let w = w.min(w_max - x);

    let (tl, tr, bl, br, hch, vch) = if emphasise {
        ('╭', '╮', '╰', '╯', '─', '│')
    } else {
        ('┌', '┐', '└', '┘', '─', '│')
    };
    // Top + bottom borders.
    put(grid, x, y, tl, border, false);
    put(grid, x + w - 1, y, tr, border, false);
    put(grid, x, y + h - 1, bl, border, false);
    put(grid, x + w - 1, y + h - 1, br, border, false);
    for i in 1..w - 1 {
        put(grid, x + i, y, hch, border, false);
        put(grid, x + i, y + h - 1, hch, border, false);
    }
    for j in 1..h - 1 {
        put(grid, x, y + j, vch, border, false);
        put(grid, x + w - 1, y + j, vch, border, false);
    }

    // Inner rows.
    for (i, (text, color, bold)) in rows.iter().enumerate() {
        let row_y = y + 1 + i;
        if row_y >= y + h - 1 {
            break;
        }
        let inner_w = w - 2;
        let truncated: String = text.chars().take(inner_w.saturating_sub(2)).collect();
        put_text(grid, x + 2, row_y, &truncated, *color, *bold);
        // Insert a horizontal rule under the header.
        if i == 0 && rows.len() > 1 && row_y + 1 < y + h - 1 {
            for k in 1..w - 1 {
                put(grid, x + k, row_y + 1, '─', th.muted, false);
            }
        }
    }
}

/// Draw a one-cell-thick orthogonal arrow from (x1,y1) to (x2,y2) with
/// a centred label. Uses box-drawing chars so it composites cleanly
/// over existing cells. `back_arrow` swaps the arrowhead direction.
#[allow(clippy::too_many_arguments)]
fn draw_arrow(
    grid: &mut [Vec<Cell2>],
    x1: usize,
    y1: usize,
    x2: usize,
    y2: usize,
    label: &str,
    color: Color,
    back_arrow: bool,
    _th: Theme,
) {
    if x2 <= x1 || grid.is_empty() {
        return;
    }
    let mid_x = x1 + (x2 - x1) / 2;
    // First leg: horizontal from x1 to mid_x at y1.
    for x in x1..=mid_x {
        put(grid, x, y1, '─', color, false);
    }
    // Vertical: y1 -> y2 at mid_x.
    if y1 != y2 {
        let (lo, hi) = if y1 < y2 { (y1, y2) } else { (y2, y1) };
        for y in lo..=hi {
            // Don't overwrite the corners we'll set below.
            if y != y1 && y != y2 {
                put(grid, mid_x, y, '│', color, false);
            }
        }
        // Corners.
        let c1 = if y1 < y2 { '╮' } else { '╯' };
        let c2 = if y1 < y2 { '╰' } else { '╭' };
        put(grid, mid_x, y1, c1, color, false);
        put(grid, mid_x, y2, c2, color, false);
    }
    // Second leg: horizontal from mid_x to x2 at y2.
    for x in mid_x..=x2 {
        put(grid, x, y2, '─', color, false);
    }
    // Arrowhead.
    let head_x = if back_arrow { x1 } else { x2 };
    let head_y = if back_arrow { y1 } else { y2 };
    let head_ch = if back_arrow { '◀' } else { '▶' };
    put(grid, head_x, head_y, head_ch, color, true);

    // Label sits one row above the horizontal leg with the longest run.
    if !label.is_empty() {
        let label_chars: Vec<char> = label.chars().collect();
        let lbl_y = if y1 == y2 {
            y1.saturating_sub(1)
        } else {
            // Centre on the longer leg; pick the source leg.
            y1.saturating_sub(1)
        };
        let lbl_x = x1 + 1;
        for (i, ch) in label_chars.iter().enumerate() {
            if lbl_x + i >= x2 {
                break;
            }
            put(grid, lbl_x + i, lbl_y, *ch, color, false);
        }
    }
}

#[inline]
fn put(grid: &mut [Vec<Cell2>], x: usize, y: usize, ch: char, fg: Color, bold: bool) {
    if let Some(row) = grid.get_mut(y) {
        if let Some(cell) = row.get_mut(x) {
            *cell = Cell2 { ch, fg, bold };
        }
    }
}

fn put_text(grid: &mut [Vec<Cell2>], x: usize, y: usize, text: &str, fg: Color, bold: bool) {
    for (i, ch) in text.chars().enumerate() {
        put(grid, x + i, y, ch, fg, bold);
    }
}

fn grid_to_lines(grid: &[Vec<Cell2>], th: Theme) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::with_capacity(grid.len());
    for row in grid {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut buf = String::new();
        let mut cur_fg = th.fg;
        let mut cur_bold = false;
        for cell in row {
            if cell.fg != cur_fg || cell.bold != cur_bold {
                if !buf.is_empty() {
                    let mut style = Style::default().fg(cur_fg).bg(th.bg);
                    if cur_bold {
                        style = style.add_modifier(Modifier::BOLD);
                    }
                    spans.push(Span::styled(std::mem::take(&mut buf), style));
                }
                cur_fg = cell.fg;
                cur_bold = cell.bold;
            }
            buf.push(cell.ch);
        }
        if !buf.is_empty() {
            let mut style = Style::default().fg(cur_fg).bg(th.bg);
            if cur_bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            spans.push(Span::styled(buf, style));
        }
        out.push(Line::from(spans));
    }
    out
}

/// Alphabetical list of tables in the active schema. Sourced from the
/// pre-loaded overview so it stays stable across selections.
fn erd_table_list(app: &AppState) -> Vec<String> {
    match app.overview.as_ref() {
        Some(ov) => ov
            .schemas
            .iter()
            .find(|s| s.name == app.current_schema)
            .map(|si| {
                let mut v = si.tables.clone();
                v.sort();
                v
            })
            .unwrap_or_default(),
        None => Vec::new(),
    }
}

fn draw_erd_table_list(
    f: &mut Frame<'_>,
    app: &AppState,
    tables: &[String],
    selected: usize,
    area: Rect,
) {
    let th = app.theme;
    let referenced: HashSet<&str> = app
        .relationships
        .iter()
        .flat_map(|e| [e.from_table.as_str(), e.to_table.as_str()])
        .collect();

    let block = Block::default()
        .title(Span::styled(
            "  Tables  ",
            Style::default().fg(th.accent2).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(th.border))
        .style(Style::default().bg(th.bg));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::with_capacity(tables.len());
    for (i, t) in tables.iter().enumerate() {
        let is_sel = i == selected;
        let bg = if is_sel { th.sel_bg } else { th.bg };
        let prefix = if is_sel { " ▶ " } else { "   " };
        let prefix_style = if is_sel {
            Style::default()
                .fg(th.warning)
                .bg(bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(th.muted).bg(bg)
        };
        let name_style = if is_sel {
            Style::default()
                .fg(th.accent)
                .bg(bg)
                .add_modifier(Modifier::BOLD)
        } else if referenced.contains(t.as_str()) {
            Style::default().fg(th.fg).bg(bg)
        } else {
            Style::default().fg(th.muted).bg(bg)
        };
        let pad_w = inner.width as usize;
        let used = prefix.chars().count() + t.chars().count();
        let pad = pad_w.saturating_sub(used);
        lines.push(Line::from(vec![
            Span::styled(prefix.to_owned(), prefix_style),
            Span::styled(t.clone(), name_style),
            Span::styled(" ".repeat(pad), Style::default().bg(bg)),
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_erd_inspector(
    f: &mut Frame<'_>,
    app: &AppState,
    tables: &[String],
    selected: usize,
    area: Rect,
) {
    let th = app.theme;
    let table = &tables[selected];
    let info = app.erd_table_info.get(table);

    let block = Block::default()
        .title(Span::styled(
            format!("  {table}  "),
            Style::default()
                .fg(th.accent)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(th.border))
        .style(Style::default().bg(th.bg));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    // Columns section.
    lines.push(section_header("Columns", th));
    if let Some(info) = info {
        let pk_set: HashSet<&str> = info
            .primary_key
            .as_ref()
            .map(|pk| pk.column_names.iter().map(String::as_str).collect())
            .unwrap_or_default();
        let mut fk_map: HashMap<&str, (String, String)> = HashMap::new();
        for fk in &info.foreign_keys {
            for (i, col) in fk.column_names.iter().enumerate() {
                let target_col = fk.referenced_columns.get(i).cloned().unwrap_or_default();
                fk_map.insert(col.as_str(), (fk.referenced_table.clone(), target_col));
            }
        }

        let name_w = info
            .columns
            .iter()
            .map(|c| c.name.chars().count())
            .max()
            .unwrap_or(8);
        let type_w = info
            .columns
            .iter()
            .map(|c| c.data_type.chars().count())
            .max()
            .unwrap_or(6);

        for c in &info.columns {
            let (marker, color) = if pk_set.contains(c.name.as_str()) {
                ('★', th.warning)
            } else if fk_map.contains_key(c.name.as_str()) {
                ('⚷', th.accent2)
            } else {
                (' ', th.fg)
            };
            let mut spans = vec![
                Span::raw("    "),
                Span::styled(
                    marker.to_string(),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("{:<w$}", c.name, w = name_w),
                    Style::default().fg(color),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("{:<w$}", c.data_type, w = type_w),
                    Style::default().fg(th.muted),
                ),
            ];
            if let Some((rt, rc)) = fk_map.get(c.name.as_str()) {
                spans.push(Span::styled("   → ", Style::default().fg(th.accent2)));
                spans.push(Span::styled(
                    rt.clone(),
                    Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(".", Style::default().fg(th.muted)));
                spans.push(Span::styled(rc.clone(), Style::default().fg(th.fg)));
            }
            lines.push(Line::from(spans));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "    (loading…)",
            Style::default().fg(th.muted),
        )));
    }
    lines.push(Line::default());

    // References → (outgoing FKs from this table).
    lines.push(section_header("References →", th));
    let outgoing: Vec<&RelationshipEdge> = app
        .relationships
        .iter()
        .filter(|e| &e.from_table == table)
        .collect();
    if outgoing.is_empty() {
        lines.push(Line::from(Span::styled(
            "    (none)",
            Style::default().fg(th.muted),
        )));
    } else {
        let n_w = outgoing.len().to_string().len();
        for (i, e) in outgoing.iter().enumerate() {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("    {:>w$}. ", i + 1, w = n_w),
                    Style::default().fg(th.muted),
                ),
                Span::styled(e.from_columns.join(", "), Style::default().fg(th.fg)),
                Span::styled("  →  ", Style::default().fg(th.accent2)),
                Span::styled(
                    e.to_table.clone(),
                    Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(".", Style::default().fg(th.muted)),
                Span::styled(e.to_columns.join(", "), Style::default().fg(th.fg)),
            ]));
        }
    }
    lines.push(Line::default());

    // Referenced by ← (incoming FKs to this table).
    lines.push(section_header("Referenced by ←", th));
    let incoming: Vec<&RelationshipEdge> = app
        .relationships
        .iter()
        .filter(|e| &e.to_table == table)
        .collect();
    if incoming.is_empty() {
        lines.push(Line::from(Span::styled(
            "    (none)",
            Style::default().fg(th.muted),
        )));
    } else {
        let n_w = incoming.len().to_string().len();
        for (i, e) in incoming.iter().enumerate() {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("    {:>w$}. ", i + 1, w = n_w),
                    Style::default().fg(th.muted),
                ),
                Span::styled(
                    e.from_table.clone(),
                    Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(".", Style::default().fg(th.muted)),
                Span::styled(e.from_columns.join(", "), Style::default().fg(th.fg)),
                Span::styled("  ←  ", Style::default().fg(th.accent2)),
                Span::styled(e.to_columns.join(", "), Style::default().fg(th.fg)),
            ]));
        }
    }
    lines.push(Line::default());

    // Mermaid source no longer dumps here — the chart pane above
    // shows the rendered diagram, and `y` still writes the source
    // out for sharing.
    let _ = tables;
    lines.push(Line::from(Span::styled(
        "    y → save Mermaid source to ./<schema>.mmd",
        Style::default().fg(th.muted).add_modifier(Modifier::ITALIC),
    )));

    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(th.bg))
            .wrap(Wrap { trim: false }),
        inner,
    );
}

fn section_header(label: &str, th: Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!("  {label}"),
        Style::default()
            .fg(th.accent2)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
    ))
}

/// Build a Mermaid `erDiagram` source for the active schema. Tables
/// without cached `TableInfo` render as empty bodies — the layout
/// still works, the columns just fill in once the prefetch completes.
fn mermaid_erdiagram(
    tables: &[String],
    edges: &[RelationshipEdge],
    cache: &HashMap<String, TableInfo>,
) -> String {
    fn ident(s: &str) -> String {
        // Mermaid identifiers: keep alphanumerics + `_`; map anything
        // else to `_`. Double underscores collapse to one for
        // readability.
        let mut out = String::with_capacity(s.len());
        let mut prev_us = false;
        for ch in s.chars() {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                out.push(ch);
                prev_us = ch == '_';
            } else if !prev_us {
                out.push('_');
                prev_us = true;
            }
        }
        out.trim_matches('_').to_owned()
    }
    fn typ(s: &str) -> String {
        // Mermaid wants single-token types. Replace whitespace and
        // parens; collapse to one token.
        ident(s)
    }
    let mut out = String::new();
    out.push_str("erDiagram\n");
    for t in tables {
        let name = ident(t);
        out.push_str("  ");
        out.push_str(&name);
        out.push_str(" {\n");
        if let Some(info) = cache.get(t) {
            let pk: HashSet<&str> = info
                .primary_key
                .as_ref()
                .map(|p| p.column_names.iter().map(String::as_str).collect())
                .unwrap_or_default();
            let fk: HashSet<&str> = info
                .foreign_keys
                .iter()
                .flat_map(|fk| fk.column_names.iter().map(String::as_str))
                .collect();
            for c in &info.columns {
                let role = if pk.contains(c.name.as_str()) {
                    " PK"
                } else if fk.contains(c.name.as_str()) {
                    " FK"
                } else {
                    ""
                };
                out.push_str(&format!(
                    "    {} {}{}\n",
                    typ(&c.data_type),
                    ident(&c.name),
                    role
                ));
            }
        }
        out.push_str("  }\n");
    }
    for e in edges {
        // FK side has many; PK side has one. `}o--||` reads
        // "many to one (optional on FK side)".
        out.push_str(&format!(
            "  {} }}o--|| {} : {}\n",
            ident(&e.from_table),
            ident(&e.to_table),
            ident(&e.from_columns.join("_"))
        ));
    }
    out
}

/// Write the Mermaid `erDiagram` for the active schema to
/// `./<schema>.mmd`. Returns the path on success or an error string.
async fn save_mermaid_for_schema(app: &AppState) -> Result<PathBuf, String> {
    if app.current_schema.is_empty() {
        return Err("no active schema".to_owned());
    }
    let tables = erd_table_list(app);
    let body = mermaid_erdiagram(&tables, &app.relationships, &app.erd_table_info);
    let path = PathBuf::from(format!("{}.mmd", app.current_schema));
    tokio::fs::write(&path, body.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    Ok(path)
}

// ─── SQL editor pane ──────────────────────────────────────────────────────────

fn draw_editor(f: &mut Frame<'_>, app: &AppState, area: Rect) {
    let th = app.theme;
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);

    let title = {
        let mut t = String::from("  SQL Editor  ");
        if let Some(p) = &app.editor_path {
            t.push_str(&p.display().to_string());
            t.push_str("  ");
        }
        let (cl, cc) = line_col(&app.editor, app.editor_cursor);
        let total_lines = app.editor.lines().count().max(1);
        t.push_str(&format!("[Ln {}:{} / {}]  ", cl + 1, cc + 1, total_lines));
        t.push_str("Ctrl+R run all  Ctrl+Enter run current  Ctrl+S save  ");
        if !app.history.is_empty() {
            t.push_str(&format!("Ctrl+P/N hist ({})  ", app.history.len()));
        }
        t.push_str("Esc browser  ");
        t
    };

    let editor_area = layout[0];
    let editor_block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(th.active_border))
        .style(Style::default().bg(th.bg));
    let editor_inner = editor_block.inner(editor_area);
    f.render_widget(editor_block, editor_area);

    // Split the inner area into [gutter | text]. The gutter holds 1-indexed
    // line numbers right-aligned to a 4-char column (max 9999 lines).
    let line_count = app.editor.lines().count().max(1);
    let gutter_width = (line_count.to_string().len() as u16 + 1).max(4);
    let editor_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(gutter_width), Constraint::Min(0)])
        .split(editor_inner);
    let gutter_area = editor_layout[0];
    let text_area = editor_layout[1];
    let viewport_h = text_area.height as usize;

    // Current-statement byte range, used to highlight the active SQL.
    let stmt_range = statement_range_at(&app.editor, app.editor_cursor);

    let (cursor_line, cursor_col) = line_col(&app.editor, app.editor_cursor);

    // Auto-scroll: keep the cursor row inside the viewport. Two-step
    // clamp — first push the offset down if cursor fell off the bottom,
    // then up if cursor scrolled above the top. Single-line jumps don't
    // surprise; jumping to top/bottom (Ctrl+Home equivalents) snaps.
    let mut scroll = app.editor_scroll.get();
    if viewport_h > 0 {
        if cursor_line >= scroll + viewport_h {
            scroll = cursor_line + 1 - viewport_h;
        }
        if cursor_line < scroll {
            scroll = cursor_line;
        }
        let max_scroll = line_count.saturating_sub(viewport_h);
        scroll = scroll.min(max_scroll);
        app.editor_scroll.set(scroll);
    }

    let mut byte_offset = 0usize;
    let mut gutter_lines: Vec<Line> = Vec::new();
    let mut text_lines: Vec<Line> = Vec::new();
    for (idx, line) in app.editor.split('\n').enumerate() {
        let line_start = byte_offset;
        let line_end = line_start + line.len();
        byte_offset = line_end + 1; // +1 for the '\n' we split on
        if idx < scroll {
            continue;
        }
        if idx >= scroll + viewport_h && viewport_h > 0 {
            break;
        }
        let in_stmt = stmt_range.start <= line_end && line_start <= stmt_range.end;
        let line_bg = if in_stmt && !line.trim().is_empty() {
            th.row_alt_bg
        } else {
            th.bg
        };
        let num_style = if idx == cursor_line {
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(th.muted)
        };
        gutter_lines.push(Line::from(Span::styled(
            format!(
                "{:>width$} ",
                idx + 1,
                width = (gutter_width as usize).saturating_sub(1)
            ),
            num_style.bg(line_bg),
        )));

        let mut spans = highlight_line(line, th);
        for span in &mut spans {
            let mut st = span.style;
            st.bg = Some(line_bg);
            span.style = st;
        }
        if spans.is_empty() {
            spans.push(Span::styled(" ", Style::default().bg(line_bg)));
        }
        text_lines.push(Line::from(spans));
    }

    f.render_widget(
        Paragraph::new(gutter_lines).style(Style::default().bg(th.bg)),
        gutter_area,
    );
    f.render_widget(
        Paragraph::new(text_lines).style(Style::default().fg(th.fg).bg(th.bg)),
        text_area,
    );

    // Position the terminal's hardware cursor inside the text pane,
    // accounting for the active scroll offset so it lands on the right
    // visible row.
    let visible_row = cursor_line.saturating_sub(scroll);
    let cx = text_area.x + (cursor_col as u16).min(text_area.width.saturating_sub(1));
    let cy = text_area.y + (visible_row as u16).min(text_area.height.saturating_sub(1));
    f.set_cursor_position((cx, cy));

    // ── Results pane: same grid as the Records tab ──
    let results_block = Block::default()
        .title(Span::styled("  Results  ", Style::default().fg(th.muted)))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(th.border))
        .style(Style::default().bg(th.bg));
    let results_inner = results_block.inner(layout[1]);
    f.render_widget(results_block, layout[1]);
    draw_records(f, app, results_inner);
}

// ─── Status bar ───────────────────────────────────────────────────────────────

fn draw_status(f: &mut Frame<'_>, app: &AppState, area: Rect) {
    let th = app.theme;
    let (text, fg) = if let Some(buf) = &app.command_input {
        // Active palette: render `:<input>_` and route the hardware cursor
        // there so the user sees their typing.
        let prompt = format!(":{buf}");
        let cursor_x = area.x + prompt.chars().count() as u16;
        let cursor_y = area.y;
        f.set_cursor_position((cursor_x, cursor_y));
        (prompt, th.accent)
    } else if let Some(err) = &app.last_error {
        (format!(" ✗  {err}"), th.error)
    } else {
        (format!(" {}", app.status), th.muted)
    };
    f.render_widget(
        Paragraph::new(text).style(Style::default().fg(fg).bg(th.bg)),
        area,
    );
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn muted_para<'a>(text: &'a str, th: Theme) -> Paragraph<'a> {
    Paragraph::new(Span::styled(text, Style::default().fg(th.muted)))
        .style(Style::default().bg(th.bg))
}

fn setup_terminal() -> Result<Terminal<ratatui::backend::CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // Bracketed paste lets us receive a multi-line clipboard in a single
    // `Event::Paste(String)` instead of streaming each character — it's
    // both faster and keeps history sane (one paste = one history entry).
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    Terminal::new(ratatui::backend::CrosstermBackend::new(stdout)).map_err(Into::into)
}

fn restore_terminal(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<Stdout>>,
) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        close_current_table, handle_paste, line_col, line_end, line_start, move_cursor_vertical,
        next_char_boundary, prev_char_boundary, short_hash, unique_connection_name, AppMode,
        AppState, ConnectionConfig, DetailTab, DriverKind, Theme, ALL_TABS,
    };

    #[test]
    fn catppuccin_mocha_name() {
        assert_eq!(Theme::catppuccin_mocha().name, "catppuccin-mocha");
    }

    #[test]
    fn detail_tab_cycles_forward_and_back() {
        let first = ALL_TABS[0];
        let last = *ALL_TABS.last().unwrap();
        assert_eq!(last.next(), first, "last tab wraps to first");
        assert_eq!(first.prev(), last, "first tab wraps to last");
    }

    #[test]
    fn all_tabs_have_labels() {
        for tab in ALL_TABS {
            assert!(!tab.label().is_empty());
        }
    }

    #[test]
    fn hotkey_label_prefixes_index() {
        // Tab order is fixed by ALL_TABS, so labels must start with
        // their 1-based hotkey to match the `1`-`6` shortcuts.
        for (idx, tab) in ALL_TABS.iter().enumerate() {
            let expected = format!("{} {}", idx + 1, tab.label());
            assert_eq!(tab.hotkey_label(), expected);
        }
    }

    #[test]
    fn char_boundary_walks_skip_utf8_continuation_bytes() {
        // "ñ" is 2 bytes; cursor at 0 → next at 2, then prev → 0.
        let s = "ñ";
        assert_eq!(next_char_boundary(s, 0), 2);
        assert_eq!(prev_char_boundary(s, 2), 0);
        assert_eq!(prev_char_boundary("", 0), 0);
        assert_eq!(next_char_boundary("", 0), 0);
    }

    #[test]
    fn line_start_and_line_end_handle_multiline() {
        let s = "select 1;\nselect 2;\nselect 3;";
        // Cursor in the middle of line 1 (0-indexed)
        let mid = 14;
        assert_eq!(line_start(s, mid), 10, "line 1 starts after first newline");
        assert_eq!(line_end(s, mid), 19, "line 1 ends at second newline");
        // Beginning of buffer: line_start = 0
        assert_eq!(line_start(s, 3), 0);
        // Last line has no trailing newline: line_end is buffer length
        assert_eq!(line_end(s, 25), s.len());
    }

    #[test]
    fn line_col_counts_lines_and_columns() {
        let s = "abc\ndé\nxyz";
        // After 'b' on line 0 → (0, 1)
        assert_eq!(line_col(s, 1), (0, 1));
        // After 'é' on line 1: 'd' at byte 4, 'é' is 2 bytes ending at 7 → (1, 2)
        assert_eq!(line_col(s, 7), (1, 2));
        // Empty buffer
        assert_eq!(line_col("", 0), (0, 0));
    }

    #[test]
    fn vertical_cursor_preserves_column() {
        let s = "abcdef\n12\nXYZ";
        // From column 4 of line 0 (after 'd'): byte idx 4
        // Down → line 1 has only 2 chars, so we land at end of "12" (byte 9).
        assert_eq!(move_cursor_vertical(s, 4, 1), 9);
        // Up from start does nothing
        assert_eq!(move_cursor_vertical(s, 2, -1), 2);
        // Down from last line does nothing
        assert_eq!(move_cursor_vertical(s, s.len(), 1), s.len());
    }

    #[test]
    fn vertical_cursor_round_trip() {
        let s = "alpha\nbeta\ngamma";
        // line 0, col 3 → byte 3
        let down = move_cursor_vertical(s, 3, 1);
        // back up should land on column 3 of line 0
        assert_eq!(move_cursor_vertical(s, down, -1), 3);
    }

    #[test]
    fn close_current_table_clears_table_state() {
        let mut app = AppState::new(DriverKind::Sqlite, "sqlite::memory:".to_owned());
        app.current_schema = "public".to_owned();
        app.current_table = "customers".to_owned();
        app.detail_tab = DetailTab::Columns;
        close_current_table(&mut app);
        assert!(app.current_table.is_empty());
        assert!(app.records.is_none());
        assert!(app.table_info.is_none());
        assert_eq!(app.detail_tab, DetailTab::Records);
    }

    #[test]
    fn unique_name_appends_numeric_suffix_when_taken() {
        let saved = vec![
            (
                "prod".to_owned(),
                ConnectionConfig {
                    driver: DriverKind::Postgres,
                    url: "u1".to_owned(),
                },
            ),
            (
                "prod-2".to_owned(),
                ConnectionConfig {
                    driver: DriverKind::Postgres,
                    url: "u2".to_owned(),
                },
            ),
        ];
        assert_eq!(unique_connection_name("prod", &saved), "prod-3");
        assert_eq!(unique_connection_name("dev", &saved), "dev");
    }

    #[test]
    fn mermaid_erdiagram_emits_columns_and_relationships() {
        use std::collections::HashMap;
        use tsqlx_db::{ColumnInfo, PrimaryKeyInfo, RelationshipEdge, TableInfo};

        let users = TableInfo {
            name: "users".to_owned(),
            schema: "public".to_owned(),
            columns: vec![ColumnInfo {
                name: "id".to_owned(),
                data_type: "int4".to_owned(),
                is_nullable: false,
                default_value: None,
            }],
            indexes: vec![],
            primary_key: Some(PrimaryKeyInfo {
                name: "users_pk".to_owned(),
                column_names: vec!["id".to_owned()],
            }),
            foreign_keys: vec![],
            constraints: vec![],
        };
        let orders = TableInfo {
            name: "orders".to_owned(),
            schema: "public".to_owned(),
            columns: vec![
                ColumnInfo {
                    name: "id".to_owned(),
                    data_type: "int4".to_owned(),
                    is_nullable: false,
                    default_value: None,
                },
                ColumnInfo {
                    name: "user_id".to_owned(),
                    data_type: "int4".to_owned(),
                    is_nullable: false,
                    default_value: None,
                },
            ],
            indexes: vec![],
            primary_key: Some(PrimaryKeyInfo {
                name: "orders_pk".to_owned(),
                column_names: vec!["id".to_owned()],
            }),
            foreign_keys: vec![tsqlx_db::ForeignKeyInfo {
                name: "orders_user_fk".to_owned(),
                column_names: vec!["user_id".to_owned()],
                referenced_table: "users".to_owned(),
                referenced_columns: vec!["id".to_owned()],
            }],
            constraints: vec![],
        };

        let mut cache: HashMap<String, TableInfo> = HashMap::new();
        cache.insert("users".to_owned(), users);
        cache.insert("orders".to_owned(), orders);

        let tables = vec!["orders".to_owned(), "users".to_owned()];
        let edges = vec![RelationshipEdge {
            from_table: "orders".to_owned(),
            from_columns: vec!["user_id".to_owned()],
            to_table: "users".to_owned(),
            to_columns: vec!["id".to_owned()],
        }];

        let mmd = super::mermaid_erdiagram(&tables, &edges, &cache);
        assert!(mmd.starts_with("erDiagram\n"), "header present");
        assert!(mmd.contains("orders {"), "orders block opens");
        assert!(mmd.contains("users {"), "users block opens");
        assert!(mmd.contains("int4 id PK"), "PK role tagged");
        assert!(mmd.contains("int4 user_id FK"), "FK role tagged");
        assert!(
            mmd.contains("orders }o--|| users : user_id"),
            "FK relationship line present"
        );
        let _ = Theme::catppuccin_mocha();
    }

    #[test]
    fn short_hash_is_deterministic_and_alphanumeric() {
        let a = short_hash("postgres://localhost/x");
        let b = short_hash("postgres://localhost/x");
        let c = short_hash("postgres://localhost/y");
        assert_eq!(a, b, "same input produces same hash");
        assert_ne!(a, c, "different input produces different hash");
        assert!(a.starts_with("url_"));
        assert!(a[4..].chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn focus_canvas_renders_centre_box_and_arrows() {
        use tsqlx_db::{ColumnInfo, ForeignKeyInfo, PrimaryKeyInfo, RelationshipEdge, TableInfo};

        let orders = TableInfo {
            name: "orders".to_owned(),
            schema: "public".to_owned(),
            columns: vec![
                ColumnInfo {
                    name: "id".to_owned(),
                    data_type: "int4".to_owned(),
                    is_nullable: false,
                    default_value: None,
                },
                ColumnInfo {
                    name: "customer_id".to_owned(),
                    data_type: "int4".to_owned(),
                    is_nullable: false,
                    default_value: None,
                },
            ],
            indexes: vec![],
            primary_key: Some(PrimaryKeyInfo {
                name: "pk".to_owned(),
                column_names: vec!["id".to_owned()],
            }),
            foreign_keys: vec![ForeignKeyInfo {
                name: "fk".to_owned(),
                column_names: vec!["customer_id".to_owned()],
                referenced_table: "customers".to_owned(),
                referenced_columns: vec!["id".to_owned()],
            }],
            constraints: vec![],
        };
        let edge = RelationshipEdge {
            from_table: "orders".to_owned(),
            from_columns: vec!["customer_id".to_owned()],
            to_table: "customers".to_owned(),
            to_columns: vec!["id".to_owned()],
        };
        let outgoing: Vec<&RelationshipEdge> = vec![&edge];
        let incoming: Vec<&RelationshipEdge> = Vec::new();
        let lines = super::render_focus_canvas(
            80,
            14,
            "orders",
            Some(&orders),
            &incoming,
            &outgoing,
            Theme::catppuccin_mocha(),
        );
        let dump: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(dump.contains("orders"), "centre table name rendered");
        assert!(dump.contains("customers"), "neighbour rendered");
        assert!(dump.contains("customer_id"), "FK column on the arrow label");
        assert!(dump.contains('▶'), "arrowhead drawn");
        assert!(dump.contains('★'), "PK marker on centre column");
        assert!(dump.contains('⚷'), "FK marker on centre column");
    }

    #[test]
    fn focus_canvas_renders_neighbours_in_half_width_pane() {
        // Half-screen pane: 60 cols × 12 rows. Centre + at least one
        // neighbour should both be visible.
        use tsqlx_db::{ColumnInfo, ForeignKeyInfo, PrimaryKeyInfo, RelationshipEdge, TableInfo};
        let orders = TableInfo {
            name: "orders".to_owned(),
            schema: "public".to_owned(),
            columns: vec![
                ColumnInfo {
                    name: "id".to_owned(),
                    data_type: "int4".to_owned(),
                    is_nullable: false,
                    default_value: None,
                },
                ColumnInfo {
                    name: "customer_id".to_owned(),
                    data_type: "int4".to_owned(),
                    is_nullable: false,
                    default_value: None,
                },
            ],
            indexes: vec![],
            primary_key: Some(PrimaryKeyInfo {
                name: "pk".to_owned(),
                column_names: vec!["id".to_owned()],
            }),
            foreign_keys: vec![ForeignKeyInfo {
                name: "fk".to_owned(),
                column_names: vec!["customer_id".to_owned()],
                referenced_table: "customers".to_owned(),
                referenced_columns: vec!["id".to_owned()],
            }],
            constraints: vec![],
        };
        let edge = RelationshipEdge {
            from_table: "orders".to_owned(),
            from_columns: vec!["customer_id".to_owned()],
            to_table: "customers".to_owned(),
            to_columns: vec!["id".to_owned()],
        };
        let outgoing: Vec<&RelationshipEdge> = vec![&edge];
        let incoming: Vec<&RelationshipEdge> = Vec::new();
        let lines = super::render_focus_canvas(
            60,
            12,
            "orders",
            Some(&orders),
            &incoming,
            &outgoing,
            Theme::catppuccin_mocha(),
        );
        let dump: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(dump.contains("orders"), "centre still rendered at 60 cols");
        assert!(
            dump.contains("customers"),
            "neighbour still rendered at 60 cols"
        );
        assert!(dump.contains('▶'), "arrowhead present at narrow width");
        // Line widths must not exceed the requested pane width.
        for line in &lines {
            let len: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            assert!(len <= 60, "line of {len} chars exceeds 60-col pane");
        }
    }

    #[test]
    fn handle_paste_inserts_multiline_at_cursor_in_editor() {
        let mut app = AppState::new(DriverKind::Sqlite, "sqlite::memory:".to_owned());
        app.mode = AppMode::Editor;
        app.editor = "SELECT 1;".to_owned();
        app.editor_cursor = app.editor.len();
        handle_paste(&mut app, "\nSELECT 2;\nSELECT 3;");
        assert_eq!(app.editor, "SELECT 1;\nSELECT 2;\nSELECT 3;");
        assert_eq!(app.editor_cursor, app.editor.len());
        assert!(app.status.contains("3 line"), "status reports line count");
    }

    #[test]
    fn handle_paste_normalises_crlf_to_lf() {
        let mut app = AppState::new(DriverKind::Sqlite, "sqlite::memory:".to_owned());
        app.mode = AppMode::Editor;
        handle_paste(&mut app, "a\r\nb\r\nc");
        assert_eq!(app.editor, "a\nb\nc", "CRLF and stray CR collapse to LF");
    }

    #[test]
    fn driver_kind_detects_mysql_and_mariadb_urls() {
        assert_eq!(
            DriverKind::from_url("mysql://u:p@h/db").unwrap(),
            DriverKind::Mysql
        );
        assert_eq!(
            DriverKind::from_url("mariadb://u:p@h/db").unwrap(),
            DriverKind::Mysql
        );
        assert_eq!(
            DriverKind::Mysql.normalise_url("mariadb://u:p@h/db"),
            "mysql://u:p@h/db",
            "mariadb scheme rewrites to mysql for sqlx"
        );
    }

    #[test]
    fn theme_registry_lookup_and_cycle() {
        assert_eq!(Theme::by_name("catppuccin-mocha").name, "catppuccin-mocha");
        assert_eq!(Theme::by_name("tokyo-night").name, "tokyo-night");
        assert_eq!(
            Theme::by_name("does-not-exist").name,
            "catppuccin-mocha",
            "unknown name falls back to default"
        );

        let mut t = Theme::catppuccin_mocha();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..Theme::all().len() {
            seen.insert(t.name);
            t = t.next_in_cycle();
        }
        assert_eq!(t.name, "catppuccin-mocha", "cycle returns to start");
        assert_eq!(seen.len(), Theme::all().len(), "every theme visited once");
    }
}
