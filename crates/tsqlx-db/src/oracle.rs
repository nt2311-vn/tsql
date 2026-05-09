//! Oracle Database driver.
//!
//! The upstream `oracle` crate wraps Oracle Instant Client (OCI) — a
//! synchronous C library. We expose a `Pool` variant that runs all I/O
//! inside `tokio::task::spawn_blocking` so the rest of the codebase
//! can keep its async surface.
//!
//! Building this module requires `--features oracle` and a working
//! Oracle Instant Client install on the runtime library path
//! (`LD_LIBRARY_PATH` on Linux, `DYLD_LIBRARY_PATH` on macOS,
//! `PATH` on Windows).

use std::sync::Arc;

use oracle::pool::{Pool as OraPool, PoolBuilder};
use oracle::sql_type::{OracleType, ToSql};
use oracle::Connection;

use crate::{
    ColumnInfo, DatabaseOverview, DbError, ForeignKeyInfo, PrimaryKeyInfo, QueryOutput,
    RelationshipEdge, SchemaInfo, StatementOutput, TableInfo,
};

/// Cheap-to-clone handle to an Oracle connection pool.
#[derive(Clone)]
pub struct OraclePool(Arc<OraPool>);

impl std::fmt::Debug for OraclePool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OraclePool").finish_non_exhaustive()
    }
}

impl From<oracle::Error> for DbError {
    fn from(value: oracle::Error) -> Self {
        Self::Oracle(value.to_string())
    }
}

/// Decompose `oracle://user:pass@host:port/service_name` into the
/// `(username, password, connect_string)` triple that the `oracle`
/// crate's `PoolBuilder::new` expects.
pub fn parse_url(url: &str) -> Result<(String, String, String), DbError> {
    let stripped = url
        .strip_prefix("oracle://")
        .ok_or_else(|| DbError::Oracle(format!("expected `oracle://` prefix, got `{url}`")))?;

    let (auth, host_path) = stripped
        .rsplit_once('@')
        .ok_or_else(|| DbError::Oracle("Oracle URL needs `user:pass@host:port/service`".into()))?;

    let (user, pass) = match auth.split_once(':') {
        Some((u, p)) => (decode(u), decode(p)),
        None => (decode(auth), String::new()),
    };

    if host_path.is_empty() {
        return Err(DbError::Oracle(
            "Oracle URL is missing the host:port/service part".into(),
        ));
    }

    Ok((user, pass, host_path.to_owned()))
}

pub async fn connect_pool(url: &str) -> Result<OraclePool, DbError> {
    let (user, pass, conn_str) = parse_url(url)?;
    let pool = tokio::task::spawn_blocking(move || {
        PoolBuilder::new(user, pass, conn_str)
            .max_connections(4)
            .build()
    })
    .await
    .map_err(|e| DbError::Oracle(format!("spawn_blocking: {e}")))??;
    Ok(OraclePool(Arc::new(pool)))
}

impl OraclePool {
    fn inner(&self) -> Arc<OraPool> {
        self.0.clone()
    }
}

pub async fn execute_script(
    pool: &OraclePool,
    statements: &[String],
) -> Result<QueryOutput, DbError> {
    let pool = pool.inner();
    let stmts = statements.to_vec();
    let outputs = tokio::task::spawn_blocking(move || -> Result<Vec<StatementOutput>, DbError> {
        let conn = pool.get()?;
        let mut out = Vec::with_capacity(stmts.len());
        for sql in stmts {
            let trimmed = sql.trim();
            // Oracle's parser rejects a trailing `;` on plain DML/DDL
            // (the SQL*Plus terminator). PL/SQL blocks already end in
            // `END;` and need that to remain.
            let to_run = if looks_like_plsql(trimmed) {
                trimmed.to_owned()
            } else {
                trimmed.trim_end_matches(';').to_owned()
            };
            out.push(run_one(&conn, &to_run)?);
        }
        Ok(out)
    })
    .await
    .map_err(|e| DbError::Oracle(format!("spawn_blocking: {e}")))??;

    Ok(QueryOutput {
        statements: outputs,
    })
}

