//! SQL editor support: statement-range detection, syntax highlighting,
//! and per-connection history persistence. The editor's UI state still
//! lives on `AppState`; this module just provides the helpers.

use std::path::{Path, PathBuf};

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::Theme;

/// Byte range of the statement that contains `cursor`. Statements are
/// separated by `;`. Semicolons inside `'…'` strings, `--` line comments,
/// `/* … */` block comments, and Postgres `$tag$ … $tag$` dollar-quoted
/// bodies do not split.
///
/// The returned range starts at the byte after the previous statement
/// terminator (or 0) and ends at the byte just before the trailing
/// semicolon (or at the end of the buffer if there is none). Leading
/// and trailing whitespace is stripped.
#[must_use]
pub fn statement_range_at(text: &str, cursor: usize) -> std::ops::Range<usize> {
    let bytes = text.as_bytes();
    let mut state = ScanState::Normal;
    let mut current_start = 0usize;
    let mut found: Option<std::ops::Range<usize>> = None;
    let mut i = 0usize;
    let mut dollar_tag: Vec<u8> = Vec::new();
    while i < bytes.len() {
        let ch = bytes[i] as char;
        match state {
            ScanState::Normal => match ch {
                '\'' => {
                    state = ScanState::SingleString;
                    i += 1;
                }
                '-' if bytes.get(i + 1).copied() == Some(b'-') => {
                    state = ScanState::LineComment;
                    i += 2;
                }
                '/' if bytes.get(i + 1).copied() == Some(b'*') => {
                    state = ScanState::BlockComment;
                    i += 2;
                }
                '$' => {
                    if let Some(tag_len) = dollar_tag_len(&bytes[i..]) {
                        dollar_tag = bytes[i..i + tag_len].to_vec();
                        state = ScanState::DollarQuote;
                        i += tag_len;
                    } else {
                        i += 1;
                    }
                }
                ';' => {
                    if found.is_none() && cursor >= current_start && cursor <= i {
                        found = Some(current_start..i);
                    }
                    i += 1;
                    current_start = i;
                }
                _ => i += 1,
            },
            ScanState::SingleString => {
                if ch == '\'' {
                    state = ScanState::Normal;
                }
                i += 1;
            }
            ScanState::LineComment => {
                if ch == '\n' {
                    state = ScanState::Normal;
                }
                i += 1;
            }
            ScanState::BlockComment => {
                if ch == '*' && bytes.get(i + 1).copied() == Some(b'/') {
                    state = ScanState::Normal;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            ScanState::DollarQuote => {
                if ch == '$'
                    && bytes.len() - i >= dollar_tag.len()
                    && bytes[i..i + dollar_tag.len()] == dollar_tag[..]
                {
                    state = ScanState::Normal;
                    i += dollar_tag.len();
                    dollar_tag.clear();
                } else {
                    i += 1;
                }
            }
        }
    }
    let range = found.unwrap_or(current_start..text.len());
    trim_range(text, range)
}

fn dollar_tag_len(bytes: &[u8]) -> Option<usize> {
    if bytes.is_empty() || bytes[0] != b'$' {
        return None;
    }
    if bytes.get(1).copied() == Some(b'$') {
        return Some(2);
    }
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'$' => return Some(i + 1),
            c if c.is_ascii_alphanumeric() || c == b'_' => i += 1,
            _ => return None,
        }
    }
    None
}

fn trim_range(text: &str, range: std::ops::Range<usize>) -> std::ops::Range<usize> {
    if range.start >= range.end {
        return range;
    }
    let slice = &text[range.clone()];
    let l = slice.len() - slice.trim_start().len();
    let r = slice.len() - slice.trim_end().len();
    let new_start = range.start + l;
    let new_end = range.end - r;
    if new_start > new_end {
        new_start..new_start
    } else {
        new_start..new_end
    }
}

enum ScanState {
    Normal,
    SingleString,
    LineComment,
    BlockComment,
    DollarQuote,
}

/// Tokenize a single line of SQL into styled `Span`s for display.
/// Delegates to `tsqlx_sql::classify_line` which uses the `sqlparser`
/// tokenizer — so dollar-quoted strings, block comments, escaped
/// quotes, hex/national string literals, and dialect type keywords
/// all classify correctly instead of leaking out as default text.
///
/// Token classes map to existing theme colours so every theme in the
/// registry just works without per-class palette extensions.
#[must_use]
pub fn highlight_line<'a>(line: &'a str, th: Theme) -> Vec<Span<'a>> {
    use tsqlx_sql::SpanClass;

    let spans_in = tsqlx_sql::classify_line(line);
    if spans_in.is_empty() {
        return if line.is_empty() {
            Vec::new()
        } else {
            vec![Span::styled(line, Style::default().fg(th.fg))]
        };
    }

    let mut out: Vec<Span<'a>> = Vec::with_capacity(spans_in.len() * 2);
    let mut cursor = 0usize;
    for s in &spans_in {
        if s.start > cursor && line.is_char_boundary(cursor) && line.is_char_boundary(s.start) {
            out.push(Span::styled(
                &line[cursor..s.start],
                Style::default().fg(th.fg),
            ));
        }
        if !(line.is_char_boundary(s.start) && line.is_char_boundary(s.end)) {
            // Bail out of styling if the classifier returned a non-
            // boundary range (shouldn't happen on ASCII SQL, but the
            // sqlparser location → byte conversion is best-effort for
            // multi-byte input). Fall back to a single plain span.
            return vec![Span::styled(line, Style::default().fg(th.fg))];
        }
        let slice = &line[s.start..s.end];
        let style = match s.class {
            SpanClass::Keyword => Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
            SpanClass::Type => Style::default().fg(th.accent2),
            SpanClass::Identifier => Style::default().fg(th.fg),
            SpanClass::StringLit => Style::default().fg(th.success),
            SpanClass::NumberLit => Style::default().fg(th.warning),
            SpanClass::Comment => Style::default().fg(th.muted).add_modifier(Modifier::ITALIC),
            SpanClass::Operator => Style::default().fg(th.accent2),
            SpanClass::Punct => Style::default().fg(th.muted),
            SpanClass::Plain => Style::default().fg(th.fg),
        };
        out.push(Span::styled(slice, style));
        cursor = s.end;
    }
    if cursor < line.len() && line.is_char_boundary(cursor) {
        out.push(Span::styled(&line[cursor..], Style::default().fg(th.fg)));
    }
    out
}

