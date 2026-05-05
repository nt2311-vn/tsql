use std::collections::HashSet;
use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, List, ListItem, ListState, Paragraph, Row, Table, TableState,
    Tabs, Wrap,
};
use ratatui::{Frame, Terminal};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tsql_core::{ConnectionConfig, DriverKind};
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
    editor: String,
    /// Byte index of the cursor within `editor`. Always sits on a UTF-8
    /// char boundary.
    editor_cursor: usize,
    /// Last 50 successfully-submitted queries, newest last.
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
            editor: String::new(),
            editor_cursor: 0,
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
    "j/k navigate  l/Enter expand  h collapse  Tab pane  e editor  q quit".to_owned()
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
        return handle_command_key(app, key);
    }

    match (key.code, key.modifiers) {
        // q quits from any non-text-entry mode
        (KeyCode::Char('q'), KeyModifiers::NONE)
            if app.mode != AppMode::Editor
                && !(app.mode == AppMode::Connect && app.connect_focus == ConnectFocus::NewUrl) =>
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
fn handle_command_key(app: &mut AppState, key: KeyEvent) -> Result<bool> {
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
            return run_command(app, &cmd);
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
fn run_command(app: &mut AppState, command: &str) -> Result<bool> {
    let cmd = command.trim();
    let (head, _rest) = cmd.split_once(' ').unwrap_or((cmd, ""));
    match head {
        "" => {}
        "q" | "quit" => return Ok(true),
        "select" | "s" => prefill_editor_with_select(app),
        "insert" | "i" => prefill_editor_with_insert(app),
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
        }
        "help" | "h" | "?" => {
            app.status = ":select :insert :describe :indexes :keys \
                          :constraints :erd :help :q"
                .to_owned();
        }
        other => {
            app.last_error = Some(format!("unknown command: :{other}  (try :help)"));
            app.status = format!("unknown command: :{other}");
        }
    }
    Ok(false)
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
    app.url = url;
    app.last_error = None;
    app.status = format!("Connecting to {}…", app.url);
    match open_pool_and_overview(app).await {
        Ok(ov) => {
            rebuild_sidebar(app, &ov);
            app.overview = Some(ov);
            app.mode = AppMode::Browser;
            app.status = nav_hint();
        }
        Err(e) => {
            app.last_error = Some(e.to_string());
            app.status = format!("Connection failed: {e}");
        }
    }
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
            run_editor_query(app).await;
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

async fn run_editor_query(app: &mut AppState) {
    if app.editor.trim().is_empty() {
        return;
    }
    app.last_error = None;
    app.status = "executing…".to_owned();
    let doc = SqlDocument::new(app.editor.clone());
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
            }
            push_history(app);
        }
        Err(e) => {
            app.last_error = Some(e.to_string());
            app.status = format!("error: {e}");
        }
    }
}

fn editor_insert_str(app: &mut AppState, s: &str) {
    app.editor.insert_str(app.editor_cursor, s);
    app.editor_cursor += s.len();
    app.history_idx = None;
}

