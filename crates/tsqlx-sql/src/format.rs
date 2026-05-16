//! SQL autoformat — wraps the `sqlformat` crate with our defaults.

use sqlformat::{Indent, QueryParams};

/// Caller-visible formatting options. Loaded from `[editor.format]`
/// in `config.toml` by the TUI; this module is pure.
#[derive(Debug, Clone, Copy)]
pub struct FormatOptions {
    pub indent: usize,
    pub uppercase_keywords: bool,
    pub lines_between_queries: u8,
}

impl Default for FormatOptions {
    fn default() -> Self {
        // No upper-casing by default — opinionated formatters that
        // SHOUT every keyword annoy half our user base. Opt in via
        // config if you want it.
        Self {
            indent: 2,
            uppercase_keywords: false,
            lines_between_queries: 1,
        }
    }
}

/// Format `input` SQL with the given options. Idempotent:
/// `format_sql(format_sql(x, opts), opts) == format_sql(x, opts)`.
pub fn format_sql(input: &str, opts: FormatOptions) -> String {
    let upstream = sqlformat::FormatOptions {
        indent: Indent::Spaces(opts.indent as u8),
        uppercase: Some(opts.uppercase_keywords),
        lines_between_queries: opts.lines_between_queries,
        ignore_case_convert: None,
        ..sqlformat::FormatOptions::default()
    };
    sqlformat::format(input, &QueryParams::None, &upstream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_compresses_messy_whitespace() {
        let input = "select 1,2 from t where x=1";
        let out = format_sql(input, FormatOptions::default());
        assert_ne!(out, input);
        assert!(
            out.lines()
                .any(|l| l.trim_start().starts_with("select")
                    || l.trim_start().starts_with("SELECT")),
            "expected a line beginning with SELECT, got:\n{out}"
        );
    }

    #[test]
    fn idempotent_single_statement() {
        let opts = FormatOptions::default();
        let once = format_sql("select a, b from t where x = 1", opts);
        let twice = format_sql(&once, opts);
        assert_eq!(once, twice);
    }

    #[test]
    fn idempotent_two_statements() {
        let opts = FormatOptions::default();
        let once = format_sql("select 1 from a; select 2 from b", opts);
        let twice = format_sql(&once, opts);
        assert_eq!(once, twice);
    }

    #[test]
    fn uppercase_keywords_true() {
        let opts = FormatOptions {
            uppercase_keywords: true,
            ..FormatOptions::default()
        };
        let out = format_sql("select a from t", opts);
        assert!(out.contains("SELECT"), "missing SELECT in {out}");
        assert!(out.contains("FROM"), "missing FROM in {out}");
    }

    #[test]
    fn uppercase_keywords_false_keeps_lowercase() {
        let out = format_sql("select a from t", FormatOptions::default());
        assert!(out.contains("select"), "missing lowercase select in {out}");
        assert!(out.contains("from"), "missing lowercase from in {out}");
        assert!(!out.contains("SELECT"));
        assert!(!out.contains("FROM"));
    }

    #[test]
    fn indent_four_spaces() {
        let opts = FormatOptions {
            indent: 4,
            ..FormatOptions::default()
        };
        let out = format_sql("select a, b, c from t where x = 1 and y = 2", opts);
        assert!(
            out.lines()
                .any(|l| l.starts_with("    ") && !l.starts_with("     ")),
            "expected a line starting with exactly 4 spaces in:\n{out}"
        );
    }

    #[test]
    fn lines_between_queries_two_blank_lines() {
        // sqlformat 0.5 treats `lines_between_queries: N` as "emit N newlines
        // between queries" (N-1 blank lines), so we need 3 to observe ≥2 blanks.
        let opts = FormatOptions {
            lines_between_queries: 3,
            ..FormatOptions::default()
        };
        let out = format_sql("select 1 from a; select 2 from b", opts);
        assert!(
            out.contains("\n\n\n"),
            "expected >=2 blank lines between statements in:\n{out}"
        );
    }

    #[test]
    fn empty_input_does_not_panic() {
        let out = format_sql("", FormatOptions::default());
        assert!(out.trim().is_empty(), "expected blank output, got {out:?}");
    }
}