fn run_one(conn: &Connection, sql: &str) -> Result<StatementOutput, DbError> {
    let no_params: &[&dyn ToSql] = &[];
    let lower = sql.trim_start().to_ascii_lowercase();
    if lower.starts_with("select") || lower.starts_with("with") {
        let rs = conn.query(sql, no_params)?;
        let columns: Vec<String> = rs
            .column_info()
            .iter()
            .map(|c| c.name().to_owned())
            .collect();
        let mut rows = Vec::new();
        for row in rs {
            let row = row?;
            let mut cells = Vec::with_capacity(columns.len());
            for i in 0..columns.len() {
                cells.push(cell_to_string(&row, i));
            }
            rows.push(cells);
        }
        let row_count = rows.len() as u64;
        Ok(StatementOutput {
            statement: sql.to_owned(),
            columns,
            rows,
            rows_affected: row_count,
        })
    } else {
        let stmt = conn.execute(sql, no_params)?;
        let _ = conn.commit();
        let affected = stmt.row_count().unwrap_or(0);
        Ok(StatementOutput {
            statement: sql.to_owned(),
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected: affected,
        })
    }
}

fn looks_like_plsql(sql: &str) -> bool {
    let lower = sql.trim_start().to_ascii_lowercase();
    lower.starts_with("begin")
        || lower.starts_with("declare")
        || lower.starts_with("create or replace procedure")
        || lower.starts_with("create or replace function")
        || lower.starts_with("create or replace package")
        || lower.starts_with("create or replace trigger")
        || lower.starts_with("create procedure")
        || lower.starts_with("create function")
        || lower.starts_with("create package")
        || lower.starts_with("create trigger")
}

fn cell_to_string(row: &oracle::Row, idx: usize) -> String {
    // Generic dynamic-typed escape hatch: ask for `Option<String>`.
    // The crate's built-in conversion handles every scalar we care
    // about (NUMBER / VARCHAR2 / DATE / TIMESTAMP / RAW). Errors fall
    // back to a typed sentinel so a single un-decodable cell can't
    // poison the whole row.
    match row.get::<usize, Option<String>>(idx) {
        Ok(Some(s)) => s,
        Ok(None) => "NULL".to_owned(),
        Err(_) => match row.column_info().get(idx).map(|c| c.oracle_type()) {
            Some(OracleType::Raw(_)) | Some(OracleType::BLOB) => "<binary>".to_owned(),
            _ => "NULL".to_owned(),
        },
    }
}

pub async fn fetch_overview(pool: &OraclePool) -> Result<DatabaseOverview, DbError> {
    let pool = pool.inner();
    tokio::task::spawn_blocking(move || -> Result<DatabaseOverview, DbError> {
        let conn = pool.get()?;
        let no_params: &[&dyn ToSql] = &[];
        let rs = conn.query_as::<(String, String)>(
            "SELECT owner, table_name FROM all_tables \
             WHERE owner NOT IN ('SYS','SYSTEM','OUTLN','XDB','MDSYS','CTXSYS','ORDSYS', \
                                 'WMSYS','APPQOSSYS','DBSNMP','GSMADMIN_INTERNAL','LBACSYS', \
                                 'OJVMSYS','OLAPSYS','ORDDATA','ORDPLUGINS','SI_INFORMTN_SCHEMA', \
                                 'AUDSYS','DVSYS','REMOTE_SCHEDULER_AGENT') \
             ORDER BY owner, table_name",
            no_params,
        )?;
        let mut by_schema: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for row in rs {
            let (owner, table) = row?;
            by_schema.entry(owner).or_default().push(table);
        }
        Ok(DatabaseOverview {
            schemas: by_schema
                .into_iter()
                .map(|(name, tables)| SchemaInfo { name, tables })
                .collect(),
        })
    })
    .await
    .map_err(|e| DbError::Oracle(format!("spawn_blocking: {e}")))?
}

