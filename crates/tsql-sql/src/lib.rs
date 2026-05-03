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
    use super::{split_statements, SqlDocument};

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
}
