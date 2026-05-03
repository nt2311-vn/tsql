use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use tsql_core::DriverKind;
use tsql_db::{execute_script, QueryOutput};
use tsql_sql::SqlDocument;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub name: &'static str,
    pub background: Color,
    pub foreground: Color,
    pub accent: Color,
    pub error: Color,
}

impl Theme {
    #[must_use]
    pub const fn catppuccin_mocha() -> Self {
        Self {
            name: "catppuccin-mocha",
            background: Color::Rgb(30, 30, 46),
            foreground: Color::Rgb(205, 214, 244),
            accent: Color::Rgb(137, 180, 250),
            error: Color::Rgb(243, 139, 168),
        }
    }
}

pub async fn run(driver: DriverKind, url: String) -> Result<()> {
    let mut terminal = setup_terminal()?;
    let mut app = AppState::new(driver, url);
    let result = run_loop(&mut terminal, &mut app).await;

    restore_terminal(&mut terminal)?;
    result
}

#[derive(Debug)]
struct AppState {
    driver: DriverKind,
    url: String,
    editor: String,
    output: String,
    status: String,
    theme: Theme,
}

impl AppState {
    fn new(driver: DriverKind, url: String) -> Self {
        Self {
            driver,
            url,
            editor: String::new(),
            output: String::new(),
            status: "Type SQL, paste scripts, Ctrl+R to run, Esc to quit.".to_owned(),
            theme: Theme::catppuccin_mocha(),
        }
    }
}

async fn run_loop(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<Stdout>>,
    app: &mut AppState,
) -> Result<()> {
    loop {
        terminal.draw(|frame| draw(frame, app))?;

        if !event::poll(Duration::from_millis(100))? {
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

async fn handle_key(app: &mut AppState, key: KeyEvent) -> Result<bool> {
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => Ok(true),
        (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
            app.status = "running query...".to_owned();
            let document = SqlDocument::new(app.editor.clone());

            match execute_script(app.driver, &app.url, &document).await {
                Ok(output) => {
                    app.output = format_output(&output);
                    app.status = "query complete".to_owned();
                }
                Err(error) => {
                    app.output.clear();
                    app.status = format!("error: {error}");
                }
            }

            Ok(false)
        }
        (KeyCode::Backspace, _) => {
            app.editor.pop();
            Ok(false)
        }
        (KeyCode::Enter, _) => {
            app.editor.push('\n');
            Ok(false)
        }
        (KeyCode::Tab, _) => {
            app.editor.push_str("    ");
            Ok(false)
        }
        (KeyCode::Char(ch), _) => {
            app.editor.push(ch);
            Ok(false)
        }
        _ => Ok(false),
    }
}

fn draw(frame: &mut Frame<'_>, app: &AppState) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Percentage(45),
            Constraint::Percentage(45),
            Constraint::Length(3),
        ])
        .split(frame.area());
    let base = Style::default()
        .fg(app.theme.foreground)
        .bg(app.theme.background);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "TSQL",
                Style::default()
                    .fg(app.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  {:?}  {}", app.driver, app.theme.name)),
        ]))
        .style(base)
        .block(Block::default().borders(Borders::ALL).title("Connection")),
        layout[0],
    );
    frame.render_widget(
        Paragraph::new(app.editor.as_str())
            .style(base)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title("Query Editor")),
        layout[1],
    );
    frame.render_widget(
        Paragraph::new(app.output.as_str())
            .style(base)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title("Results")),
        layout[2],
    );
    frame.render_widget(
        Paragraph::new(app.status.as_str()).style(if app.status.starts_with("error:") {
            base.fg(app.theme.error)
        } else {
            base
        }),
        layout[3],
    );
}

fn format_output(output: &QueryOutput) -> String {
    let mut formatted = String::new();

    for (index, statement) in output.statements.iter().enumerate() {
        formatted.push_str(&format!("-- statement {}\n", index + 1));

        if statement.columns.is_empty() {
            formatted.push_str(&format!("rows affected: {}\n", statement.rows_affected));
            continue;
        }

        formatted.push_str(&statement.columns.join("\t"));
        formatted.push('\n');

        for row in &statement.rows {
            formatted.push_str(&row.join("\t"));
            formatted.push('\n');
        }
    }

    formatted
}

fn setup_terminal() -> Result<Terminal<ratatui::backend::CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    Terminal::new(backend).map_err(Into::into)
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
    use super::Theme;

    #[test]
    fn default_dark_theme_is_catppuccin_mocha() {
        let theme = Theme::catppuccin_mocha();

        assert_eq!(theme.name, "catppuccin-mocha");
    }
}