pub async fn fetch_table_info(
    pool: &OraclePool,
    schema: &str,
    table: &str,
) -> Result<TableInfo, DbError> {
    let pool = pool.inner();
    let owner = schema.to_owned();
    let table_name = table.to_owned();

    tokio::task::spawn_blocking(move || -> Result<TableInfo, DbError> {
        let conn = pool.get()?;

        let columns = {
            let rs = conn.query_as::<(String, String, String, Option<String>)>(
                "SELECT column_name, data_type, nullable, data_default \
                 FROM all_tab_columns \
                 WHERE owner = :1 AND table_name = :2 \
                 ORDER BY column_id",
                &[&owner, &table_name],
            )?;
            let mut cols = Vec::new();
            for row in rs {
                let (name, ty, nullable, default) = row?;
                cols.push(ColumnInfo {
                    name,
                    data_type: ty,
                    is_nullable: nullable == "Y",
                    default_value: default,
                });
            }
            cols
        };

        // Pull every PK + FK row for the table, then resolve the
        // referenced table/columns by chasing each FK's r_constraint
        // back to its ALL_CONS_COLUMNS rows.
        type ConRow = (String, String, String, Option<String>, Option<String>);
        let rs = conn.query_as::<ConRow>(
            "SELECT c.constraint_name, c.constraint_type, cc.column_name, \
                    c.r_constraint_name, c.r_owner \
             FROM all_constraints c \
             JOIN all_cons_columns cc \
               ON cc.owner = c.owner AND cc.constraint_name = c.constraint_name \
             WHERE c.owner = :1 AND c.table_name = :2 \
               AND c.constraint_type IN ('P', 'R') \
             ORDER BY c.constraint_name, cc.position",
            &[&owner, &table_name],
        )?;
        let mut pk: Option<PrimaryKeyInfo> = None;
        let mut fks: std::collections::BTreeMap<String, ForeignKeyInfo> = Default::default();
        let mut r_lookup: std::collections::BTreeMap<String, (String, Option<String>)> =
            Default::default();
        for row in rs {
            let (cname, ctype, col, r_cname, r_owner) = row?;
            match ctype.as_str() {
                "P" => {
                    pk.get_or_insert(PrimaryKeyInfo {
                        name: cname.clone(),
                        column_names: Vec::new(),
                    })
                    .column_names
                    .push(col);
                }
                "R" => {
                    if let Some(rc) = r_cname.clone() {
                        r_lookup.insert(cname.clone(), (rc, r_owner));
                    }
                    fks.entry(cname.clone())
                        .or_insert_with(|| ForeignKeyInfo {
                            name: cname.clone(),
                            column_names: Vec::new(),
                            referenced_table: String::new(),
                            referenced_columns: Vec::new(),
                        })
                        .column_names
                        .push(col);
                }
                _ => {}
            }
        }

        for (fk_name, fk) in fks.iter_mut() {
            if let Some((r_cname, r_owner_opt)) = r_lookup.get(fk_name) {
                let r_owner = r_owner_opt.clone().unwrap_or_else(|| owner.clone());
                let rs = conn.query_as::<(String, String)>(
                    "SELECT c.table_name, cc.column_name \
                     FROM all_constraints c \
                     JOIN all_cons_columns cc \
                       ON cc.owner = c.owner AND cc.constraint_name = c.constraint_name \
                     WHERE c.owner = :1 AND c.constraint_name = :2 \
                     ORDER BY cc.position",
                    &[&r_owner, r_cname],
                )?;
                for row in rs {
                    let (t, c) = row?;
                    fk.referenced_table = t;
                    fk.referenced_columns.push(c);
                }
            }
        }

        Ok(TableInfo {
            name: table_name,
            schema: owner,
            columns,
            indexes: Vec::new(),
            primary_key: pk,
            foreign_keys: fks.into_values().collect(),
            constraints: Vec::new(),
        })
    })
    .await
    .map_err(|e| DbError::Oracle(format!("spawn_blocking: {e}")))?
}

