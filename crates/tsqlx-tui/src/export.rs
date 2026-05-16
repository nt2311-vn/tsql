//! Export the active records grid to CSV / JSON / SQL INSERT.
//!
//! Pure functions over `StatementOutput` so they are trivially
//! testable without a database. Each encoder returns bytes (we never
//! load the whole result into a `String` because SQL-INSERT outputs
//! get large fast).

use std::io::{self, Write};

use thiserror::Error;
use tsqlx_db::StatementOutput;

#[derive(Debug, Error)]
pub enum ExportError {
    #[error("csv encoding failed: {0}")]
    Csv(#[from] csv::Error),
    #[error("json encoding failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("i/o failed: {0}")]
    Io(#[from] io::Error),
    #[error("missing table name for SQL INSERT export")]
    MissingTable,
    #[error("no columns in result set")]
    NoColumns,
}

/// Encode `rec` as RFC-4180 CSV. Header row is `rec.columns`.
/// Each cell renders as the source `String` (drivers already stringify).
pub fn export_csv(rec: &StatementOutput) -> Result<Vec<u8>, ExportError> {
    let mut wtr = csv::Writer::from_writer(Vec::new());
    wtr.write_record(&rec.columns)?;
    for row in &rec.rows {
        wtr.write_record(row)?;
    }
    wtr.flush()?;
    wtr.into_inner()
        .map_err(|e| ExportError::Io(e.into_error()))
}

/// Encode `rec` as a JSON array of objects. `pretty=true` indents
/// with two spaces; `pretty=false` emits NDJSON (one object per line,
/// no enclosing array).
pub fn export_json(rec: &StatementOutput, pretty: bool) -> Result<Vec<u8>, ExportError> {
    let objects: Vec<serde_json::Map<String, serde_json::Value>> = rec
        .rows
        .iter()
        .map(|row| {
            rec.columns
                .iter()
                .enumerate()
                .map(|(i, col)| {
                    let cell = row.get(i).cloned().unwrap_or_default();
                    (col.clone(), serde_json::Value::String(cell))
                })
                .collect()
        })
        .collect();

    if pretty {
        Ok(serde_json::to_vec_pretty(&objects)?)
    } else {
        let mut buf = Vec::new();
        for (i, obj) in objects.iter().enumerate() {
            if i > 0 {
                buf.write_all(b"\n")?;
            }
            serde_json::to_writer(&mut buf, obj)?;
        }
        Ok(buf)
    }
}

/// Encode `rec` as a single `INSERT INTO <table> (cols…) VALUES …;`
/// statement. Identifiers are always wrapped in double quotes; cell
/// values are wrapped in single quotes with inner single quotes
/// doubled. Empty `rec.rows` returns the no-op marker
/// `-- no rows to insert from <table>\n`.
pub fn export_sql_insert(rec: &StatementOutput, table: &str) -> Result<Vec<u8>, ExportError> {
    if table.is_empty() {
        return Err(ExportError::MissingTable);
    }
    if rec.columns.is_empty() {
        return Err(ExportError::NoColumns);
    }
    if rec.rows.is_empty() {
        return Ok(format!("-- no rows to insert from {table}\n").into_bytes());
    }

    let cols = rec
        .columns
        .iter()
        .map(|c| format!("\"{}\"", c.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(", ");

    let values = rec
        .rows
        .iter()
        .map(|row| {
            let cells = (0..rec.columns.len())
                .map(|i| {
                    let v = row.get(i).map(String::as_str).unwrap_or("");
                    format!("'{}'", v.replace('\'', "''"))
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("({cells})")
        })
        .collect::<Vec<_>>()
        .join(", ");

    Ok(format!("INSERT INTO \"{table}\" ({cols}) VALUES {values};").into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(columns: &[&str], rows: &[&[&str]]) -> StatementOutput {
        StatementOutput {
            statement: String::new(),
            columns: columns.iter().map(|s| s.to_string()).collect(),
            rows: rows
                .iter()
                .map(|r| r.iter().map(|s| s.to_string()).collect())
                .collect(),
            rows_affected: 0,
        }
    }

    #[test]
    fn csv_roundtrips_header_and_rows() {
        let r = rec(&["id", "name"], &[&["1", "alice"], &["2", "bob"]]);
        let out = export_csv(&r).unwrap();

        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(false)
            .from_reader(out.as_slice());
        let records: Vec<Vec<String>> = rdr
            .records()
            .map(|r| r.unwrap().iter().map(|s| s.to_string()).collect())
            .collect();

        assert_eq!(
            records,
            vec![
                vec!["id".to_string(), "name".to_string()],
                vec!["1".to_string(), "alice".to_string()],
                vec!["2".to_string(), "bob".to_string()],
            ]
        );
    }

    #[test]
    fn csv_escapes_commas_and_quotes() {
        let r = rec(&["a", "b"], &[&["hello, world", "she said \"hi\""]]);
        let out = export_csv(&r).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("\"hello, world\""));
        assert!(s.contains("\"she said \"\"hi\"\"\""));

        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(true)
            .from_reader(s.as_bytes());
        let row = rdr.records().next().unwrap().unwrap();
        assert_eq!(&row[0], "hello, world");
        assert_eq!(&row[1], "she said \"hi\"");
    }

    #[test]
    fn json_pretty_parses_back() {
        let r = rec(&["id", "name"], &[&["1", "alice"], &["2", "bob"]]);
        let out = export_json(&r, true).unwrap();
        let parsed: Vec<serde_json::Map<String, serde_json::Value>> =
            serde_json::from_slice(&out).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0]["id"], serde_json::Value::String("1".into()));
        assert_eq!(parsed[1]["name"], serde_json::Value::String("bob".into()));
        let s = std::str::from_utf8(&out).unwrap();
        assert!(s.contains('\n'));
    }

    #[test]
    fn json_ndjson_one_object_per_line() {
        let r = rec(&["k"], &[&["a"], &["b"], &["c"]]);
        let out = export_json(&r, false).unwrap();
        let s = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = s.split('\n').collect();
        assert_eq!(lines.len(), 3);
        for line in &lines {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(v.is_object());
        }
        assert!(!s.starts_with('['));
    }

    #[test]
    fn sql_insert_emits_tuples_and_escapes_quotes() {
        let r = rec(&["id", "name"], &[&["1", "O'Reilly"], &["2", "plain"]]);
        let out = export_sql_insert(&r, "users").unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("INSERT INTO \"users\" (\"id\", \"name\") VALUES "));
        assert!(s.contains("('1', 'O''Reilly')"));
        assert!(s.contains("('2', 'plain')"));
        assert!(s.ends_with(';'));
    }

    #[test]
    fn sql_insert_empty_rows_returns_noop_marker() {
        let r = rec(&["id"], &[]);
        let out = export_sql_insert(&r, "users").unwrap();
        assert_eq!(out, b"-- no rows to insert from users\n");
    }

    #[test]
    fn sql_insert_missing_table_errors() {
        let r = rec(&["id"], &[&["1"]]);
        assert!(matches!(
            export_sql_insert(&r, ""),
            Err(ExportError::MissingTable)
        ));
    }

    #[test]
    fn sql_insert_no_columns_errors() {
        let r = rec(&[], &[]);
        assert!(matches!(
            export_sql_insert(&r, "t"),
            Err(ExportError::NoColumns)
        ));
    }
}