/// `connection_label` is sanitised — anything outside `[A-Za-z0-9_-]`
/// is replaced with `_`.
#[must_use]
pub fn history_path(connection_label: &str) -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".local")
                .join("share")
        });
    let safe: String = connection_label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = if safe.is_empty() {
        "_default".to_owned()
    } else {
        safe
    };
    base.join("tsqlx")
        .join("history")
        .join(format!("{safe}.txt"))
}

/// Load up to the last `cap` deduplicated history entries.
pub async fn load_history(path: &Path, cap: usize) -> Vec<String> {
    let Ok(text) = fs::read_to_string(path).await else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for raw in text.split('\u{1f}') {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if out.last().map(String::as_str) == Some(trimmed) {
            continue;
        }
        out.push(trimmed.to_owned());
    }
    if out.len() > cap {
        let drop = out.len() - cap;
        out.drain(..drop);
    }
    out
}

/// Append a single history entry to the on-disk file. Entries are
/// separated by an ASCII Unit-Separator (`0x1F`) so multi-line queries
/// survive the round-trip without ambiguity.
pub async fn append_history(path: &Path, entry: &str) -> std::io::Result<()> {
    if entry.trim().is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent).await?;
        }
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    let mut buf = String::with_capacity(entry.len() + 1);
    buf.push_str(entry.trim());
    buf.push('\u{1f}');
    file.write_all(buf.as_bytes()).await?;
    file.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Theme;

    #[test]
    fn statement_range_finds_middle_statement() {
        let s = "SELECT 1;\nSELECT 2;\nSELECT 3;";
        let cursor = s.find('2').unwrap();
        let range = statement_range_at(s, cursor);
        assert_eq!(&s[range], "SELECT 2");
    }

    #[test]
    fn statement_range_handles_no_terminator() {
        let s = "SELECT 1;\nSELECT 2";
        let range = statement_range_at(s, s.len());
        assert_eq!(&s[range], "SELECT 2");
    }

    #[test]
    fn statement_range_ignores_semicolons_in_strings() {
        let s = "SELECT 'a;b'; SELECT 2;";
        let range = statement_range_at(s, 4);
        assert_eq!(&s[range], "SELECT 'a;b'");
    }

    #[test]
    fn statement_range_ignores_semicolons_in_comments() {
        let s = "SELECT 1; -- ;\nSELECT 2;";
        let range = statement_range_at(s, 18);
        assert_eq!(&s[range], "-- ;\nSELECT 2");
    }

    #[test]
    fn statement_range_handles_dollar_quoted_body() {
        let s = "DO $$ BEGIN SELECT 1; END $$; SELECT 2;";
        let cursor = s.find("BEGIN").unwrap();
        let range = statement_range_at(s, cursor);
        assert_eq!(&s[range], "DO $$ BEGIN SELECT 1; END $$");
    }

    #[test]
    fn highlight_marks_keywords_and_numbers_and_strings() {
        let th = Theme::catppuccin_mocha();
        let spans = highlight_line("SELECT 42 FROM t WHERE x = 'hi'", th);
        let labels: Vec<&str> = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(labels.contains(&"SELECT"));
        assert!(labels.contains(&"FROM"));
        assert!(labels.contains(&"WHERE"));
        assert!(labels.contains(&"42"));
        assert!(labels.contains(&"'hi'"));
    }

    #[test]
    fn highlight_marks_line_comment() {
        let th = Theme::catppuccin_mocha();
        let spans = highlight_line("SELECT 1 -- trailing", th);
        let last = spans.last().expect("at least one span");
        assert_eq!(last.content.as_ref(), "-- trailing");
    }

    #[test]
    fn history_path_sanitises_label() {
        std::env::set_var("XDG_DATA_HOME", "/tmp/tsqlx_hist_test");
        let path = history_path("prod conn?");
        assert!(path.ends_with("tsqlx/history/prod_conn_.txt"));
        std::env::remove_var("XDG_DATA_HOME");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn history_round_trip_dedupes_and_caps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hist.txt");
        append_history(&path, "SELECT 1").await.unwrap();
        append_history(&path, "SELECT 1").await.unwrap();
        append_history(&path, "SELECT 2").await.unwrap();
        let loaded = load_history(&path, 10).await;
        assert_eq!(loaded, vec!["SELECT 1".to_owned(), "SELECT 2".to_owned()]);
    }
}