pub async fn fetch_relationships(
    pool: &OraclePool,
    schema: &str,
) -> Result<Vec<RelationshipEdge>, DbError> {
    let pool = pool.inner();
    let owner = schema.to_owned();

    tokio::task::spawn_blocking(move || -> Result<Vec<RelationshipEdge>, DbError> {
        let conn = pool.get()?;
        let rs = conn.query_as::<(String, String, String, String)>(
            "SELECT c.table_name AS from_table, cc.column_name AS from_col, \
                    rc.table_name AS to_table, rcc.column_name AS to_col \
             FROM all_constraints c \
             JOIN all_cons_columns cc \
               ON cc.owner = c.owner AND cc.constraint_name = c.constraint_name \
             JOIN all_constraints rc \
               ON rc.owner = NVL(c.r_owner, c.owner) AND rc.constraint_name = c.r_constraint_name \
             JOIN all_cons_columns rcc \
               ON rcc.owner = rc.owner AND rcc.constraint_name = rc.constraint_name \
                  AND rcc.position = cc.position \
             WHERE c.owner = :1 AND c.constraint_type = 'R' \
             ORDER BY c.constraint_name, cc.position",
            &[&owner],
        )?;
        let mut by_pair: std::collections::BTreeMap<(String, String), RelationshipEdge> =
            Default::default();
        for row in rs {
            let (from_t, from_c, to_t, to_c) = row?;
            let entry = by_pair
                .entry((from_t.clone(), to_t.clone()))
                .or_insert_with(|| RelationshipEdge {
                    from_table: from_t,
                    from_columns: Vec::new(),
                    to_table: to_t,
                    to_columns: Vec::new(),
                });
            entry.from_columns.push(from_c);
            entry.to_columns.push(to_c);
        }
        Ok(by_pair.into_values().collect())
    })
    .await
    .map_err(|e| DbError::Oracle(format!("spawn_blocking: {e}")))?
}

fn decode(input: &str) -> String {
    let mut bytes = Vec::with_capacity(input.len());
    let raw = input.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            if let (Some(hi), Some(lo)) = (
                (raw[i + 1] as char).to_digit(16),
                (raw[i + 2] as char).to_digit(16),
            ) {
                bytes.push(((hi << 4) | lo) as u8);
                i += 3;
                continue;
            }
        }
        bytes.push(raw[i]);
        i += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_url_basic() {
        let (u, p, c) = parse_url("oracle://hr:hr@localhost:1521/FREEPDB1").unwrap();
        assert_eq!(u, "hr");
        assert_eq!(p, "hr");
        assert_eq!(c, "localhost:1521/FREEPDB1");
    }

    #[test]
    fn parse_url_rejects_missing_host() {
        let err = parse_url("oracle://hr:hr@").unwrap_err();
        assert!(matches!(err, DbError::Oracle(_)));
    }

    #[test]
    fn parse_url_handles_no_password() {
        let (u, p, c) = parse_url("oracle://hr@host:1521/svc").unwrap();
        assert_eq!(u, "hr");
        assert_eq!(p, "");
        assert_eq!(c, "host:1521/svc");
    }

    #[test]
    fn looks_like_plsql_recognises_blocks() {
        assert!(looks_like_plsql("BEGIN NULL; END;"));
        assert!(looks_like_plsql("DECLARE x int; BEGIN END;"));
        assert!(looks_like_plsql(
            "create or replace procedure p as begin null; end;"
        ));
        assert!(!looks_like_plsql("SELECT 1 FROM dual"));
        assert!(!looks_like_plsql("CREATE TABLE t (id INT)"));
    }
}