fn push_history(app: &mut AppState) {
    let trimmed = app.editor.trim().to_owned();
    if trimmed.is_empty() {
        return;
    }
    // De-duplicate adjacent: don't store the same query twice in a row.
    if app.history.last().map(String::as_str) == Some(trimmed.as_str()) {
        return;
    }
    app.history.push(trimmed);
    const MAX_HISTORY: usize = 50;
    if app.history.len() > MAX_HISTORY {
        let drop = app.history.len() - MAX_HISTORY;
        app.history.drain(..drop);
    }
    app.history_idx = None;
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
            app.status = "Ctrl+R run  Esc back".to_owned();
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
    // Preserve cached relationships when jumping inside the same schema
    // (ERD edges are schema-scoped, so they're still valid).
    if schema != app.current_schema {
        app.relationships.clear();
        app.erd_selected = 0;
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
                .constraints([Constraint::Percentage(24), Constraint::Percentage(76)])
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

    let url_display = if app.connect_input.is_empty() && app.connect_focus == ConnectFocus::NewUrl {
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

    let url_title = if has_saved {
        "  New Connection (n)  "
    } else {
        "  Connection URL  "
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
            Line::from(Span::styled(tab.label(), st))
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

    let col_count = rec.columns.len().max(1);
    let pct = (100 / col_count) as u16;
    let widths: Vec<Constraint> = rec
        .columns
        .iter()
        .map(|_| Constraint::Percentage(pct))
        .collect();

    let header: Row = Row::new(rec.columns.iter().enumerate().map(|(ci, name)| {
        let st = if ci == app.record_col {
            Style::default()
                .fg(th.accent)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
        };
        Cell::from(name.as_str()).style(st)
    }))
    .style(Style::default().bg(th.sel_bg))
    .height(1);

    let rows: Vec<Row> = rec
        .rows
        .iter()
        .enumerate()
        .map(|(ri, row)| {
            let cells = row.iter().enumerate().map(|(ci, val)| {
                let st = if ri == app.record_row && ci == app.record_col {
                    Style::default().fg(th.bg).bg(th.accent)
                } else if ri == app.record_row {
                    Style::default().fg(th.sel_fg).bg(th.sel_bg)
                } else if val == "NULL" {
                    Style::default().fg(th.muted)
                } else {
                    Style::default().fg(th.fg)
                };
                Cell::from(val.as_str()).style(st)
            });
            Row::new(cells).height(1)
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

    let header = Row::new(vec![
        Cell::from("Column").style(Style::default().fg(th.accent).add_modifier(Modifier::BOLD)),
        Cell::from("Type").style(Style::default().fg(th.accent).add_modifier(Modifier::BOLD)),
        Cell::from("PK").style(Style::default().fg(th.accent).add_modifier(Modifier::BOLD)),
        Cell::from("FK").style(Style::default().fg(th.accent).add_modifier(Modifier::BOLD)),
        Cell::from("Nullable").style(Style::default().fg(th.accent).add_modifier(Modifier::BOLD)),
        Cell::from("Default").style(Style::default().fg(th.accent).add_modifier(Modifier::BOLD)),
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
            Row::new(vec![
                Cell::from(col.name.as_str()).style(name_st),
                Cell::from(col.data_type.as_str()).style(Style::default().fg(th.accent2)),
                Cell::from(if is_pk { "✓" } else { "" }).style(Style::default().fg(th.warning)),
                Cell::from(if is_fk { "✓" } else { "" }).style(Style::default().fg(th.accent2)),
                Cell::from(if col.is_nullable { "yes" } else { "no" }).style(Style::default().fg(
                    if col.is_nullable {
                        th.muted
                    } else {
                        th.success
                    },
                )),
                Cell::from(col.default_value.as_deref().unwrap_or("—"))
                    .style(Style::default().fg(th.muted)),
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

    let header = Row::new(vec![
        Cell::from("Index").style(Style::default().fg(th.accent).add_modifier(Modifier::BOLD)),
        Cell::from("Columns").style(Style::default().fg(th.accent).add_modifier(Modifier::BOLD)),
        Cell::from("Unique").style(Style::default().fg(th.accent).add_modifier(Modifier::BOLD)),
    ])
    .style(Style::default().bg(th.sel_bg));

    let rows: Vec<Row> = info
        .indexes
        .iter()
        .map(|idx| {
            Row::new(vec![
                Cell::from(idx.name.as_str()).style(Style::default().fg(th.fg)),
                Cell::from(idx.column_names.join(", ")).style(Style::default().fg(th.accent2)),
                Cell::from(if idx.is_unique { "✓" } else { "—" })
                    .style(Style::default().fg(if idx.is_unique { th.success } else { th.muted })),
            ])
        })
        .collect();

    f.render_widget(
        Table::new(
            rows,
            [
                Constraint::Percentage(40),
                Constraint::Percentage(45),
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

    let mut lines: Vec<Line> = vec![Line::default()];

    if let Some(pk) = &info.primary_key {
        lines.push(Line::from(vec![
            Span::styled(
                "  PRIMARY KEY ",
                Style::default()
                    .fg(th.bg)
                    .bg(th.warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                pk.column_names.join(", "),
                Style::default().fg(th.fg).add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::default());
    }

    if info.foreign_keys.is_empty() && info.primary_key.is_none() {
        lines.push(Line::from(Span::styled(
            "  No keys defined.",
            Style::default().fg(th.muted),
        )));
    }

    for fk in &info.foreign_keys {
        lines.push(Line::from(vec![
            Span::styled(
                "  FK ",
                Style::default()
                    .fg(th.bg)
                    .bg(th.accent2)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(&fk.name, Style::default().fg(th.muted)),
        ]));
        lines.push(Line::from(vec![
            Span::raw("       "),
            Span::styled(fk.column_names.join(", "), Style::default().fg(th.fg)),
            Span::styled("  →  ", Style::default().fg(th.muted)),
            Span::styled(
                &fk.referenced_table,
                Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(".", Style::default().fg(th.muted)),
            Span::styled(fk.referenced_columns.join(", "), Style::default().fg(th.fg)),
        ]));
        lines.push(Line::default());
    }

    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(th.bg)),
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

fn draw_erd(f: &mut Frame<'_>, app: &AppState, area: Rect) {
    let th = app.theme;
    let schema = &app.current_schema;

    if app.relationships.is_empty() {
        let hint = if schema.is_empty() {
            "  No schema selected."
        } else {
            "  No foreign-key relationships found in this schema."
        };
        f.render_widget(muted_para(hint, th), area);
        return;
    }

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(
                format!("  ERD  schema: {schema}  "),
                Style::default()
                    .fg(th.bg)
                    .bg(th.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {} relationship(s)  ", app.relationships.len()),
                Style::default().fg(th.muted),
            ),
        ]),
        Line::default(),
    ];

    // Walk relationships in their (already-sorted) order so the flat
    // index `erd_selected` lines up with the rendered rows.
    let selected = app.erd_selected;
    let mut prev_from: Option<&str> = None;
    let mut group_remaining = 0usize;
    for (idx, edge) in app.relationships.iter().enumerate() {
        let from = edge.from_table.as_str();
        if Some(from) != prev_from {
            // Header for a new source-table group.
            lines.push(Line::from(vec![Span::styled(
                format!("  ┌─  {from}"),
                Style::default().fg(th.accent2).add_modifier(Modifier::BOLD),
            )]));
            // Count consecutive edges that share this from_table.
            group_remaining = app.relationships[idx..]
                .iter()
                .take_while(|e| e.from_table == edge.from_table)
                .count();
            prev_from = Some(from);
        }
        let connector = if group_remaining == 1 { "└" } else { "├" };
        group_remaining -= 1;

        let is_selected = idx == selected;
        let row_bg = if is_selected { th.sel_bg } else { th.bg };
        let row_fg = if is_selected { th.sel_fg } else { th.fg };
        let highlight = Style::default().fg(row_fg).bg(row_bg);
        let arrow = if is_selected {
            "  ━━▶  "
        } else {
            "  ──▶  "
        };

        lines.push(Line::from(vec![
            Span::styled(
                format!("  {connector}── "),
                Style::default().fg(th.border).bg(row_bg),
            ),
            Span::styled(edge.from_columns.join(", "), highlight),
            Span::styled(
                arrow,
                Style::default()
                    .fg(if is_selected { th.accent } else { th.muted })
                    .bg(row_bg)
                    .add_modifier(if is_selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::styled(
                edge.to_table.clone(),
                Style::default()
                    .fg(th.accent)
                    .bg(row_bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(".", Style::default().fg(th.muted).bg(row_bg)),
            Span::styled(edge.to_columns.join(", "), highlight),
        ]));

        if group_remaining == 0 {
            lines.push(Line::default());
        }
    }

    // Show unreferenced tables if we have the overview
    if let Some(ov) = &app.overview {
        if let Some(si) = ov.schemas.iter().find(|s| s.name == *schema) {
            let referenced: HashSet<&str> = app
                .relationships
                .iter()
                .flat_map(|e| [e.from_table.as_str(), e.to_table.as_str()])
                .collect();
            let standalone: Vec<&str> = si
                .tables
                .iter()
                .map(String::as_str)
                .filter(|name| !referenced.contains(*name))
                .collect();
            if !standalone.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  Standalone tables (no FK edges):",
                    Style::default().fg(th.muted),
                )));
                for name in standalone {
                    lines.push(Line::from(vec![
                        Span::styled("    ◆  ", Style::default().fg(th.muted)),
                        Span::styled(name, Style::default().fg(th.fg)),
                    ]));
                }
            }
        }
    }

    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(th.bg))
            .wrap(Wrap { trim: false }),
        area,
    );
}

// ─── SQL editor pane ──────────────────────────────────────────────────────────

fn draw_editor(f: &mut Frame<'_>, app: &AppState, area: Rect) {
    let th = app.theme;
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);

    let title = if app.history.is_empty() {
        "  SQL Editor   Ctrl+R run  Esc browser  ".to_owned()
    } else {
        format!(
            "  SQL Editor   Ctrl+R run  Ctrl+P/N history ({})  Esc browser  ",
            app.history.len()
        )
    };

    let editor_area = layout[0];
    f.render_widget(
        Paragraph::new(app.editor.as_str())
            .style(Style::default().fg(th.fg).bg(th.bg))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .title(Span::styled(
                        title,
                        Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                    ))
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(th.active_border))
                    .style(Style::default().bg(th.bg)),
            ),
        editor_area,
    );

    // Position the terminal's hardware cursor inside the editor pane so the
    // user can see exactly where edits will land. Coordinates are computed
    // from the (line, column) of `editor_cursor`. Note this ignores soft-wrap
    // so very long lines may push the cursor off the visible width; that's a
    // known limitation deferred to future work.
    let inner = Rect {
        x: editor_area.x + 1,
        y: editor_area.y + 1,
        width: editor_area.width.saturating_sub(2),
        height: editor_area.height.saturating_sub(2),
    };
    let (line, col) = line_col(&app.editor, app.editor_cursor);
    let cx = inner.x + (col as u16).min(inner.width.saturating_sub(1));
    let cy = inner.y + (line as u16).min(inner.height.saturating_sub(1));
    f.set_cursor_position((cx, cy));

    // Results from editor queries
    let result_text: String = if let Some(rec) = &app.records {
        if rec.columns.is_empty() {
            format!("rows affected: {}", rec.rows_affected)
        } else {
            let mut s = rec.columns.join("  │  ");
            s.push('\n');
            s.push_str(&"─".repeat(60));
            s.push('\n');
            for row in rec.rows.iter().take(200) {
                s.push_str(&row.join("  │  "));
                s.push('\n');
            }
            s
        }
    } else {
        String::new()
    };

    f.render_widget(
        Paragraph::new(result_text.as_str())
            .style(Style::default().fg(th.fg).bg(th.bg))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .title(Span::styled("  Results  ", Style::default().fg(th.muted)))
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(th.border))
                    .style(Style::default().bg(th.bg)),
            ),
        layout[1],
    );
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
        line_col, line_end, line_start, move_cursor_vertical, next_char_boundary,
        prev_char_boundary, Theme, ALL_TABS,
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
}
