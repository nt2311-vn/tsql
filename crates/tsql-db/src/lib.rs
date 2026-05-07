use std::collections::BTreeMap;
use std::str::FromStr;

use sqlx::postgres::{PgPool, PgPoolOptions, PgRow};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions, SqliteRow};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseOverview {
    pub schemas: Vec<SchemaInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaInfo {
    pub name: String,
    pub tables: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableInfo {
    pub name: String,
    pub schema: String,
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
    pub primary_key: Option<PrimaryKeyInfo>,
    pub foreign_keys: Vec<ForeignKeyInfo>,
    pub constraints: Vec<ConstraintInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub default_value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexInfo {
    pub name: String,
    pub column_names: Vec<String>,
    pub is_unique: bool,
    /// Whether this is the index backing the primary-key constraint.
    /// SQLite reports this via `PRAGMA index_list.origin == 'pk'`;
    /// Postgres via `pg_index.indisprimary`.
    pub is_primary: bool,
    /// Index access method, lowercased. Postgres: `btree`, `hash`,
    /// `gin`, `gist`, `brin`, `spgist`. SQLite: always `btree`
    /// (FTS / R*Tree show up as virtual tables, not in PRAGMA).
    pub method: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrimaryKeyInfo {
    pub name: String,
    pub column_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKeyInfo {
    pub name: String,
    pub column_names: Vec<String>,
    pub referenced_table: String,
    pub referenced_columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstraintInfo {
    pub name: String,
    pub definition: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationshipEdge {
    pub from_table: String,
    pub from_columns: Vec<String>,
    pub to_table: String,
    pub to_columns: Vec<String>,
}

/// A reusable database connection pool. Cheap to clone (each variant is
/// `Arc`-backed in `sqlx`), so it can be shared across tasks and used to
/// avoid the per-call connection handshake that the URL-based helpers pay.
#[derive(Debug, Clone)]
pub enum Pool {
    Postgres(PgPool),
    Sqlite(SqlitePool),
}

impl Pool {
    /// Open a fresh pool for the given driver and URL.
    pub async fn connect(driver: DriverKind, url: &str) -> Result<Self, DbError> {
        match driver {
            DriverKind::Postgres => {
                let pool = PgPoolOptions::new().max_connections(4).connect(url).await?;
                Ok(Self::Postgres(pool))
            }
            DriverKind::Sqlite => Ok(Self::Sqlite(sqlite_pool(url).await?)),
        }
    }

    pub fn driver(&self) -> DriverKind {
        match self {
            Pool::Postgres(_) => DriverKind::Postgres,
            Pool::Sqlite(_) => DriverKind::Sqlite,
        }
    }

    pub async fn execute_script(&self, document: &SqlDocument) -> Result<QueryOutput, DbError> {
        let stmts = document.statements();
        match self {
            Pool::Postgres(pool) => execute_postgres(pool, &stmts).await,
            Pool::Sqlite(pool) => execute_sqlite(pool, &stmts).await,
        }
    }

    pub async fn fetch_overview(&self) -> Result<DatabaseOverview, DbError> {
        match self {
            Pool::Postgres(pool) => fetch_postgres_overview(pool).await,
            Pool::Sqlite(pool) => fetch_sqlite_overview(pool).await,
        }
    }

    pub async fn fetch_table_info(&self, schema: &str, table: &str) -> Result<TableInfo, DbError> {
        match self {
            Pool::Postgres(pool) => fetch_postgres_table_info(pool, schema, table).await,
            Pool::Sqlite(pool) => fetch_sqlite_table_info(pool, schema, table).await,
        }
    }

    pub async fn fetch_records(
        &self,
        schema: &str,
        table: &str,
        limit: usize,
        offset: usize,
    ) -> Result<StatementOutput, DbError> {
        let sql = match self.driver() {
            DriverKind::Postgres => {
                format!("SELECT * FROM \"{schema}\".\"{table}\" LIMIT {limit} OFFSET {offset}")
            }
            DriverKind::Sqlite => {
                format!("SELECT * FROM \"{table}\" LIMIT {limit} OFFSET {offset}")
            }
        };
        let document = SqlDocument::new(sql);
        self.execute_script(&document)
            .await?
            .statements
            .into_iter()
            .next()
            .ok_or_else(|| DbError::Sqlx(sqlx::Error::RowNotFound))
    }

    pub async fn fetch_relationships(
        &self,
        schema: &str,
    ) -> Result<Vec<RelationshipEdge>, DbError> {
        match self {
            Pool::Postgres(pool) => fetch_postgres_relationships(pool, schema).await,
            Pool::Sqlite(pool) => fetch_sqlite_relationships(pool, schema).await,
        }
    }
}

pub async fn execute_script(
    driver: DriverKind,
    url: &str,
    document: &SqlDocument,
) -> Result<QueryOutput, DbError> {
    Pool::connect(driver, url)
        .await?
        .execute_script(document)
        .await
}

pub async fn fetch_overview(driver: DriverKind, url: &str) -> Result<DatabaseOverview, DbError> {
    Pool::connect(driver, url).await?.fetch_overview().await
}

pub async fn fetch_table_info(
    driver: DriverKind,
    url: &str,
    schema: &str,
    table: &str,
) -> Result<TableInfo, DbError> {
    Pool::connect(driver, url)
        .await?
        .fetch_table_info(schema, table)
        .await
}

pub async fn fetch_records(
    driver: DriverKind,
    url: &str,
    schema: &str,
    table: &str,
    limit: usize,
    offset: usize,
) -> Result<StatementOutput, DbError> {
    Pool::connect(driver, url)
        .await?
        .fetch_records(schema, table, limit, offset)
        .await
}

type PragmaFkRow = (i64, i64, String, String, String, String, String, String);

async fn sqlite_pool(url: &str) -> Result<SqlitePool, DbError> {
    let opts = SqliteConnectOptions::from_str(url)
        .map_err(DbError::Sqlx)?
        .create_if_missing(true);
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .map_err(DbError::Sqlx)
}

async fn fetch_sqlite_overview(pool: &SqlitePool) -> Result<DatabaseOverview, DbError> {
    let tables: Vec<(String,)> = sqlx::query_as(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .fetch_all(pool)
    .await?;

    Ok(DatabaseOverview {
        schemas: vec![SchemaInfo {
            name: "main".to_owned(),
            tables: tables.into_iter().map(|(t,)| t).collect(),
        }],
    })
}

async fn fetch_sqlite_table_info(
    pool: &SqlitePool,
    _schema: &str,
    table: &str,
) -> Result<TableInfo, DbError> {
    // Columns
    let columns: Vec<(i64, String, String, i64, Option<String>, i64)> =
        sqlx::query_as(&format!("PRAGMA table_info(\"{}\")", table))
            .fetch_all(pool)
            .await?;

    let column_infos = columns
        .into_iter()
        .map(|(_, name, data_type, notnull, dflt_value, _)| ColumnInfo {
            name,
            data_type,
            is_nullable: notnull == 0,
            default_value: dflt_value,
        })
        .collect();

    // Primary Key
    let pk_columns: Vec<String> = sqlx::query_as(&format!("PRAGMA table_info(\"{}\")", table))
        .fetch_all(pool)
        .await?
        .into_iter()
        .filter(|row: &(i64, String, String, i64, Option<String>, i64)| row.5 > 0)
        .map(|row| row.1)
        .collect();

    let primary_key = if pk_columns.is_empty() {
        None
    } else {
        Some(PrimaryKeyInfo {
            name: "PRIMARY KEY".to_owned(),
            column_names: pk_columns,
        })
    };

    // Foreign Keys
    let fks: Vec<PragmaFkRow> = sqlx::query_as(&format!("PRAGMA foreign_key_list(\"{}\")", table))
        .fetch_all(pool)
        .await?;

    let mut foreign_keys = Vec::new();
    for (_, _, ref_table, from, to, _, _, _) in fks {
        foreign_keys.push(ForeignKeyInfo {
            name: format!("FK_{}_{}", table, ref_table),
            column_names: vec![from],
            referenced_table: ref_table,
            referenced_columns: vec![to],
        });
    }

    // Indexes. PRAGMA index_list rows: (seq, name, unique, origin, partial).
    // origin is 'c' (CREATE INDEX), 'u' (UNIQUE constraint), 'pk' (PK).
    let index_list: Vec<(i64, String, i64, String, i64)> =
        sqlx::query_as(&format!("PRAGMA index_list(\"{}\")", table))
            .fetch_all(pool)
            .await?;

    let mut indexes = Vec::new();
    for (_, index_name, unique, origin, _) in index_list {
        let index_info: Vec<(i64, i64, String)> =
            sqlx::query_as(&format!("PRAGMA index_info(\"{}\")", index_name))
                .fetch_all(pool)
                .await?;

        indexes.push(IndexInfo {
            name: index_name,
            column_names: index_info.into_iter().map(|(_, _, name)| name).collect(),
            is_unique: unique == 1,
            is_primary: origin == "pk",
            // Every regular SQLite index uses a B-tree under the hood;
            // FTS/R*Tree etc. live as virtual tables and don't appear
            // in PRAGMA index_list.
            method: "btree".to_owned(),
        });
    }

    Ok(TableInfo {
        name: table.to_owned(),
        schema: "main".to_owned(),
        columns: column_infos,
        indexes,
        primary_key,
        foreign_keys,
        constraints: Vec::new(),
    })
}

async fn fetch_postgres_overview(pool: &PgPool) -> Result<DatabaseOverview, DbError> {
    // Hide every Postgres-internal schema. `information_schema.schemata`
    // includes `pg_toast`, `pg_temp_*`, and `pg_toast_temp_*` alongside
    // `pg_catalog`, so a literal NOT IN list isn't enough — match the
    // whole `pg_%` family with LIKE plus information_schema explicitly.
    let schemas: Vec<(String,)> = sqlx::query_as(
        "SELECT schema_name FROM information_schema.schemata
         WHERE schema_name NOT LIKE 'pg\\_%' ESCAPE '\\'
           AND schema_name <> 'information_schema'
         ORDER BY schema_name",
    )
    .fetch_all(pool)
    .await?;

    let mut schema_infos = Vec::with_capacity(schemas.len());
    for (schema_name,) in schemas {
        let tables: Vec<(String,)> = sqlx::query_as(
            "SELECT table_name FROM information_schema.tables 
             WHERE table_schema = $1 AND table_type = 'BASE TABLE'
             ORDER BY table_name",
        )
        .bind(&schema_name)
        .fetch_all(pool)
        .await?;

        schema_infos.push(SchemaInfo {
            name: schema_name,
            tables: tables.into_iter().map(|(t,)| t).collect(),
        });
    }

    Ok(DatabaseOverview {
        schemas: schema_infos,
    })
}

async fn fetch_postgres_table_info(
    pool: &PgPool,
    schema: &str,
    table: &str,
) -> Result<TableInfo, DbError> {
    // Columns
    let columns: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT column_name, data_type, is_nullable, column_default 
         FROM information_schema.columns 
         WHERE table_schema = $1 AND table_name = $2
         ORDER BY ordinal_position",
    )
    .bind(schema)
    .bind(table)
    .fetch_all(pool)
    .await?;

    let column_infos = columns
        .into_iter()
        .map(|(name, data_type, is_nullable, default_value)| ColumnInfo {
            name,
            data_type,
            is_nullable: is_nullable == "YES",
            default_value,
        })
        .collect();

    // Primary Key
    let pk_columns: Vec<(String,)> = sqlx::query_as(
        "SELECT kcu.column_name 
         FROM information_schema.table_constraints tc 
         JOIN information_schema.key_column_usage kcu 
           ON tc.constraint_name = kcu.constraint_name 
           AND tc.table_schema = kcu.table_schema 
         WHERE tc.constraint_type = 'PRIMARY KEY' 
           AND tc.table_schema = $1 AND tc.table_name = $2
         ORDER BY kcu.ordinal_position",
    )
    .bind(schema)
    .bind(table)
    .fetch_all(pool)
    .await?;

    let primary_key = if pk_columns.is_empty() {
        None
    } else {
        Some(PrimaryKeyInfo {
            name: "PRIMARY KEY".to_owned(),
            column_names: pk_columns.into_iter().map(|(c,)| c).collect(),
        })
    };

    // Foreign Keys
    let fks: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT
            kcu.column_name, 
            ccu.table_name AS foreign_table_name,
            ccu.column_name AS foreign_column_name,
            tc.constraint_name
        FROM 
            information_schema.table_constraints AS tc 
            JOIN information_schema.key_column_usage AS kcu
              ON tc.constraint_name = kcu.constraint_name
              AND tc.table_schema = kcu.table_schema
            JOIN information_schema.constraint_column_usage AS ccu
              ON ccu.constraint_name = tc.constraint_name
              AND ccu.table_schema = tc.table_schema
        WHERE tc.constraint_type = 'FOREIGN KEY' 
          AND tc.table_schema = $1 AND tc.table_name = $2",
    )
    .bind(schema)
    .bind(table)
    .fetch_all(pool)
    .await?;

    let foreign_keys = fks
        .into_iter()
        .map(|(col, ref_table, ref_col, name)| ForeignKeyInfo {
            name,
            column_names: vec![col],
            referenced_table: ref_table,
            referenced_columns: vec![ref_col],
        })
        .collect();

    // Indexes. Includes the primary-key index so users can see the
    // default btree backing the PK; pg_am.amname carries the access
    // method (btree/hash/gin/gist/brin/spgist).
    let idx_rows: Vec<(String, String, bool, bool, String)> = sqlx::query_as(
        "SELECT i.relname        AS index_name,
                a.attname        AS column_name,
                ix.indisunique   AS is_unique,
                ix.indisprimary  AS is_primary,
                am.amname        AS method
         FROM pg_class t
         JOIN pg_index ix    ON t.oid = ix.indrelid
         JOIN pg_class i     ON i.oid = ix.indexrelid
         JOIN pg_am am       ON am.oid = i.relam
         JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = ANY(ix.indkey)
         JOIN pg_namespace n ON n.oid = t.relnamespace
         WHERE n.nspname = $1 AND t.relname = $2
         ORDER BY ix.indisprimary DESC, ix.indisunique DESC, i.relname, a.attnum",
    )
    .bind(schema)
    .bind(table)
    .fetch_all(pool)
    .await?;

    // (cols, is_unique, is_primary, method) keyed by index name.
    let mut idx_map: BTreeMap<String, (Vec<String>, bool, bool, String)> = BTreeMap::new();
    for (idx_name, col_name, is_unique, is_primary, method) in idx_rows {
        idx_map
            .entry(idx_name)
            .or_insert_with(|| (Vec::new(), is_unique, is_primary, method))
            .0
            .push(col_name);
    }
    let indexes = idx_map
        .into_iter()
        .map(
            |(name, (column_names, is_unique, is_primary, method))| IndexInfo {
                name,
                column_names,
                is_unique,
                is_primary,
                method,
            },
        )
        .collect();

    // Constraints (CHECK + UNIQUE + EXCLUSION). We go through
    // pg_constraint instead of information_schema for two reasons:
    //   1. `pg_get_constraintdef` gives us the actual SQL clause
    //      (e.g. `CHECK ((amount > 0))`, `UNIQUE (email)`) rather
    //      than just the constraint *type*.
    //   2. We can filter out the synthetic NOT NULL constraints
    //      Postgres now records (e.g. `2200_16385_1_not_null`) —
    //      those are already surfaced in the Columns tab via the
    //      `is_nullable` flag and would otherwise spam this view.
    let constraint_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT con.conname, pg_get_constraintdef(con.oid)
         FROM pg_constraint con
         JOIN pg_class rel ON rel.oid = con.conrelid
         JOIN pg_namespace nsp ON nsp.oid = rel.relnamespace
         WHERE nsp.nspname = $1
           AND rel.relname = $2
           AND con.contype IN ('c', 'u', 'x')
           AND con.conname !~ '_not_null$'
         ORDER BY con.conname",
    )
    .bind(schema)
    .bind(table)
    .fetch_all(pool)
    .await?;

    let constraints = constraint_rows
        .into_iter()
        .map(|(name, definition)| ConstraintInfo { name, definition })
        .collect();

    Ok(TableInfo {
        name: table.to_owned(),
        schema: schema.to_owned(),
        columns: column_infos,
        indexes,
        primary_key,
        foreign_keys,
        constraints,
    })
}

pub async fn fetch_relationships(
    driver: DriverKind,
    url: &str,
    schema: &str,
) -> Result<Vec<RelationshipEdge>, DbError> {
    Pool::connect(driver, url)
        .await?
        .fetch_relationships(schema)
        .await
}

async fn execute_postgres(pool: &PgPool, statements: &[String]) -> Result<QueryOutput, DbError> {
    let mut output = Vec::with_capacity(statements.len());

    for statement in statements {
        if returns_rows(statement) {
            let rows = sqlx::query(statement).fetch_all(pool).await?;
            output.push(postgres_rows(statement, &rows));
        } else {
            let result = sqlx::query(statement).execute(pool).await?;
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

async fn execute_sqlite(pool: &SqlitePool, statements: &[String]) -> Result<QueryOutput, DbError> {
    let mut output = Vec::with_capacity(statements.len());

    for statement in statements {
        if returns_rows(statement) {
            let rows = sqlx::query(statement).fetch_all(pool).await?;
            output.push(sqlite_rows(statement, &rows));
        } else {
            let result = sqlx::query(statement).execute(pool).await?;
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
    use sqlx::types::chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
    use sqlx::types::{BigDecimal, JsonValue, Uuid};

    if row
        .try_get_raw(index)
        .is_ok_and(|value| ValueRef::is_null(&value))
    {
        return "NULL".to_owned();
    }

    // Try the most specific decoders first. sqlx is strict about Postgres
    // OIDs — `try_get::<String>` on a `numeric` or `timestamptz` returns
    // an error, so without these branches every NUMERIC / TIMESTAMP /
    // DATE / TIME / UUID / JSON cell falls back to `<type_name>` and
    // looks like a NULL or seed bug.
    row.try_get::<String, _>(index)
        .or_else(|_| row.try_get::<i64, _>(index).map(|v| v.to_string()))
        .or_else(|_| row.try_get::<i32, _>(index).map(|v| v.to_string()))
        .or_else(|_| row.try_get::<i16, _>(index).map(|v| v.to_string()))
        .or_else(|_| row.try_get::<f64, _>(index).map(|v| v.to_string()))
        .or_else(|_| row.try_get::<f32, _>(index).map(|v| v.to_string()))
        .or_else(|_| row.try_get::<bool, _>(index).map(|v| v.to_string()))
        .or_else(|_| row.try_get::<BigDecimal, _>(index).map(|v| v.to_string()))
        .or_else(|_| {
            row.try_get::<DateTime<Utc>, _>(index)
                .map(|v| v.to_rfc3339())
        })
        .or_else(|_| {
            row.try_get::<NaiveDateTime, _>(index)
                .map(|v| v.format("%Y-%m-%d %H:%M:%S%.f").to_string())
        })
        .or_else(|_| {
            row.try_get::<NaiveDate, _>(index)
                .map(|v| v.format("%Y-%m-%d").to_string())
        })
        .or_else(|_| {
            row.try_get::<NaiveTime, _>(index)
                .map(|v| v.format("%H:%M:%S%.f").to_string())
        })
        .or_else(|_| row.try_get::<Uuid, _>(index).map(|v| v.to_string()))
        .or_else(|_| row.try_get::<JsonValue, _>(index).map(|v| v.to_string()))
        .or_else(|_| {
            row.try_get::<Vec<u8>, _>(index)
                .map(|v| format!("0x{}", hex_encode(&v)))
        })
        .unwrap_or_else(|_| format!("<{}>", row.columns()[index].type_info().name()))
}

fn sqlite_cell(row: &SqliteRow, index: usize) -> String {
    use sqlx::types::chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};

    if row
        .try_get_raw(index)
        .is_ok_and(|value| ValueRef::is_null(&value))
    {
        return "NULL".to_owned();
    }

    // SQLite stores most values as TEXT, INTEGER, or REAL, but declared
    // TIMESTAMP / DATE columns may also decode as the chrono types
    // depending on storage. Try chrono after the simple primitives so
    // the canonical 'YYYY-MM-DD HH:MM:SS' text form takes priority.
    row.try_get::<String, _>(index)
        .or_else(|_| row.try_get::<i64, _>(index).map(|v| v.to_string()))
        .or_else(|_| row.try_get::<f64, _>(index).map(|v| v.to_string()))
        .or_else(|_| row.try_get::<bool, _>(index).map(|v| v.to_string()))
        .or_else(|_| {
            row.try_get::<DateTime<Utc>, _>(index)
                .map(|v| v.to_rfc3339())
        })
        .or_else(|_| {
            row.try_get::<NaiveDateTime, _>(index)
                .map(|v| v.format("%Y-%m-%d %H:%M:%S%.f").to_string())
        })
        .or_else(|_| {
            row.try_get::<NaiveDate, _>(index)
                .map(|v| v.format("%Y-%m-%d").to_string())
        })
        .or_else(|_| {
            row.try_get::<NaiveTime, _>(index)
                .map(|v| v.format("%H:%M:%S%.f").to_string())
        })
        .or_else(|_| {
            row.try_get::<Vec<u8>, _>(index)
                .map(|v| format!("0x{}", hex_encode(&v)))
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

async fn fetch_sqlite_relationships(
    pool: &SqlitePool,
    _schema: &str,
) -> Result<Vec<RelationshipEdge>, DbError> {
    let tables: Vec<(String,)> = sqlx::query_as(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .fetch_all(pool)
    .await?;

    let mut edges = Vec::new();
    for (table,) in tables {
        let fks: Vec<PragmaFkRow> =
            sqlx::query_as(&format!("PRAGMA foreign_key_list(\"{}\")", table))
                .fetch_all(pool)
                .await?;

        for (_, _, ref_table, from, to, _, _, _) in fks {
            edges.push(RelationshipEdge {
                from_table: table.clone(),
                from_columns: vec![from],
                to_table: ref_table,
                to_columns: vec![to],
            });
        }
    }

    Ok(edges)
}

async fn fetch_postgres_relationships(
    pool: &PgPool,
    schema: &str,
) -> Result<Vec<RelationshipEdge>, DbError> {
    let fks: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT
            tc.table_name, kcu.column_name, 
            ccu.table_name AS foreign_table_name,
            ccu.column_name AS foreign_column_name 
        FROM 
            information_schema.table_constraints AS tc 
            JOIN information_schema.key_column_usage AS kcu
              ON tc.constraint_name = kcu.constraint_name
              AND tc.table_schema = kcu.table_schema
            JOIN information_schema.constraint_column_usage AS ccu
              ON ccu.constraint_name = tc.constraint_name
              AND ccu.table_schema = tc.table_schema
        WHERE tc.constraint_type = 'FOREIGN KEY' AND tc.table_schema = $1",
    )
    .bind(schema)
    .fetch_all(pool)
    .await?;

    Ok(fks
        .into_iter()
        .map(|(table, col, ref_table, ref_col)| RelationshipEdge {
            from_table: table,
            from_columns: vec![col],
            to_table: ref_table,
            to_columns: vec![ref_col],
        })
        .collect())
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
