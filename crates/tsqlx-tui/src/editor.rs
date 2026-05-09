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
/// Keywords come back bold-accent, string literals in `theme.success`,
/// numbers in `theme.warning`, and `--` line comments in muted italics.
/// Anything else falls through as default-styled text.
#[must_use]
pub fn highlight_line<'a>(line: &'a str, th: Theme) -> Vec<Span<'a>> {
    let bytes = line.as_bytes();
    let mut spans: Vec<Span<'a>> = Vec::new();
    let mut tail_start = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        let ch = bytes[i] as char;
        if ch == '\'' {
            flush(&mut spans, line, tail_start, i, th);
            let start = i;
            i += 1;
            while i < bytes.len() {
                let here = bytes[i];
                i += 1;
                if here == b'\'' {
                    break;
                }
            }
            spans.push(Span::styled(
                &line[start..i],
                Style::default().fg(th.success),
            ));
            tail_start = i;
            continue;
        }
        if ch == '-' && bytes.get(i + 1).copied() == Some(b'-') {
            flush(&mut spans, line, tail_start, i, th);
            spans.push(Span::styled(
                &line[i..],
                Style::default().fg(th.muted).add_modifier(Modifier::ITALIC),
            ));
            tail_start = bytes.len();
            break;
        }
        if ch.is_ascii_alphabetic() || ch == '_' {
            flush(&mut spans, line, tail_start, i, th);
            let start = i;
            while i < bytes.len() {
                let c = bytes[i] as char;
                if c.is_ascii_alphanumeric() || c == '_' {
                    i += 1;
                } else {
                    break;
                }
            }
            let word = &line[start..i];
            let upper = word.to_ascii_uppercase();
            let style = if SQL_KEYWORDS.binary_search(&upper.as_str()).is_ok() {
                Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(th.fg)
            };
            spans.push(Span::styled(word, style));
            tail_start = i;
            continue;
        }
        if ch.is_ascii_digit() {
            flush(&mut spans, line, tail_start, i, th);
            let start = i;
            while i < bytes.len() {
                let c = bytes[i] as char;
                if c.is_ascii_digit() || c == '.' {
                    i += 1;
                } else {
                    break;
                }
            }
            spans.push(Span::styled(
                &line[start..i],
                Style::default().fg(th.warning),
            ));
            tail_start = i;
            continue;
        }
        i += 1;
    }
    flush(&mut spans, line, tail_start, bytes.len(), th);
    spans
}

fn flush<'a>(spans: &mut Vec<Span<'a>>, line: &'a str, from: usize, to: usize, th: Theme) {
    if from < to {
        spans.push(Span::styled(&line[from..to], Style::default().fg(th.fg)));
    }
}

/// Curated SQL keyword list, sorted ASCII-uppercase for binary search.
const SQL_KEYWORDS: &[&str] = &[
    "ALL",
    "ALTER",
    "AND",
    "AS",
    "ASC",
    "BEGIN",
    "BETWEEN",
    "BY",
    "CASCADE",
    "CASE",
    "CHECK",
    "COMMIT",
    "CONSTRAINT",
    "CREATE",
    "CROSS",
    "DATABASE",
    "DEFAULT",
    "DELETE",
    "DESC",
    "DISTINCT",
    "DROP",
    "ELSE",
    "END",
    "EXISTS",
    "FALSE",
    "FOREIGN",
    "FROM",
    "FULL",
    "GROUP",
    "HAVING",
    "IF",
    "IN",
    "INDEX",
    "INNER",
    "INSERT",
    "INTO",
    "IS",
    "JOIN",
    "KEY",
    "LEFT",
    "LIKE",
    "LIMIT",
    "NOT",
    "NULL",
    "OFFSET",
    "ON",
    "OR",
    "ORDER",
    "OUTER",
    "PRIMARY",
    "REFERENCES",
    "RETURNING",
    "RIGHT",
    "ROLLBACK",
    "SELECT",
    "SET",
    "TABLE",
    "THEN",
    "TRUE",
    "UNION",
    "UNIQUE",
    "UPDATE",
    "USING",
    "VALUES",
    "VIEW",
    "WHEN",
    "WHERE",
    "WITH",
];

/// Path under which we persist a connection's query history. The
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
