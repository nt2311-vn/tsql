//! SQL autocomplete — cursor-context detection, prefix extraction,
//! and candidate ranking. Pure functions over a buffer + cursor byte
//! offset; the editor wires these into a popup overlay.
//!
//! No async, no I/O. Uses the existing `sqlparser` tokenizer so we
//! reuse the dependency PR #31 already brought in.

use sqlparser::dialect::GenericDialect;
use sqlparser::keywords::Keyword;
use sqlparser::tokenizer::{Token, Tokenizer};

/// Where the cursor sits in the current SQL statement. The editor
/// uses this to pick which candidate set to surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionContext {
    /// Start of buffer, or after a `;` — offer top-level keywords.
    Statement,
    /// Last meaningful token is `FROM`. Offer table names.
    AfterFrom,
    /// Last meaningful token is `JOIN`. Offer table names.
    AfterJoin,
    /// Last meaningful token is `INTO`. Offer table names.
    AfterInto,
    /// Last meaningful token is `UPDATE`. Offer table names.
    AfterUpdate,
    /// User typed `<qualifier>.` and is now typing the suffix.
    /// Qualifier may be a schema name (offer its tables) or a table
    /// name / alias (offer its columns).
    AfterDot(String),
    /// Inside the projection list of a SELECT (between SELECT and FROM).
    AfterSelectProjection,
    /// After a WHERE keyword.
    AfterWhere,
    /// After an ON keyword (JOIN ... ON ...).
    AfterOn,
    /// After GROUP BY.
    AfterGroupBy,
    /// After ORDER BY.
    AfterOrderBy,
    /// Anything else — generally don't show a popup.
    Other,
}

/// What kind of thing a candidate represents. The editor maps this
/// to an icon / colour in the popup row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind {
    Keyword,
    Table,
    Column,
    Schema,
}

/// A single completion suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub text: String,
    pub kind: CandidateKind,
    /// Filled in by [`rank_candidates`]. Higher = better. Ignored
    /// by callers who only consume the post-rank result.
    pub score: i32,
}

impl Candidate {
    pub fn new(text: impl Into<String>, kind: CandidateKind) -> Self {
        Self {
            text: text.into(),
            kind,
            score: 0,
        }
    }
}

