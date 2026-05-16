//! SQL syntax classification for editor highlighting.
//!
//! Pure tokenizer + classifier. Hands the editor a vector of
//! `(byte_range, SpanClass)` per line so the renderer can paint
//! token-aware colours. We deliberately do *not* parse — just
//! tokenize — so the highlight survives half-typed statements.

use sqlparser::dialect::GenericDialect;
use sqlparser::keywords::Keyword;
use sqlparser::tokenizer::{Token, Tokenizer, Whitespace};

/// Coarse token class that the editor maps to a theme colour.
/// Kept small on purpose; we don't want a separate class per
/// reserved word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanClass {
    Keyword,
    Type,
    Identifier,
    StringLit,
    NumberLit,
    Comment,
    Operator,
    Punct,
    Plain,
}

/// Byte-range (relative to the input slice) tagged with its class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedSpan {
    pub start: usize,
    pub end: usize,
    pub class: SpanClass,
}

/// Tokenize `line` against the generic SQL dialect and return one
/// `ClassifiedSpan` per token. Multi-line tokens (block comments,
/// dollar-quoted bodies that span newlines) are NOT handled here —
/// the caller must pass full statements if it wants those tracked.
/// For single-line editor highlighting this restriction is fine; we
/// will add a streaming variant in a follow-up if/when we need it.
pub fn classify_line(line: &str) -> Vec<ClassifiedSpan> {
    if line.is_empty() {
        return Vec::new();
    }

    let dialect = GenericDialect {};
    let Ok(tokens) = Tokenizer::new(&dialect, line).tokenize_with_location() else {
        return Vec::new();
    };

    let char_byte_offsets = build_char_byte_table(line);
    let mut spans = Vec::with_capacity(tokens.len());

    for tws in tokens {
        let Some(class) = classify_token(&tws.token) else {
            continue;
        };

        let start = column_to_byte(
            &char_byte_offsets,
            tws.span.start.line,
            tws.span.start.column,
        );
        let end_raw = column_to_byte(&char_byte_offsets, tws.span.end.line, tws.span.end.column);
        let end = end_raw.min(line.len()).max(start);
        if end == start {
            continue;
        }
        spans.push(ClassifiedSpan { start, end, class });
    }

    spans
}

fn build_char_byte_table(line: &str) -> Vec<usize> {
    let mut table = Vec::with_capacity(line.len() + 1);
    let mut byte = 0usize;
    table.push(byte);
    for ch in line.chars() {
        byte += ch.len_utf8();
        table.push(byte);
    }
    table
}

fn column_to_byte(table: &[usize], line_no: u64, column: u64) -> usize {
    if line_no > 1 {
        return *table.last().unwrap_or(&0);
    }
    if column == 0 {
        return 0;
    }
    let idx = (column as usize).saturating_sub(1).min(table.len() - 1);
    table[idx]
}

fn classify_token(token: &Token) -> Option<SpanClass> {
    match token {
        Token::Word(w) => Some(if w.keyword == Keyword::NoKeyword {
            SpanClass::Identifier
        } else if is_type_keyword(w.keyword) {
            SpanClass::Type
        } else {
            SpanClass::Keyword
        }),

        Token::SingleQuotedString(_)
        | Token::DoubleQuotedString(_)
        | Token::TripleSingleQuotedString(_)
        | Token::TripleDoubleQuotedString(_)
        | Token::NationalStringLiteral(_)
        | Token::EscapedStringLiteral(_)
        | Token::UnicodeStringLiteral(_)
        | Token::HexStringLiteral(_)
        | Token::DollarQuotedString(_) => Some(SpanClass::StringLit),

        Token::Number(_, _) => Some(SpanClass::NumberLit),

        Token::Whitespace(Whitespace::SingleLineComment { .. })
        | Token::Whitespace(Whitespace::MultiLineComment(_)) => Some(SpanClass::Comment),
        Token::Whitespace(_) => None,

        Token::Plus
        | Token::Minus
        | Token::Mul
        | Token::Div
        | Token::Mod
        | Token::Eq
        | Token::Neq
        | Token::Lt
        | Token::LtEq
        | Token::Gt
        | Token::GtEq
        | Token::Pipe
        | Token::Caret
        | Token::Ampersand
        | Token::DoubleEq
        | Token::Spaceship
        | Token::StringConcat
        | Token::ArrowAt
        | Token::AtArrow
        | Token::LongArrow
        | Token::Arrow
        | Token::DoubleColon => Some(SpanClass::Operator),

        Token::Comma
        | Token::SemiColon
        | Token::LParen
        | Token::RParen
        | Token::LBracket
        | Token::RBracket
        | Token::LBrace
        | Token::RBrace
        | Token::Period
        | Token::Colon
        | Token::Tilde => Some(SpanClass::Punct),

        Token::EOF => None,
        _ => Some(SpanClass::Plain),
    }
}

