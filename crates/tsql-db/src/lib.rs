use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::sqlite::{SqlitePoolOptions, SqliteRow};
use sqlx::{Column, Row, TypeInfo, ValueRef};
use thiserror::Error;
use tsql_core::DriverKind;
use tsql_sql::SqlDocument;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseDriver {
    Postgres,
    Sqlite,
}

impl From<DriverKind> for DatabaseDriver {
    fn from(value: DriverKind) -> Self {
        match value {
            DriverKind::Postgres => Self::Postgres,
            DriverKind::Sqlite => Self::Sqlite,
        }
    }
}

pub trait DatabaseConnection {
    fn driver(&self) -> DatabaseDriver;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryOutput {
    pub statements: Vec<StatementOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementOutput {
    pub statement: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub rows_affected: u64,
}

pub async fn execute_script(
    driver: DriverKind,
    url: &str,
    document: &SqlDocument,
) -> Result<QueryOutput, DbError> {
    match driver {
        DriverKind::Postgres => execute_postgres(url, &document.statements()).await,
        DriverKind::Sqlite => execute_sqlite(url, &document.statements()).await,
    }
}

async fn execute_postgres(url: &str, statements: &[String]) -> Result<QueryOutput, DbError> {
    let pool = PgPoolOptions::new().max_connections(1).connect(url).await?;
    let mut output = Vec::with_capacity(statements.len());

    for statement in statements {
        if returns_rows(statement) {
            let rows = sqlx::query(statement).fetch_all(&pool).await?;
            output.push(postgres_rows(statement, &rows));
        } else {
            let result = sqlx::query(statement).execute(&pool).await?;
            output.push(StatementOutput {
                statement: statement.clone(),
                columns: Vec::new(),
                rows: Vec::new(),
                rows_affected: result.rows_affected(),
            });
        }
    }

    Ok(QueryOutput { statements: output })
}

async fn execute_sqlite(url: &str, statements: &[String]) -> Result<QueryOutput, DbError> {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(url)
        .await?;
    let mut output = Vec::with_capacity(statements.len());

    for statement in statements {
        if returns_rows(statement) {
            let rows = sqlx::query(statement).fetch_all(&pool).await?;
            output.push(sqlite_rows(statement, &rows));
        } else {
            let result = sqlx::query(statement).execute(&pool).await?;
            output.push(StatementOutput {
                statement: statement.clone(),
                columns: Vec::new(),
                rows: Vec::new(),
                rows_affected: result.rows_affected(),
            });
        }
    }

    Ok(QueryOutput { statements: output })
}

fn returns_rows(statement: &str) -> bool {
    let trimmed = statement.trim_start().to_ascii_lowercase();

    trimmed.starts_with("select")
        || trimmed.starts_with("with")
        || trimmed.starts_with("pragma")
        || trimmed.starts_with("show")
        || trimmed.starts_with("explain")
        || trimmed.contains(" returning ")
}

fn postgres_rows(statement: &str, rows: &[PgRow]) -> StatementOutput {
    let columns = rows
        .first()
        .map_or_else(Vec::new, |row| column_names(row.columns()));
    let rows = rows
        .iter()
        .map(|row| {
            (0..row.columns().len())
                .map(|index| postgres_cell(row, index))
                .collect()
        })
        .collect();

    StatementOutput {
        statement: statement.to_owned(),
        columns,
        rows,
        rows_affected: 0,
    }
}

fn sqlite_rows(statement: &str, rows: &[SqliteRow]) -> StatementOutput {
    let columns = rows
        .first()
        .map_or_else(Vec::new, |row| column_names(row.columns()));
    let rows = rows
        .iter()
        .map(|row| {
            (0..row.columns().len())
                .map(|index| sqlite_cell(row, index))
                .collect()
        })
        .collect();

    StatementOutput {
        statement: statement.to_owned(),
        columns,
        rows,
        rows_affected: 0,
    }
}

fn column_names(columns: &[impl Column]) -> Vec<String> {
    columns
        .iter()
        .map(|column| column.name().to_owned())
        .collect()
}

fn postgres_cell(row: &PgRow, index: usize) -> String {
    if row
        .try_get_raw(index)
        .is_ok_and(|value| ValueRef::is_null(&value))
    {
        return "NULL".to_owned();
    }

    row.try_get::<String, _>(index)
        .or_else(|_| row.try_get::<i64, _>(index).map(|value| value.to_string()))
        .or_else(|_| row.try_get::<i32, _>(index).map(|value| value.to_string()))
        .or_else(|_| row.try_get::<i16, _>(index).map(|value| value.to_string()))
        .or_else(|_| row.try_get::<f64, _>(index).map(|value| value.to_string()))
        .or_else(|_| row.try_get::<f32, _>(index).map(|value| value.to_string()))
        .or_else(|_| row.try_get::<bool, _>(index).map(|value| value.to_string()))
        .unwrap_or_else(|_| format!("<{}>", row.columns()[index].type_info().name()))
}

fn sqlite_cell(row: &SqliteRow, index: usize) -> String {
    if row
        .try_get_raw(index)
        .is_ok_and(|value| ValueRef::is_null(&value))
    {
        return "NULL".to_owned();
    }

    row.try_get::<String, _>(index)
        .or_else(|_| row.try_get::<i64, _>(index).map(|value| value.to_string()))
        .or_else(|_| row.try_get::<f64, _>(index).map(|value| value.to_string()))
        .or_else(|_| {
            row.try_get::<Vec<u8>, _>(index)
                .map(|value| format!("0x{}", hex_encode(&value)))
        })
        .unwrap_or_else(|_| format!("<{}>", row.columns()[index].type_info().name()))
}

fn hex_encode(bytes: &[u8]) -> String {
    const CHARS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);

    for byte in bytes {
        output.push(CHARS[(byte >> 4) as usize] as char);
        output.push(CHARS[(byte & 0x0f) as usize] as char);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::{returns_rows, DatabaseDriver};

    #[test]
    fn postgres_driver_is_distinct_from_sqlite() {
        assert_ne!(DatabaseDriver::Postgres, DatabaseDriver::Sqlite);
    }

    #[test]
    fn detects_row_returning_statements() {
        assert!(returns_rows("select 1"));
        assert!(returns_rows("with x as (select 1) select * from x"));
        assert!(returns_rows(
            "insert into users(name) values ('a') returning id"
        ));
        assert!(!returns_rows("create table users(id integer)"));
    }
}
