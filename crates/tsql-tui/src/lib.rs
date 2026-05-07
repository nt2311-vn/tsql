use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::Duration;

mod editor;
use editor::{append_history, highlight_line, history_path, load_history, statement_range_at};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
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
use tsql_core::{append_connection, default_config_path, ConnectionConfig, DriverKind};
use tsql_db::{DatabaseOverview, Pool, RelationshipEdge, StatementOutput, TableInfo};
use tsql_sql::SqlDocument;

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
    /// the connection can be persisted to `~/.config/tsql/config.toml`.
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
    /// `tsql_core::default_config_path()`.
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
    record_table_state: TableState,
    relationships: Vec<RelationshipEdge>,
    /// Row index into `relationships` selected on the ERD tab. Used by
    /// j/k navigation and the Enter / `o` jump-to-table shortcuts.
    erd_selected: usize,
    /// Schema-scoped `TableInfo` cache feeding the ERD card-grid
    /// renderer. Cleared on schema change. Tables that have not yet
    /// been pre-fetched render as a name-only stub card.
    erd_table_info: HashMap<String, TableInfo>,
    editor: String,
    /// Byte index of the cursor within `editor`. Always sits on a UTF-8
    /// char boundary.
    editor_cursor: usize,
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
            record_table_state: ts,
            relationships: Vec::new(),
            erd_selected: 0,
            erd_table_info: HashMap::new(),
            editor: String::new(),
            editor_cursor: 0,
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

pub async fn run(driver: DriverKind, url: String) -> Result<()> {
    let mut terminal = setup_terminal()?;
    let mut app = AppState::new(driver, url);
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
    let mut terminal = setup_terminal()?;
    let mut app = AppState::new(DriverKind::Postgres, String::new());
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
        if let Event::Key(key) = event::read()? {
            if handle_key(app, key).await? {
                break;
            }
        }
    }
    Ok(())
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