fn is_type_keyword(k: Keyword) -> bool {
    matches!(
        k,
        Keyword::INT
            | Keyword::INTEGER
            | Keyword::BIGINT
            | Keyword::SMALLINT
            | Keyword::TINYINT
            | Keyword::BOOL
            | Keyword::BOOLEAN
            | Keyword::TEXT
            | Keyword::VARCHAR
            | Keyword::CHAR
            | Keyword::BLOB
            | Keyword::BYTEA
            | Keyword::JSON
            | Keyword::JSONB
            | Keyword::UUID
            | Keyword::TIMESTAMP
            | Keyword::TIMESTAMPTZ
            | Keyword::DATE
            | Keyword::TIME
            | Keyword::DECIMAL
            | Keyword::NUMERIC
            | Keyword::REAL
            | Keyword::FLOAT
            | Keyword::DOUBLE
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_span<'a>(
        line: &'a str,
        spans: &'a [ClassifiedSpan],
        substring: &str,
        class: SpanClass,
    ) -> &'a ClassifiedSpan {
        spans
            .iter()
            .find(|s| s.class == class && &line[s.start..s.end] == substring)
            .unwrap_or_else(|| {
                panic!(
                    "expected span ({class:?}, {substring:?}) not found in {:?}",
                    spans
                        .iter()
                        .map(|s| (s.class, &line[s.start..s.end]))
                        .collect::<Vec<_>>()
                )
            })
    }

    #[test]
    fn simple_select() {
        let line = "SELECT a, b FROM t";
        let spans = classify_line(line);
        find_span(line, &spans, "SELECT", SpanClass::Keyword);
        find_span(line, &spans, "FROM", SpanClass::Keyword);
        find_span(line, &spans, "a", SpanClass::Identifier);
        find_span(line, &spans, "b", SpanClass::Identifier);
        find_span(line, &spans, "t", SpanClass::Identifier);
        find_span(line, &spans, ",", SpanClass::Punct);
        assert!(!spans.iter().any(|s| s.class == SpanClass::StringLit));
    }

    #[test]
    fn quoted_string() {
        let line = "SELECT 'hello' AS x";
        let spans = classify_line(line);
        find_span(line, &spans, "'hello'", SpanClass::StringLit);
        find_span(line, &spans, "SELECT", SpanClass::Keyword);
        find_span(line, &spans, "AS", SpanClass::Keyword);
        find_span(line, &spans, "x", SpanClass::Identifier);
    }

    #[test]
    fn numbers() {
        let line = "SELECT 42, 3.14 FROM t";
        let spans = classify_line(line);
        let nums: Vec<_> = spans
            .iter()
            .filter(|s| s.class == SpanClass::NumberLit)
            .map(|s| &line[s.start..s.end])
            .collect();
        assert_eq!(nums, vec!["42", "3.14"]);
    }

    #[test]
    fn single_line_comment() {
        let line = "SELECT 1 -- this is a comment";
        let spans = classify_line(line);
        let last = spans.last().expect("non-empty spans");
        assert_eq!(last.class, SpanClass::Comment);
        assert!(line[last.start..last.end].contains("comment"));
    }

    #[test]
    fn multi_line_comment_inline() {
        let line = "/* hello */ SELECT 1";
        let spans = classify_line(line);
        let first = spans.first().expect("non-empty spans");
        assert_eq!(first.class, SpanClass::Comment);
        assert_eq!(&line[first.start..first.end], "/* hello */");
    }

    #[test]
    fn type_mapping() {
        let line = "CREATE TABLE t (id INT, name TEXT)";
        let spans = classify_line(line);
        find_span(line, &spans, "INT", SpanClass::Type);
        find_span(line, &spans, "TEXT", SpanClass::Type);
        find_span(line, &spans, "CREATE", SpanClass::Keyword);
        find_span(line, &spans, "TABLE", SpanClass::Keyword);
    }

    #[test]
    fn dollar_quoted() {
        let line = "SELECT $tag$ body $tag$ FROM t";
        let spans = classify_line(line);
        let s = find_span(line, &spans, "$tag$ body $tag$", SpanClass::StringLit);
        assert_eq!(&line[s.start..s.end], "$tag$ body $tag$");
    }

    #[test]
    fn empty_line() {
        assert!(classify_line("").is_empty());
    }

    #[test]
    fn whitespace_only() {
        assert!(classify_line("   \t  ").is_empty());
    }

    #[test]
    fn byte_range_sanity() {
        let line = "SELECT 1";
        let spans = classify_line(line);
        let kw = find_span(line, &spans, "SELECT", SpanClass::Keyword);
        assert_eq!(&line[kw.start..kw.end], "SELECT");
    }
}