/// Classify the cursor position by walking the sqlparser tokens that
/// precede it. `cursor` is a byte offset into `input`; it must sit on
/// a UTF-8 char boundary.
pub fn context_at(input: &str, cursor: usize) -> CompletionContext {
    let end = cursor.min(input.len());
    let prefix = &input[..end];
    let tokens = Tokenizer::new(&GenericDialect, prefix)
        .tokenize_with_location()
        .unwrap_or_default();

    let meaningful: Vec<&Token> = tokens
        .iter()
        .map(|t| &t.token)
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect();

    let Some(last) = meaningful.last() else {
        return CompletionContext::Statement;
    };

    if matches!(last, Token::SemiColon) {
        return CompletionContext::Statement;
    }

    if matches!(last, Token::Period) {
        if let Some(Token::Word(w)) = meaningful.get(meaningful.len().saturating_sub(2)) {
            if w.keyword == Keyword::NoKeyword {
                return CompletionContext::AfterDot(w.value.clone());
            }
        }
    }

    if let Token::Word(w) = last {
        match w.keyword {
            Keyword::FROM => return CompletionContext::AfterFrom,
            Keyword::JOIN => return CompletionContext::AfterJoin,
            Keyword::INTO => return CompletionContext::AfterInto,
            Keyword::UPDATE => return CompletionContext::AfterUpdate,
            Keyword::WHERE => return CompletionContext::AfterWhere,
            Keyword::ON => return CompletionContext::AfterOn,
            Keyword::SELECT => return CompletionContext::AfterSelectProjection,
            Keyword::BY => {
                let prev = meaningful.iter().rev().skip(1).find_map(|t| {
                    if let Token::Word(w) = t {
                        Some(w)
                    } else {
                        None
                    }
                });
                if let Some(p) = prev {
                    match p.keyword {
                        Keyword::GROUP => return CompletionContext::AfterGroupBy,
                        Keyword::ORDER => return CompletionContext::AfterOrderBy,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    for tok in meaningful.iter().rev() {
        if let Token::Word(w) = tok {
            match w.keyword {
                Keyword::FROM => return CompletionContext::AfterFrom,
                Keyword::SELECT => return CompletionContext::AfterSelectProjection,
                _ => {}
            }
        }
    }

    CompletionContext::Other
}

/// Extract the partial word the user is typing at `cursor`. Returns
/// `(prefix_text, prefix_byte_start)` so the editor can decide what
/// span to replace when a candidate is accepted.
///
/// Identifier chars are `[A-Za-z0-9_]`. If the byte directly before
/// `cursor` is not an identifier char, returns `("", cursor)`.
/// Also: if the user is mid-qualified-ref like `users.na|`, this
/// returns just `"na"` — the qualifier is surfaced separately by
/// [`context_at`] via `CompletionContext::AfterDot`.
pub fn prefix_at(input: &str, cursor: usize) -> (String, usize) {
    let end = cursor.min(input.len());
    let bytes = input.as_bytes();
    let mut start = end;
    while start > 0 {
        let b = bytes[start - 1];
        if b.is_ascii_alphanumeric() || b == b'_' {
            start -= 1;
        } else {
            break;
        }
    }
    if start == end {
        return (String::new(), end);
    }
    (input[start..end].to_string(), start)
}

/// Rank `candidates` against `prefix`. Sort order, best first:
///   1. case-insensitive prefix match
///   2. case-insensitive substring match
///   3. case-insensitive char-subsequence (fuzzy) match
///   4. drop everything that doesn't even fuzzy-match
///
/// Within a tier, shorter candidates beat longer ones (stable on tie).
/// `limit == 0` means no cap.
pub fn rank_candidates(prefix: &str, candidates: Vec<Candidate>, limit: usize) -> Vec<Candidate> {
    let cap = |v: Vec<Candidate>| -> Vec<Candidate> {
        if limit == 0 {
            v
        } else {
            v.into_iter().take(limit).collect()
        }
    };

    if prefix.is_empty() {
        let mut out: Vec<Candidate> = candidates
            .into_iter()
            .map(|mut c| {
                c.score = 500;
                c
            })
            .collect();
        out.sort_by_key(|a| a.text.to_ascii_lowercase());
        return cap(out);
    }

    let prefix_lower = prefix.to_ascii_lowercase();
    let mut scored: Vec<Candidate> = candidates
        .into_iter()
        .filter_map(|mut c| {
            let text_lower = c.text.to_ascii_lowercase();
            let score = if text_lower.starts_with(&prefix_lower) {
                let bonus = if c.text.starts_with(prefix) { 10 } else { 0 };
                1000 + bonus
            } else if text_lower.contains(&prefix_lower) {
                500
            } else if is_subsequence(&prefix_lower, &text_lower) {
                100
            } else {
                0
            };
            if score == 0 {
                None
            } else {
                c.score = score;
                Some(c)
            }
        })
        .collect();

    scored.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.text.len().cmp(&b.text.len()))
            .then_with(|| {
                a.text
                    .to_ascii_lowercase()
                    .cmp(&b.text.to_ascii_lowercase())
            })
    });

    cap(scored)
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut it = haystack.chars();
    'outer: for nc in needle.chars() {
        for hc in it.by_ref() {
            if hc == nc {
                continue 'outer;
            }
        }
        return false;
    }
    true
}

/// The static set of top-level keywords offered at `CompletionContext::Statement`.
pub fn top_level_keywords() -> Vec<Candidate> {
    [
        "SELECT", "INSERT", "UPDATE", "DELETE", "WITH", "CREATE", "DROP", "ALTER", "TRUNCATE",
        "BEGIN", "COMMIT", "ROLLBACK", "EXPLAIN", "VACUUM", "ANALYZE", "GRANT", "REVOKE", "SHOW",
        "USE",
    ]
    .iter()
    .map(|k| Candidate::new(*k, CandidateKind::Keyword))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Candidate {
        Candidate::new(s, CandidateKind::Table)
    }

    #[test]
    fn context_empty_is_statement() {
        assert_eq!(context_at("", 0), CompletionContext::Statement);
    }

    #[test]
    fn context_after_from() {
        assert_eq!(
            context_at("SELECT * FROM ", 14),
            CompletionContext::AfterFrom
        );
    }

    #[test]
    fn context_after_update() {
        assert_eq!(context_at("UPDATE ", 7), CompletionContext::AfterUpdate);
    }

    #[test]
    fn context_after_into() {
        assert_eq!(context_at("INSERT INTO ", 12), CompletionContext::AfterInto);
    }

    #[test]
    fn context_after_dot() {
        assert_eq!(
            context_at("SELECT u.", 9),
            CompletionContext::AfterDot("u".to_string())
        );
    }

    #[test]
    fn context_select_projection() {
        assert_eq!(
            context_at("SELECT col", 10),
            CompletionContext::AfterSelectProjection
        );
    }

    #[test]
    fn context_after_where() {
        assert_eq!(
            context_at("SELECT a FROM t WHERE ", 22),
            CompletionContext::AfterWhere
        );
    }

    #[test]
    fn context_after_on() {
        assert_eq!(
            context_at("SELECT a FROM t JOIN u ON ", 26),
            CompletionContext::AfterOn
        );
    }

    #[test]
    fn context_after_group_by() {
        assert_eq!(
            context_at("SELECT a FROM t GROUP BY ", 25),
            CompletionContext::AfterGroupBy
        );
    }

    #[test]
    fn context_after_order_by() {
        assert_eq!(
            context_at("SELECT a FROM t ORDER BY ", 25),
            CompletionContext::AfterOrderBy
        );
    }

    #[test]
    fn context_after_semicolon() {
        assert_eq!(context_at("SELECT 1; ", 10), CompletionContext::Statement);
    }

    #[test]
    fn context_after_join() {
        assert_eq!(
            context_at("SELECT a FROM t LEFT JOIN ", 26),
            CompletionContext::AfterJoin
        );
    }

    #[test]
    fn prefix_basic() {
        assert_eq!(prefix_at("SELECT us", 9), ("us".to_string(), 7));
    }

    #[test]
    fn prefix_at_space() {
        assert_eq!(prefix_at("SELECT ", 7), (String::new(), 7));
    }

    #[test]
    fn prefix_after_dot() {
        assert_eq!(prefix_at("SELECT u.na", 11), ("na".to_string(), 9));
    }

    #[test]
    fn prefix_cursor_past_end() {
        assert_eq!(prefix_at("abc", 99), ("abc".to_string(), 0));
    }

    #[test]
    fn rank_prefix_then_substring_drops_no_match() {
        let out = rank_candidates(
            "us",
            vec![c("users"), c("usage"), c("status"), c("orders")],
            0,
        );
        let names: Vec<&str> = out.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(names, vec!["usage", "users", "status"]);
    }

    #[test]
    fn rank_no_match_returns_empty() {
        let out = rank_candidates("xyz", vec![c("users")], 0);
        assert!(out.is_empty());
    }

    #[test]
    fn rank_empty_prefix_alphabetical() {
        let out = rank_candidates("", vec![c("b"), c("a")], 0);
        let names: Vec<&str> = out.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn rank_respects_limit() {
        let many: Vec<Candidate> = (0..20).map(|i| c(&format!("user{i:02}"))).collect();
        let out = rank_candidates("us", many, 5);
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn rank_exact_case_beats_other_case() {
        let out = rank_candidates("SE", vec![c("select_one"), c("SELECT_ONE")], 0);
        assert_eq!(out[0].text, "SELECT_ONE");
    }

    #[test]
    fn rank_fuzzy_match_scores_lower_than_substring() {
        let out = rank_candidates("ac", vec![c("abc"), c("cab")], 0);
        assert_eq!(out[0].text, "abc");
        assert_eq!(out[0].score, 100);
    }
}