async fn handle_key(app: &mut AppState, key: KeyEvent) -> Result<bool> {
    // Ctrl+C is always a hard quit, even from text-entry contexts.
    if (key.code, key.modifiers) == (KeyCode::Char('c'), KeyModifiers::CONTROL) {
        return Ok(true);
    }

    // The command palette steals all input while open.
    if app.command_input.is_some() {
        return handle_command_key(app, key).await;
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
            app.driver = match app.driver {
                DriverKind::Postgres => DriverKind::Sqlite,
                DriverKind::Sqlite => DriverKind::Postgres,
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
/// (created by other tsql sessions), but that's a corner case we accept.
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
async fn open_pool_and_overview(app: &mut AppState) -> Result<DatabaseOverview, tsql_db::DbError> {
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
        // ERD navigation: move the highlight through the flat edge list.
        KeyCode::Char('j') | KeyCode::Down
            if app.detail_tab == DetailTab::Erd && !app.relationships.is_empty() =>
        {
            let max = app.relationships.len() - 1;
            app.erd_selected = (app.erd_selected + 1).min(max);
        }
        KeyCode::Char('k') | KeyCode::Up
            if app.detail_tab == DetailTab::Erd && !app.relationships.is_empty() =>
        {
            app.erd_selected = app.erd_selected.saturating_sub(1);
        }
        KeyCode::Enter if app.detail_tab == DetailTab::Erd && !app.relationships.is_empty() => {
            jump_to_erd_target(app, ErdJump::Target).await;
        }
        KeyCode::Char('o') if app.detail_tab == DetailTab::Erd && !app.relationships.is_empty() => {
            jump_to_erd_target(app, ErdJump::Source).await;
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

/// Direction of an ERD jump: follow the FK arrow to its `to_table`, or
/// hop back to the owning `from_table`.
#[derive(Debug, Clone, Copy)]
enum ErdJump {
    Target,
    Source,
}

/// Load the table at the other end of the highlighted ERD edge. Schema is
/// inherited from the current schema (FK edges are schema-scoped). Updates
/// the sidebar selection so the new table is the active context for
/// further navigation.
async fn jump_to_erd_target(app: &mut AppState, direction: ErdJump) {
    let Some(edge) = app.relationships.get(app.erd_selected).cloned() else {
        return;
    };
    let target = match direction {
        ErdJump::Target => edge.to_table,
        ErdJump::Source => edge.from_table,
    };
    let schema = app.current_schema.clone();
    select_sidebar_for(app, &target);
    app.detail_tab = DetailTab::Records;
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

    // Build a zebra-striped grid with vertical column separators. The
    // widths vector interleaves data columns with 1-wide separator
    // columns so ratatui's Table draws them as siblings; we render a
    // styled `│` glyph in each separator slot.
    let col_count = rec.columns.len().max(1);
    let total_pct = 100u16;
    let sep_count = col_count.saturating_sub(1) as u16;
    let sep_width = sep_count; // 1 cell each
    let data_pct = (total_pct.saturating_sub(sep_width) / col_count as u16).max(1);
    let widths: Vec<Constraint> = build_grid_widths(col_count, data_pct);

    let header_cells: Vec<Cell<'_>> = interleave_separators(
        rec.columns.iter().enumerate().map(|(ci, name)| {
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
                row.iter().enumerate().map(|(ci, val)| {
                    let st = if ri == app.record_row && ci == app.record_col {
                        Style::default().fg(th.bg).bg(th.accent)
                    } else if ri == app.record_row {
                        Style::default().fg(th.sel_fg).bg(row_bg)
                    } else if val == "NULL" {
                        Style::default().fg(th.muted).bg(row_bg)
                    } else {
                        Style::default().fg(th.fg).bg(row_bg)
                    };
                    ccell(val.as_str(), st)
                }),
                th,
                row_bg,
            );
            Row::new(cells).style(Style::default().bg(row_bg)).height(1)
        })
        .collect();

    let mut ts = app.record_table_state;
    f.render_stateful_widget(
        Table::new(rows, widths)
            .header(header)
            .row_highlight_style(Style::default().bg(th.sel_bg)),
        area,
        &mut ts,
    );
}

/// Build a centred `Cell` with the given style. Used by every detail
/// tab so headers and body values share the same alignment treatment.
fn ccell<'a>(content: impl Into<std::borrow::Cow<'a, str>>, style: Style) -> Cell<'a> {
    Cell::from(Line::from(Span::raw(content)).alignment(Alignment::Center)).style(style)
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

    f.render_widget(
        Table::new(
            rows,
            [
                Constraint::Percentage(25),
                Constraint::Percentage(22),
                Constraint::Percentage(6),
                Constraint::Percentage(6),
                Constraint::Percentage(12),
                Constraint::Percentage(29),
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
                Constraint::Percentage(30),
                Constraint::Percentage(35),
                Constraint::Length(8),
                Constraint::Length(4),
                Constraint::Percentage(15),
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
                Constraint::Length(6),
                Constraint::Percentage(30),
                Constraint::Percentage(30),
                Constraint::Percentage(34),
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
        Table::new(
            rows,
            [Constraint::Percentage(35), Constraint::Percentage(65)],
        )
        .header(header),
        area,
    );
}

// ─── ERD tab ──────────────────────────────────────────────────────────────────
//
// Earlier iterations of this view tried to draw a 2-D graph with
// orthogonal-routed edges. On real schemas the routes ran straight
// through table boxes and the result was unreadable. We've since
// pivoted to a **focused vertical view** that trades graph topology
// for clarity:
//
//   1. **Tables** — a compact chip strip of every table in the active
//      schema. Tables that participate in any FK render in `accent`;
//      standalone tables in `muted` so the user sees the full set.
//   2. **Selected edge focus** — two stacked rounded-corner boxes
//      stacked vertically, source on top, target below, with a
//      labelled vertical arrow between them and the column names
//      printed inside each box. This is the "what does this edge
//      mean" panel.
//   3. **All edges** — a numbered list of every relationship with
//      the selected one bolded and prefixed with `►`. `j/k` cycles
//      the selection, `o` jumps to the target table.
//
// No autorouting, no overlapping lines, no cognitive load to decode
// crossings. Every relationship is one Enter / `o` away from being
// inspected in detail.

fn draw_erd(f: &mut Frame<'_>, app: &AppState, area: Rect) {
    let th = app.theme;
    let schema = &app.current_schema;

    // Collect the table set: every table that participates in an edge,
    // plus all tables in the active schema (so standalone tables still
    // show as boxes).
    let mut table_set: BTreeSet<String> = BTreeSet::new();
    for e in &app.relationships {
        table_set.insert(e.from_table.clone());
        table_set.insert(e.to_table.clone());
    }
    if let Some(ov) = &app.overview {
        if let Some(si) = ov.schemas.iter().find(|s| s.name == *schema) {
            for t in &si.tables {
                table_set.insert(t.clone());
            }
        }
    }
    let tables: Vec<String> = table_set.into_iter().collect();
    let referenced: HashSet<&str> = app
        .relationships
        .iter()
        .flat_map(|e| [e.from_table.as_str(), e.to_table.as_str()])
        .collect();

    if tables.is_empty() {
        let hint = if schema.is_empty() {
            "  No schema selected."
        } else {
            "  No tables in this schema."
        };
        f.render_widget(muted_para(hint, th), area);
        return;
    }

    // ── Build the lines top-down ───────────────────────────────────
    let mut lines: Vec<Line> = Vec::new();

    // Banner.
    lines.push(Line::from(vec![
        Span::styled(
            format!("  ERD  schema: {schema}  "),
            Style::default()
                .fg(th.bg)
                .bg(th.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "  {} table(s), {} relationship(s)  j/k select  o jump  ",
                tables.len(),
                app.relationships.len()
            ),
            Style::default().fg(th.muted),
        ),
    ]));
    lines.push(Line::default());

    // ── Section 1: card-grid diagram ───────────────────────────────
    // Each table becomes a vertical card with PK/FK badges next to
    // every column; FK edges route from the FK column-row of the
    // source card to the PK column-row of the target card.
    lines.push(Line::from(Span::styled(
        "  Diagram",
        Style::default()
            .fg(th.accent2)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
    )));
    lines.extend(render_card_erd(
        &tables,
        &app.relationships,
        app.erd_selected,
        &app.erd_table_info,
        th,
    ));
    lines.push(Line::default());
    let _ = referenced; // referenced was only used by the old chip strip

    // ── Section 2: focused selected edge ───────────────────────────
    let selected = app
        .erd_selected
        .min(app.relationships.len().saturating_sub(1));
    if app.relationships.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No foreign-key relationships in this schema.",
            Style::default().fg(th.muted),
        )));
    } else if let Some(edge) = app.relationships.get(selected) {
        lines.push(Line::from(Span::styled(
            format!(
                "  Relationship  {} / {}",
                selected + 1,
                app.relationships.len()
            ),
            Style::default()
                .fg(th.accent2)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        lines.push(Line::default());
        // Compute box width so both boxes share a consistent size and
        // the column labels fit comfortably.
        let from_label = format!("column: {}", edge.from_columns.join(", "));
        let to_label = format!("column: {}", edge.to_columns.join(", "));
        let inner_w = [
            edge.from_table.chars().count(),
            edge.to_table.chars().count(),
            from_label.chars().count(),
            to_label.chars().count(),
        ]
        .into_iter()
        .max()
        .unwrap_or(20)
            + 6;
        let inner_w = inner_w.max(28);
        let indent = "         ";
        // Source box (top).
        lines.extend(render_erd_box(
            indent,
            &edge.from_table,
            &from_label,
            inner_w,
            th,
            true,
        ));
        // Vertical connector with the FK label centered.
        let bar = format!("{}{:^w$}", indent, "│", w = inner_w);
        let label_line = format!("{}{:^w$}", indent, " FOREIGN KEY ", w = inner_w);
        let arrow = format!("{}{:^w$}", indent, "▼", w = inner_w);
        lines.push(Line::from(Span::styled(
            bar.clone(),
            Style::default().fg(th.warning).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            label_line,
            Style::default()
                .fg(th.bg)
                .bg(th.warning)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            bar,
            Style::default().fg(th.warning).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            arrow,
            Style::default().fg(th.warning).add_modifier(Modifier::BOLD),
        )));
        // Target box (bottom).
        lines.extend(render_erd_box(
            indent,
            &edge.to_table,
            &to_label,
            inner_w,
            th,
            true,
        ));
        lines.push(Line::default());
    }

    // ── Section 3: numbered list of all edges ──────────────────────
    if !app.relationships.is_empty() {
        lines.push(Line::from(Span::styled(
            "  All relationships",
            Style::default()
                .fg(th.accent2)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        let max_n = app.relationships.len();
        let n_width = max_n.to_string().len();
        for (idx, edge) in app.relationships.iter().enumerate() {
            let is_sel = idx == selected;
            let row_bg = if is_sel { th.sel_bg } else { th.bg };
            let prefix = if is_sel { "►" } else { " " };
            let prefix_style = if is_sel {
                Style::default()
                    .fg(th.warning)
                    .bg(row_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(th.muted).bg(row_bg)
            };
            let num_style = Style::default().fg(th.muted).bg(row_bg);
            let table_style = Style::default()
                .fg(th.accent)
                .bg(row_bg)
                .add_modifier(Modifier::BOLD);
            let col_style = Style::default().fg(th.fg).bg(row_bg);
            let dot_style = Style::default().fg(th.muted).bg(row_bg);
            let arrow_style = if is_sel {
                Style::default()
                    .fg(th.warning)
                    .bg(row_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(th.muted).bg(row_bg)
            };
            lines.push(Line::from(vec![
                Span::styled(format!("  {prefix} "), prefix_style),
                Span::styled(format!("{:>w$}.", idx + 1, w = n_width), num_style),
                Span::styled("  ", col_style),
                Span::styled(edge.from_table.clone(), table_style),
                Span::styled(".", dot_style),
                Span::styled(edge.from_columns.join(", "), col_style),
                Span::styled("  ─▶  ", arrow_style),
                Span::styled(edge.to_table.clone(), table_style),
                Span::styled(".", dot_style),
                Span::styled(edge.to_columns.join(", "), col_style),
            ]));
        }
    }

    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(th.bg))
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// Render one rounded-corner box centered at `indent` width `inner_w`,
/// containing a bold title and a muted subtitle line. Returns three
/// `Line`s: top border, title row, and a divider+subtitle+bottom row
/// pair so the boxes always have the same height regardless of label.
fn render_erd_box(
    indent: &str,
    title: &str,
    subtitle: &str,
    inner_w: usize,
    th: Theme,
    accented: bool,
) -> Vec<Line<'static>> {
    let border = Style::default().fg(th.border);
    let title_style = if accented {
        Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(th.fg).add_modifier(Modifier::BOLD)
    };
    let sub_style = Style::default().fg(th.muted);

    let top = format!("╭{}╮", "─".repeat(inner_w));
    let bottom = format!("╰{}╯", "─".repeat(inner_w));
    let divider = format!("├{}┤", "─".repeat(inner_w));

    let title_pad_l = inner_w.saturating_sub(title.chars().count()) / 2;
    let title_pad_r = inner_w
        .saturating_sub(title.chars().count())
        .saturating_sub(title_pad_l);
    let sub_pad_l = inner_w.saturating_sub(subtitle.chars().count()) / 2;
    let sub_pad_r = inner_w
        .saturating_sub(subtitle.chars().count())
        .saturating_sub(sub_pad_l);

    vec![
        Line::from(vec![
            Span::raw(indent.to_owned()),
            Span::styled(top, border),
        ]),
        Line::from(vec![
            Span::raw(indent.to_owned()),
            Span::styled("│", border),
            Span::raw(" ".repeat(title_pad_l)),
            Span::styled(title.to_owned(), title_style),
            Span::raw(" ".repeat(title_pad_r)),
            Span::styled("│", border),
        ]),
        Line::from(vec![
            Span::raw(indent.to_owned()),
            Span::styled(divider, border),
        ]),
        Line::from(vec![
            Span::raw(indent.to_owned()),
            Span::styled("│", border),
            Span::raw(" ".repeat(sub_pad_l)),
            Span::styled(subtitle.to_owned(), sub_style),
            Span::raw(" ".repeat(sub_pad_r)),
            Span::styled("│", border),
        ]),
        Line::from(vec![
            Span::raw(indent.to_owned()),
            Span::styled(bottom, border),
        ]),
    ]
}

/// One cell of the layered-ERD char canvas.
#[derive(Clone, Copy)]
struct ErdCell {
    ch: char,
    fg: Color,
    bold: bool,
}

/// Pre-computed geometry + content for one ERD card. The renderer
/// walks `rows` to draw the body; `col_y` lets the edge router
/// attach to a specific column row by name.
struct ErdCard<'a> {
    name: &'a str,
    /// One entry per data row (columns). Each is `(marker, color,
    /// name, type)` where `marker` is one of `★`, `⚷`, or ` `.
    rows: Vec<(char, Color, &'a str, &'a str)>,
    /// Total inner width (between the two border `│`s).
    inner_w: usize,
    /// Map column-name → row index inside `rows` (0-based).
    col_y: HashMap<&'a str, usize>,
}

impl ErdCard<'_> {
    /// Outer width including both border columns.
    fn width(&self) -> usize {
        self.inner_w + 2
    }
    /// Total height: top border + N column rows + bottom border. When
    /// no metadata is cached we fall back to a 3-row name-only stub.
    fn height(&self) -> usize {
        2 + self.rows.len().max(1)
    }
}

/// Render a dbdiagram.io-style card-grid ERD into a list of styled lines.
///
/// Each table becomes a vertical card listing every column with a PK
/// (`★`) / FK (`⚷`) / regular (` `) badge and the column type
/// right-aligned. Cards sit in columns by FK depth (referenced tables
/// LEFT, dependent tables RIGHT) and edges route from the FK column-
/// row of the source card to the PK column-row of the target card.
///
/// Layout algorithm:
///   1. **Layer assignment** — longest-path-to-sink along outgoing
///      FK edges. Sinks land at layer 0 (leftmost). Cycles are broken
///      by marking on-stack nodes as layer 0.
///   2. **Within-layer ordering** — alphabetical for determinism.
///   3. **Per-card geometry** — inner width = max of column-row width
///      and title width. Layer width is the max of all its cards so
///      every card in a column has aligned borders.
///   4. **Channel routing** — edges going from layer F → layer T
///      (F > T) route through the channel just to the right of the
///      target layer. Each edge in a given channel is given a unique
///      mid-X so parallel edges never sit on top of each other. The
///      route is `source-left-edge → horizontal → bend → vertical →
///      bend → horizontal → target-right-edge ◀`.
///   5. **Selected edge wins** — all non-selected edges paint first;
///      the active edge paints last in `theme.warning` + bold so it
///      sits on top of any crossings.
///
/// Tables without cached `TableInfo` fall back to a name-only stub
/// card; edges to/from those attach to the box mid-Y instead of a
/// specific column row.
fn render_card_erd(
    tables: &[String],
    edges: &[RelationshipEdge],
    selected: usize,
    cache: &HashMap<String, TableInfo>,
    th: Theme,
) -> Vec<Line<'static>> {
    if tables.is_empty() {
        return Vec::new();
    }

    // ── 1. Build outgoing-edge adjacency (table → referenced tables) ──
    let mut out: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in edges {
        out.entry(e.from_table.as_str())
            .or_default()
            .push(e.to_table.as_str());
    }

    // ── 2. Layer assignment via memoised longest-path-to-sink. ────
    fn compute_layer<'a>(
        node: &'a str,
        out: &HashMap<&'a str, Vec<&'a str>>,
        memo: &mut HashMap<&'a str, usize>,
        on_stack: &mut HashSet<&'a str>,
    ) -> usize {
        if let Some(&l) = memo.get(node) {
            return l;
        }
        if on_stack.contains(node) {
            // Cycle: pin to 0 to break it.
            return 0;
        }
        on_stack.insert(node);
        let layer = match out.get(node) {
            None => 0,
            Some(targets) => targets
                .iter()
                .map(|t| compute_layer(t, out, memo, on_stack) + 1)
                .max()
                .unwrap_or(0),
        };
        on_stack.remove(node);
        memo.insert(node, layer);
        layer
    }

    let mut memo: HashMap<&str, usize> = HashMap::new();
    let mut on_stack: HashSet<&str> = HashSet::new();
    let mut node_layer: HashMap<&str, usize> = HashMap::new();
    for t in tables {
        let l = compute_layer(t.as_str(), &out, &mut memo, &mut on_stack);
        node_layer.insert(t.as_str(), l);
    }
    let max_layer = node_layer.values().copied().max().unwrap_or(0);

    // ── 3. Group tables by layer, sort alphabetically. ────────────
    let mut layers: Vec<Vec<&str>> = vec![Vec::new(); max_layer + 1];
    for t in tables {
        let l = node_layer[t.as_str()];
        layers[l].push(t.as_str());
    }
    for v in layers.iter_mut() {
        v.sort();
    }

    // ── 4. Build cards for every table. ───────────────────────────
    let referenced: HashSet<&str> = edges
        .iter()
        .flat_map(|e| [e.from_table.as_str(), e.to_table.as_str()])
        .collect();

    let mut cards: HashMap<&str, ErdCard> = HashMap::new();
    for t in tables {
        let info = cache.get(t.as_str());
        let pk_set: HashSet<&str> = info
            .and_then(|i| i.primary_key.as_ref())
            .map(|pk| pk.column_names.iter().map(String::as_str).collect())
            .unwrap_or_default();
        let fk_set: HashSet<&str> = info
            .map(|i| {
                i.foreign_keys
                    .iter()
                    .flat_map(|fk| fk.column_names.iter().map(String::as_str))
                    .collect()
            })
            .unwrap_or_default();
        let mut rows: Vec<(char, Color, &str, &str)> = Vec::new();
        let mut col_y: HashMap<&str, usize> = HashMap::new();
        if let Some(info) = info {
            for (i, c) in info.columns.iter().enumerate() {
                let (marker, color) = if pk_set.contains(c.name.as_str()) {
                    ('★', th.warning)
                } else if fk_set.contains(c.name.as_str()) {
                    ('⚷', th.accent2)
                } else {
                    (' ', th.fg)
                };
                rows.push((marker, color, c.name.as_str(), c.data_type.as_str()));
                col_y.insert(c.name.as_str(), i);
            }
        }
        // Inner width: room for `M name  type ` plus 2 cells of side
        // padding. Title `─ name ─` inside the top border also has to
        // fit, so floor the width at name + 6.
        let row_w = rows
            .iter()
            .map(|(_, _, n, t)| 2 + n.chars().count() + 2 + t.chars().count())
            .max()
            .unwrap_or(0);
        let title_w = t.chars().count() + 4; // "─ name ─"
        let inner_w = row_w.max(title_w).max(12) + 2;
        cards.insert(
            t.as_str(),
            ErdCard {
                name: t.as_str(),
                rows,
                inner_w,
                col_y,
            },
        );
    }

    // ── 5. Layer geometry: align card widths within each layer and
    //       stack cards vertically with a gap.
    const V_GAP: usize = 1;
    const CHANNEL_W: usize = 10;

    let layer_w: Vec<usize> = layers
        .iter()
        .map(|ts| ts.iter().map(|n| cards[n].width()).max().unwrap_or(14))
        .collect();
    // Pin all cards in a layer to the layer width so their borders align.
    for (l, ts) in layers.iter().enumerate() {
        let target = layer_w[l] - 2;
        for n in ts {
            cards.get_mut(n).unwrap().inner_w = target;
        }
    }

    let mut layer_x: Vec<usize> = Vec::with_capacity(layers.len());
    let mut x_cursor = 1; // 1-cell left margin
    for (i, w) in layer_w.iter().enumerate() {
        layer_x.push(x_cursor);
        x_cursor += w;
        if i + 1 < layer_w.len() {
            x_cursor += CHANNEL_W;
        }
    }
    let canvas_w = x_cursor + 1;

    // node_y0 = top border row of the card.
    let mut node_y0: HashMap<&str, usize> = HashMap::new();
    let mut max_h = 0;
    for ts in &layers {
        let mut y = 0;
        for n in ts {
            node_y0.insert(*n, y);
            y += cards[n].height() + V_GAP;
        }
        if y > max_h {
            max_h = y;
        }
    }
    let canvas_h = max_h.max(3);

    let blank = ErdCell {
        ch: ' ',
        fg: th.fg,
        bold: false,
    };
    let mut buf: Vec<Vec<ErdCell>> = vec![vec![blank; canvas_w]; canvas_h];

    let put = |buf: &mut Vec<Vec<ErdCell>>, x: usize, y: usize, ch: char, fg: Color, bold: bool| {
        if y < buf.len() && x < buf[0].len() {
            buf[y][x] = ErdCell { ch, fg, bold };
        }
    };

    // ── 6. Draw cards. ────────────────────────────────────────────
    for ts in &layers {
        for n in ts {
            let card = &cards[n];
            let x0 = layer_x[node_layer[n]];
            let y0 = node_y0[n];
            let w = card.width();
            let inner = card.inner_w;
            let title_color = if referenced.contains(n) {
                th.accent
            } else {
                th.muted
            };

            // Top border: `╭─ name ────╮`. Render `─` first, then the
            // name overlays positions 2..=2+name_len.
            put(&mut buf, x0, y0, '╭', th.border, false);
            for k in 1..w - 1 {
                put(&mut buf, x0 + k, y0, '─', th.border, false);
            }
            put(&mut buf, x0 + w - 1, y0, '╮', th.border, false);
            // Title label (inset 2 cells, surrounded by spaces for breathing room).
            put(&mut buf, x0 + 1, y0, ' ', th.border, false);
            for (i, ch) in card.name.chars().enumerate() {
                put(&mut buf, x0 + 2 + i, y0, ch, title_color, true);
            }
            put(
                &mut buf,
                x0 + 2 + card.name.chars().count(),
                y0,
                ' ',
                th.border,
                false,
            );

            // Body rows.
            if card.rows.is_empty() {
                // Stub: single muted "(loading…)" row.
                let label = "(loading…)";
                put(&mut buf, x0, y0 + 1, '│', th.border, false);
                for k in 1..w - 1 {
                    put(&mut buf, x0 + k, y0 + 1, ' ', th.fg, false);
                }
                let pad = inner.saturating_sub(label.chars().count()) / 2;
                for (i, ch) in label.chars().enumerate() {
                    put(&mut buf, x0 + 1 + pad + i, y0 + 1, ch, th.muted, false);
                }
                put(&mut buf, x0 + w - 1, y0 + 1, '│', th.border, false);
            } else {
                for (i, (marker, color, name, ty)) in card.rows.iter().enumerate() {
                    let y = y0 + 1 + i;
                    put(&mut buf, x0, y, '│', th.border, false);
                    for k in 1..w - 1 {
                        put(&mut buf, x0 + k, y, ' ', th.fg, false);
                    }
                    put(&mut buf, x0 + w - 1, y, '│', th.border, false);
                    // Marker at col 1 (inner col 0).
                    put(&mut buf, x0 + 1, y, *marker, *color, true);
                    // Column name at col 3.
                    for (j, ch) in name.chars().enumerate() {
                        put(&mut buf, x0 + 3 + j, y, ch, *color, false);
                    }
                    // Type right-aligned with one cell of inner padding.
                    let ty_w = ty.chars().count();
                    let ty_x = x0 + w - 1 - 1 - ty_w; // right border − 1 padding − ty_w
                    for (j, ch) in ty.chars().enumerate() {
                        put(&mut buf, ty_x + j, y, ch, th.muted, false);
                    }
                }
            }

            // Bottom border.
            let yb = y0 + card.height() - 1;
            put(&mut buf, x0, yb, '╰', th.border, false);
            for k in 1..w - 1 {
                put(&mut buf, x0 + k, yb, '─', th.border, false);
            }
            put(&mut buf, x0 + w - 1, yb, '╯', th.border, false);
        }
    }

    // ── 7. Pre-allocate one mid_x per edge inside its channel so
    //       parallel edges don't overlap.
    let mut by_channel: HashMap<usize, Vec<usize>> = HashMap::new();
    for (eid, edge) in edges.iter().enumerate() {
        let Some(&fl) = node_layer.get(edge.from_table.as_str()) else {
            continue;
        };
        let Some(&tl) = node_layer.get(edge.to_table.as_str()) else {
            continue;
        };
        if fl <= tl || edge.from_table == edge.to_table {
            continue;
        }
        by_channel.entry(tl).or_default().push(eid);
    }
    let mut edge_mid_x: HashMap<usize, usize> = HashMap::new();
    for (tl, eids) in &by_channel {
        let channel_start = layer_x[*tl] + layer_w[*tl];
        let channel_end = if tl + 1 < layer_x.len() {
            layer_x[tl + 1]
        } else {
            channel_start + CHANNEL_W
        };
        let span = channel_end.saturating_sub(channel_start).max(1);
        let n = eids.len();
        for (i, &eid) in eids.iter().enumerate() {
            let mx = channel_start + (i + 1) * span / (n + 1);
            edge_mid_x.insert(eid, mx.max(channel_start).min(channel_end - 1));
        }
    }

    // Helper: anchor Y inside a card for a given column. Falls back to
    // box-mid when the column isn't in the cache (table info still
    // loading).
    let anchor_y = |table: &str, col: &str| -> usize {
        let card = &cards[table];
        let y0 = node_y0[table];
        if let Some(&i) = card.col_y.get(col) {
            y0 + 1 + i
        } else {
            y0 + card.height() / 2
        }
    };

    // ── 8. Route edges: non-selected first, selected last.
    let order: Vec<usize> = (0..edges.len())
        .filter(|i| *i != selected)
        .chain(std::iter::once(selected).filter(|_| selected < edges.len()))
        .collect();
    for eid in order {
        let edge = &edges[eid];
        let Some(&fl) = node_layer.get(edge.from_table.as_str()) else {
            continue;
        };
        let Some(&tl) = node_layer.get(edge.to_table.as_str()) else {
            continue;
        };
        if fl <= tl || edge.from_table == edge.to_table {
            continue;
        }
        let Some(&mid_x) = edge_mid_x.get(&eid) else {
            continue;
        };
        let is_sel = eid == selected;
        let color = if is_sel { th.warning } else { th.muted };
        let bold = is_sel;

        let from_col = edge.from_columns.first().map(String::as_str).unwrap_or("");
        let to_col = edge.to_columns.first().map(String::as_str).unwrap_or("");
        let from_y = anchor_y(edge.from_table.as_str(), from_col);
        let to_y = anchor_y(edge.to_table.as_str(), to_col);

        let from_x_left = layer_x[fl];
        let to_x_right = layer_x[tl] + layer_w[tl] - 1;
        let stub_x = from_x_left.saturating_sub(1);
        let target_stub = to_x_right + 1;

        // Segment 1: horizontal at from_y from mid_x → stub_x.
        let (a, b) = (mid_x.min(stub_x), mid_x.max(stub_x));
        for x in a..=b {
            let cell = buf[from_y][x];
            if cell.ch == ' ' || cell.ch == '─' || is_sel {
                put(&mut buf, x, from_y, '─', color, bold);
            }
        }

        // Segment 2: vertical at mid_x from from_y → to_y.
        if from_y != to_y {
            let going_down = to_y > from_y;
            let bend_top = if going_down { '╭' } else { '╰' };
            let bend_bot = if going_down { '╯' } else { '╮' };
            put(&mut buf, mid_x, from_y, bend_top, color, bold);
            let (a, b) = (from_y.min(to_y), from_y.max(to_y));
            for y in (a + 1)..b {
                let cell = buf[y][mid_x];
                if cell.ch == ' ' || cell.ch == '│' || is_sel {
                    put(&mut buf, mid_x, y, '│', color, bold);
                }
            }
            put(&mut buf, mid_x, to_y, bend_bot, color, bold);
        }

        // Segment 3: horizontal at to_y from target_stub → mid_x.
        let (a, b) = (mid_x.min(target_stub), mid_x.max(target_stub));
        for x in a..=b {
            if x == mid_x && from_y != to_y {
                continue;
            }
            let cell = buf[to_y][x];
            if cell.ch == ' ' || cell.ch == '─' || is_sel {
                put(&mut buf, x, to_y, '─', color, bold);
            }
        }

        // Arrowhead pointing into the target card's right border.
        let head_color = if is_sel { th.accent } else { color };
        put(&mut buf, target_stub, to_y, '◀', head_color, true);
    }

    // ── 9. Convert canvas to styled lines. ────────────────────────
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(canvas_h + 2);
    for row in &buf {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut current = String::new();
        let mut current_style = Style::default().fg(th.fg);
        for cell in row {
            let mut st = Style::default().fg(cell.fg);
            if cell.bold {
                st = st.add_modifier(Modifier::BOLD);
            }
            if st != current_style && !current.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut current), current_style));
            }
            current.push(cell.ch);
            current_style = st;
        }
        if !current.is_empty() {
            spans.push(Span::styled(current, current_style));
        }
        lines.push(Line::from(spans));
    }
    lines
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

    // Current-statement byte range, used to highlight the active SQL.
    let stmt_range = statement_range_at(&app.editor, app.editor_cursor);

    let (cursor_line, cursor_col) = line_col(&app.editor, app.editor_cursor);

    let mut byte_offset = 0usize;
    let mut gutter_lines: Vec<Line> = Vec::new();
    let mut text_lines: Vec<Line> = Vec::new();
    for (idx, line) in app.editor.split('\n').enumerate() {
        let line_start = byte_offset;
        let line_end = line_start + line.len();
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
            // Apply the per-line background by patching each span's style.
            let mut st = span.style;
            st.bg = Some(line_bg);
            span.style = st;
        }
        if spans.is_empty() {
            spans.push(Span::styled(" ", Style::default().bg(line_bg)));
        }
        text_lines.push(Line::from(spans));
        byte_offset = line_end + 1; // +1 for the '\n' we split on
    }

    f.render_widget(
        Paragraph::new(gutter_lines).style(Style::default().bg(th.bg)),
        gutter_area,
    );
    f.render_widget(
        Paragraph::new(text_lines)
            .style(Style::default().fg(th.fg).bg(th.bg))
            .wrap(Wrap { trim: false }),
        text_area,
    );

    // Position the terminal's hardware cursor inside the text pane.
    let cx = text_area.x + (cursor_col as u16).min(text_area.width.saturating_sub(1));
    let cy = text_area.y + (cursor_line as u16).min(text_area.height.saturating_sub(1));
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
    execute!(stdout, EnterAlternateScreen)?;
    Terminal::new(ratatui::backend::CrosstermBackend::new(stdout)).map_err(Into::into)
}

