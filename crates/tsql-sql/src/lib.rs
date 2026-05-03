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
}

#[cfg(test)]
mod tests {
    use super::SqlDocument;

    #[test]
    fn document_preserves_multiline_sql() {
        let sql = "select 1;\nselect 2;";
        let document = SqlDocument::new(sql);

        assert_eq!(document.as_str(), sql);
    }
}
