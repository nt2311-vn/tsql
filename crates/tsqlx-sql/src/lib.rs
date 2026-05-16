pub mod complete;
pub mod format;
pub mod highlight;

pub use complete::{
    context_at, prefix_at, rank_candidates, top_level_keywords, Candidate, CandidateKind,
    CompletionContext,
};
pub use format::{format_sql, FormatOptions};
pub use highlight::{classify_line, ClassifiedSpan, SpanClass};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlDocument {
    text: String,
}

impl SqlDocument {
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn statements(&self) -> Vec<String> {
        split_statements(&self.text)
    }

    /// T-SQL batches. SQL Server uses `GO` on its own line as a
    /// client-side batch separator (sqlcmd/SSMS convention; the wire
    /// protocol never sees it). Each returned string is one batch and
    /// may itself contain multiple `;`-separated statements.
    #[must_use]
    pub fn tsql_batches(&self) -> Vec<String> {
        split_tsql_batches(&self.text)
    }

    /// PL/SQL batches. Oracle / SQL*Plus uses `/` on its own line as a
    /// batch terminator — required for anonymous PL/SQL blocks (which
    /// can't be split on internal `;`) and for `CREATE OR REPLACE
    /// PROCEDURE` bodies. The `/` line is consumed; each returned
    /// string is one batch.
    #[must_use]
    pub fn plsql_batches(&self) -> Vec<String> {
        split_plsql_batches(&self.text)
    }
}

/// Split a T-SQL script on `GO` batch separators.
///
/// `GO` is a sqlcmd / SSMS client convention — never sent over TDS — so
/// MSSQL drivers have to peel batches off before they reach the server.
/// A separator is a line whose only non-whitespace content is `GO`,
/// optionally followed by an integer repeat count (`GO 5`). The
/// directive is case-insensitive.
///
/// We deliberately *don't* honor `GO` inside string literals, line
/// comments, or block comments — same boundary rules as
/// [`split_statements`].
#[must_use]
pub fn split_tsql_batches(input: &str) -> Vec<String> {
    let mut batches = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut state = SplitState::Normal;
    let mut at_line_start = true;

    while let Some(ch) = chars.next() {
        match state {
            SplitState::Normal => match ch {
                '\'' => {
                    current.push(ch);
                    at_line_start = false;
                    state = SplitState::SingleQuoted;
                }
                '"' => {
                    current.push(ch);
                    at_line_start = false;
                    state = SplitState::DoubleQuoted;
                }
                '-' if chars.peek() == Some(&'-') => {
                    current.push(ch);
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                    state = SplitState::LineComment;
                }
                '/' if chars.peek() == Some(&'*') => {
                    current.push(ch);
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                    state = SplitState::BlockComment;
                }
                '\n' => {
                    current.push(ch);
                    at_line_start = true;
                }
                ' ' | '\t' | '\r' => {
                    current.push(ch);
                }
                _ => {
                    if at_line_start && is_go_separator(ch, &mut chars) {
                        push_statement(&mut batches, &mut current);
                        // Consume the rest of the GO line (including any
                        // trailing repeat count and the line terminator).
                        consume_to_eol(&mut chars);
                        at_line_start = true;
                    } else {
                        current.push(ch);
                        at_line_start = false;
                    }
                }
            },
            SplitState::SingleQuoted => {
                current.push(ch);
                if ch == '\'' {
                    if chars.peek() == Some(&'\'') {
                        if let Some(next) = chars.next() {
                            current.push(next);
                        }
                    } else {
                        state = SplitState::Normal;
                    }
                }
            }
            SplitState::DoubleQuoted => {
                current.push(ch);
                if ch == '"' {
                    if chars.peek() == Some(&'"') {
                        if let Some(next) = chars.next() {
                            current.push(next);
                        }
                    } else {
                        state = SplitState::Normal;
                    }
                }
            }
            SplitState::LineComment => {
                current.push(ch);
                if ch == '\n' {
                    state = SplitState::Normal;
                    at_line_start = true;
                }
            }
            SplitState::BlockComment => {
                current.push(ch);
                if ch == '*' && chars.peek() == Some(&'/') {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                    state = SplitState::Normal;
                }
            }
            SplitState::DollarQuoted(_) => {
                // T-SQL doesn't use dollar-quoted bodies; if we're here
                // we copy the char and move on. (Reachable only if the
                // caller mixes dialects in the same buffer.)
                current.push(ch);
            }
        }
    }

    push_statement(&mut batches, &mut current);
    batches
}