fn restore_terminal(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<Stdout>>,
) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        close_current_table, line_col, line_end, line_start, move_cursor_vertical,
        next_char_boundary, prev_char_boundary, short_hash, unique_connection_name, AppState,
        ConnectionConfig, DetailTab, DriverKind, Theme, ALL_TABS,
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
    fn render_card_erd_emits_box_borders_and_pk_marker() {
        use std::collections::HashMap;
        use tsql_db::{ColumnInfo, PrimaryKeyInfo, RelationshipEdge, TableInfo};

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
            foreign_keys: vec![tsql_db::ForeignKeyInfo {
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

        let lines = super::render_card_erd(&tables, &edges, 0, &cache, Theme::catppuccin_mocha());
        let rendered: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();

        assert!(rendered.contains("orders"), "card title for orders");
        assert!(rendered.contains("users"), "card title for users");
        assert!(rendered.contains("user_id"), "FK column name visible");
        assert!(
            rendered.chars().any(|c| c == '╭') && rendered.chars().any(|c| c == '╯'),
            "rounded card borders rendered"
        );
        assert!(rendered.contains('★'), "PK marker rendered");
        assert!(rendered.contains('⚷'), "FK marker rendered");
        assert!(rendered.contains('◀'), "edge arrowhead rendered");
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
}