/// Returns true iff the next two characters spell `GO` (case-insensitive)
/// and the character after `GO` is whitespace, end-of-input, or a digit
/// (for the optional repeat count). `ch` is the *first* char already
/// consumed; on a true return the matching `O` is also consumed.
fn is_go_separator(ch: char, chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> bool {
    if !matches!(ch, 'g' | 'G') {
        return false;
    }
    let Some(&next) = chars.peek() else {
        return true;
    };
    if !matches!(next, 'o' | 'O') {
        return false;
    }
    // Look one further: must be EOL, EOF, whitespace, or a digit (count).
    let mut probe = chars.clone();
    probe.next();
    let after = probe.peek().copied();
    let ok = matches!(
        after,
        None | Some('\n') | Some('\r') | Some(' ') | Some('\t') | Some('0'..='9')
    );
    if ok {
        chars.next();
    }
    ok
}

fn consume_to_eol(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    for ch in chars.by_ref() {
        if ch == '\n' {
            break;
        }
    }
}

/// Split a PL/SQL script on `/` batch terminators (SQL*Plus convention).
///
/// A separator is a line whose only non-whitespace content is `/`. The
/// boundary is honored only at line-start so division operators inside
/// expressions (`SELECT a/b FROM t`) still parse as part of the current
/// batch. Strings, line comments, and block comments are skipped over
/// like in [`split_statements`].
///
/// PL/SQL blocks (`BEGIN … END;`) routinely contain semicolons that
/// don't terminate a statement, so we never split on `;` here — the
/// caller is expected to send each batch verbatim. For non-PL/SQL
/// scripts that happen to use `;`-only terminators this is still
/// correct: the whole script becomes one batch and Oracle parses each
/// statement individually.
#[must_use]
pub fn split_plsql_batches(input: &str) -> Vec<String> {
    let mut batches = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut state = SplitState::Normal;
    let mut at_line_start = true;

    while let Some(ch) = chars.next() {
        match state {
            SplitState::Normal => match ch {
                '\'' => {
                    current.push(ch);
                    at_line_start = false;
                    state = SplitState::SingleQuoted;
                }
                '"' => {
                    current.push(ch);
                    at_line_start = false;
                    state = SplitState::DoubleQuoted;
                }
                '-' if chars.peek() == Some(&'-') => {
                    current.push(ch);
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                    state = SplitState::LineComment;
                }
                '/' if chars.peek() == Some(&'*') => {
                    current.push(ch);
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                    state = SplitState::BlockComment;
                }
                '/' if at_line_start && line_is_only_terminator(&mut chars) => {
                    push_statement(&mut batches, &mut current);
                    consume_to_eol(&mut chars);
                    at_line_start = true;
                }
                '\n' => {
                    current.push(ch);
                    at_line_start = true;
                }
                ' ' | '\t' | '\r' => {
                    current.push(ch);
                }
                _ => {
                    current.push(ch);
                    at_line_start = false;
                }
            },
            SplitState::SingleQuoted => {
                current.push(ch);
                if ch == '\'' {
                    if chars.peek() == Some(&'\'') {
                        if let Some(next) = chars.next() {
                            current.push(next);
                        }
                    } else {
                        state = SplitState::Normal;
                    }
                }
            }
            SplitState::DoubleQuoted => {
                current.push(ch);
                if ch == '"' {
                    if chars.peek() == Some(&'"') {
                        if let Some(next) = chars.next() {
                            current.push(next);
                        }
                    } else {
                        state = SplitState::Normal;
                    }
                }
            }
            SplitState::LineComment => {
                current.push(ch);
                if ch == '\n' {
                    state = SplitState::Normal;
                    at_line_start = true;
                }
            }
            SplitState::BlockComment => {
                current.push(ch);
                if ch == '*' && chars.peek() == Some(&'/') {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                    state = SplitState::Normal;
                }
            }
            SplitState::DollarQuoted(_) => {
                current.push(ch);
            }
        }
    }

    push_statement(&mut batches, &mut current);
    batches
}

/// Returns true iff the rest of the current line (already past the
/// leading `/`) is whitespace + EOL/EOF.
fn line_is_only_terminator(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> bool {
    let mut probe = chars.clone();
    loop {
        match probe.next() {
            None => return true,
            Some('\n') => return true,
            Some(' ') | Some('\t') | Some('\r') => continue,
            Some(_) => return false,
        }
    }
}

#[must_use]
pub fn split_statements(input: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut state = SplitState::Normal;

    while let Some(ch) = chars.next() {
        match state {
            SplitState::Normal => match ch {
                '\'' => {
                    current.push(ch);
                    state = SplitState::SingleQuoted;
                }
                '"' => {
                    current.push(ch);
                    state = SplitState::DoubleQuoted;
                }
                '-' if chars.peek() == Some(&'-') => {
                    current.push(ch);
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                    state = SplitState::LineComment;
                }
                '/' if chars.peek() == Some(&'*') => {
                    current.push(ch);
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                    state = SplitState::BlockComment;
                }
                '$' => {
                    current.push(ch);
                    if let Some(tag) = read_dollar_quote_tag(&mut chars) {
                        current.push_str(&tag);
                        state = SplitState::DollarQuoted(tag);
                    }
                }
                ';' => {
                    push_statement(&mut statements, &mut current);
                }
                _ => current.push(ch),
            },
            SplitState::SingleQuoted => {
                current.push(ch);
                if ch == '\'' {
                    if chars.peek() == Some(&'\'') {
                        if let Some(next) = chars.next() {
                            current.push(next);
                        }
                    } else {
                        state = SplitState::Normal;
                    }
                }
            }
            SplitState::DoubleQuoted => {
                current.push(ch);
                if ch == '"' {
                    if chars.peek() == Some(&'"') {
                        if let Some(next) = chars.next() {
                            current.push(next);
                        }
                    } else {
                        state = SplitState::Normal;
                    }
                }
            }
            SplitState::LineComment => {
                current.push(ch);
                if ch == '\n' {
                    state = SplitState::Normal;
                }
            }
            SplitState::BlockComment => {
                current.push(ch);
                if ch == '*' && chars.peek() == Some(&'/') {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                    state = SplitState::Normal;
                }
            }
            SplitState::DollarQuoted(ref tag) => {
                current.push(ch);
                if ch == '$' && current.ends_with(tag) {
                    state = SplitState::Normal;
                }
            }
        }
    }

    push_statement(&mut statements, &mut current);
    statements
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SplitState {
    Normal,
    SingleQuoted,
    DoubleQuoted,
    LineComment,
    BlockComment,
    DollarQuoted(String),
}

fn read_dollar_quote_tag(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
    let mut tag = String::new();

    while let Some(&ch) = chars.peek() {
        tag.push(ch);
        chars.next();

        if ch == '$' {
            return Some(tag);
        }

        if !(ch == '_' || ch.is_ascii_alphanumeric()) {
            break;
        }
    }

    None
}

fn push_statement(statements: &mut Vec<String>, current: &mut String) {
    let trimmed = current.trim();

    if !trimmed.is_empty() {
        statements.push(trimmed.to_owned());
    }

    current.clear();
}

#[cfg(test)]
mod tests {
    use super::{split_plsql_batches, split_statements, split_tsql_batches, SqlDocument};

    #[test]
    fn document_preserves_multiline_sql() {
        let sql = "select 1;\nselect 2;";
        let document = SqlDocument::new(sql);

        assert_eq!(document.as_str(), sql);
    }

    #[test]
    fn splits_multiline_statements() {
        let statements = split_statements("select 1;\nselect 2;");

        assert_eq!(statements, ["select 1", "select 2"]);
    }

    #[test]
    fn ignores_semicolon_inside_single_quoted_string() {
        let statements = split_statements("select 'a;b'; select 2;");

        assert_eq!(statements, ["select 'a;b'", "select 2"]);
    }

    #[test]
    fn ignores_semicolon_inside_comments() {
        let statements = split_statements("select 1; -- ;\nselect /* ; */ 2;");

        assert_eq!(statements, ["select 1", "-- ;\nselect /* ; */ 2"]);
    }

    #[test]
    fn keeps_postgres_dollar_quoted_body_together() {
        let statements = split_statements(
            r"
            create function test_fn() returns void as $$
            begin
              perform 1;
            end;
            $$ language plpgsql;
            select 1;
            ",
        );

        assert_eq!(statements.len(), 2);
        assert!(statements[0].contains("perform 1;"));
        assert_eq!(statements[1], "select 1");
    }

    #[test]
    fn tsql_batches_split_on_go() {
        let batches = split_tsql_batches("create table t (id int);\nGO\nselect * from t;\nGO");
        assert_eq!(batches.len(), 2);
        assert!(batches[0].contains("create table t"));
        assert_eq!(batches[1], "select * from t;");
    }

    #[test]
    fn tsql_batches_case_insensitive_and_repeat_count() {
        let batches = split_tsql_batches("select 1;\ngo 5\nselect 2;\nGo\nselect 3;");
        assert_eq!(batches, ["select 1;", "select 2;", "select 3;"]);
    }

    #[test]
    fn tsql_batches_ignore_go_inside_string_or_comment() {
        let batches = split_tsql_batches("select 'GO';\n-- GO\n/* GO */\nselect 2;");
        assert_eq!(batches.len(), 1);
        assert!(batches[0].contains("select 'GO';"));
        assert!(batches[0].contains("select 2;"));
    }

    #[test]
    fn tsql_batches_keep_go_when_not_at_line_start() {
        // `GO` mid-statement (e.g. inside an identifier or after other
        // tokens on the same line) is just an identifier — never a
        // separator.
        let batches = split_tsql_batches("select 1; GO\nselect 2;");
        assert_eq!(batches.len(), 1);
    }

    #[test]
    fn document_tsql_batches_round_trip() {
        let doc = SqlDocument::new("a;\nGO\nb;");
        assert_eq!(doc.tsql_batches(), ["a;", "b;"]);
    }

    #[test]
    fn plsql_batches_split_on_slash() {
        let batches = split_plsql_batches("BEGIN NULL; END;\n/\nSELECT * FROM dual;\n/");
        assert_eq!(batches.len(), 2);
        assert!(batches[0].contains("BEGIN NULL; END;"));
        assert_eq!(batches[1], "SELECT * FROM dual;");
    }

    #[test]
    fn plsql_batches_keep_division_inside_expression() {
        let batches = split_plsql_batches("SELECT a/b FROM t;");
        assert_eq!(batches.len(), 1);
    }

    #[test]
    fn plsql_batches_keep_block_comment_open_slash() {
        let batches = split_plsql_batches(
            "/* leading comment */\nSELECT 1 FROM dual;\n/\nSELECT 2 FROM dual;",
        );
        assert_eq!(batches.len(), 2);
    }

    #[test]
    fn plsql_batches_no_terminator_returns_one_batch() {
        let batches = split_plsql_batches("SELECT 1 FROM dual; SELECT 2 FROM dual;");
        assert_eq!(batches.len(), 1);
    }

    #[test]
    fn document_plsql_batches_round_trip() {
        let doc = SqlDocument::new("SELECT 1 FROM dual;\n/\nSELECT 2 FROM dual;\n/");
        assert_eq!(
            doc.plsql_batches(),
            ["SELECT 1 FROM dual;", "SELECT 2 FROM dual;"]
        );
    }
}
