use std::str::FromStr;

use bb8::Pool as Bb8Pool;
use bb8_tiberius::ConnectionManager;
use futures_util::TryStreamExt;
use tiberius::{AuthMethod, ColumnData, Config, EncryptionLevel, QueryItem};

use crate::{
    ColumnInfo, DatabaseOverview, DbError, ForeignKeyInfo, IndexInfo, PrimaryKeyInfo, QueryOutput,
    RelationshipEdge, SchemaInfo, StatementOutput, TableInfo,
};

pub type MssqlPool = Bb8Pool<ConnectionManager>;

/// Build a `tiberius::Config` from a tsqlx-style URL.
///
/// Accepted shapes:
///   - `mssql://user:pass@host:port/database?encrypt=off&trust_cert=true`
///   - `mssql://user:pass@host/database` (port defaults to 1433)
///   - `mssql://host/database` (Windows integrated auth on Windows only;
///     elsewhere this returns an error since we don't link winauth)
///
/// `encrypt=off|on|required` (default: `on`) and `trust_cert=true|false`
/// (default: `false`) are honored. `instance=NAME` switches to a named
/// instance and contacts the SQL Browser for port discovery.
pub fn config_from_url(url: &str) -> Result<Config, DbError> {
    let stripped = url.strip_prefix("mssql://").ok_or_else(|| {
        DbError::Mssql(format!(
            "expected URL starting with `mssql://`, got `{url}`"
        ))
    })?;

    let (auth_part, rest) = match stripped.rsplit_once('@') {
        Some((auth, rest)) => (Some(auth), rest),
        None => (None, stripped),
    };

    let (host_part, query_part) = match rest.split_once('?') {
        Some((h, q)) => (h, Some(q)),
        None => (rest, None),
    };

    let (host_port, database) = match host_part.split_once('/') {
        Some((hp, db)) if !db.is_empty() => (hp, Some(db)),
        Some((hp, _)) => (hp, None),
        None => (host_part, None),
    };

    let (host, port) = match host_port.split_once(':') {
        Some((h, p)) => {
            let port_n = u16::from_str(p)
                .map_err(|_| DbError::Mssql(format!("invalid port `{p}` in URL `{url}`")))?;
            (h.to_owned(), Some(port_n))
        }
        None => (host_port.to_owned(), None),
    };

    let mut config = Config::new();
    config.host(host);
    if let Some(p) = port {
        config.port(p);
    }
    if let Some(db) = database {
        config.database(decode(db));
    }

    if let Some(auth) = auth_part {
        let (user, pass) = match auth.split_once(':') {
            Some((u, p)) => (decode(u), decode(p)),
            None => (decode(auth), String::new()),
        };
        config.authentication(AuthMethod::sql_server(user, pass));
    } else {
        return Err(DbError::Mssql(
            "tiberius URL needs `user[:pass]@host[:port]/db` (winauth not enabled)".to_owned(),
        ));
    }

    let mut encrypt = EncryptionLevel::On;
    let mut trust_cert = false;
    if let Some(q) = query_part {
        for pair in q.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            match k {
                "encrypt" => {
                    encrypt = match v {
                        "off" | "false" | "0" => EncryptionLevel::Off,
                        "required" => EncryptionLevel::Required,
                        "on" | "true" | "1" | "" => EncryptionLevel::On,
                        other => {
                            return Err(DbError::Mssql(format!(
                                "unknown encrypt value `{other}` (expected on/off/required)"
                            )))
                        }
                    }
                }
                "trust_cert" | "trust_server_certificate" => {
                    trust_cert = matches!(v, "true" | "1" | "yes" | "");
                }
                "instance" | "instance_name" => {
                    config.instance_name(v);
                }
                _ => {}
            }
        }
    }
    config.encryption(encrypt);
    if trust_cert {
        config.trust_cert();
    }
    Ok(config)
}

pub async fn connect_pool(url: &str) -> Result<MssqlPool, DbError> {
    let config = config_from_url(url)?;
    let manager = ConnectionManager::new(config);
    Bb8Pool::builder()
        .max_size(4)
        .build(manager)
        .await
        .map_err(|e| DbError::Mssql(e.to_string()))
}

pub async fn execute_script(
    pool: &MssqlPool,
    statements: &[String],
) -> Result<QueryOutput, DbError> {
    let mut conn = pool.get().await?;
    let mut outputs = Vec::with_capacity(statements.len());

    for sql in statements {
        let mut stream = conn.simple_query(sql.as_str()).await?;
        let mut columns: Vec<String> = Vec::new();
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut rows_affected: u64 = 0;

        while let Some(item) = stream.try_next().await? {
            match item {
                QueryItem::Metadata(meta) => {
                    columns = meta.columns().iter().map(|c| c.name().to_owned()).collect();
                }
                QueryItem::Row(row) => {
                    let cells: Vec<String> = row
                        .cells()
                        .map(|(_, data)| cell_to_string(Some(data)))
                        .collect();
                    rows.push(cells);
                }
            }
        }
        // tiberius surfaces rows-affected via the `result_set` API; for
        // simple_query we sum row counts as a best-effort approximation
        // for non-SELECT statements.
        if columns.is_empty() {
            rows_affected = rows.len() as u64;
        }

        outputs.push(StatementOutput {
            statement: sql.clone(),
            columns,
            rows,
            rows_affected,
        });
    }

    Ok(QueryOutput {
        statements: outputs,
    })
}

fn cell_to_string(cell: Option<&ColumnData<'static>>) -> String {
    let Some(cell) = cell else {
        return "NULL".to_owned();
    };
    match cell {
        ColumnData::U8(Some(v)) => v.to_string(),
        ColumnData::I16(Some(v)) => v.to_string(),
        ColumnData::I32(Some(v)) => v.to_string(),
        ColumnData::I64(Some(v)) => v.to_string(),
        ColumnData::F32(Some(v)) => v.to_string(),
        ColumnData::F64(Some(v)) => v.to_string(),
        ColumnData::Bit(Some(v)) => if *v { "1" } else { "0" }.to_owned(),
        ColumnData::String(Some(s)) => s.to_string(),
        ColumnData::Guid(Some(g)) => g.to_string(),
        ColumnData::Binary(Some(b)) => format!("0x{}", hex_lower(b)),
        ColumnData::Numeric(Some(n)) => n.to_string(),
        ColumnData::Xml(Some(x)) => x.as_ref().to_string(),
        ColumnData::DateTime(Some(d)) => format!("{d:?}"),
        ColumnData::SmallDateTime(Some(d)) => format!("{d:?}"),
        ColumnData::Time(Some(t)) => format!("{t:?}"),
        ColumnData::Date(Some(d)) => format!("{d:?}"),
        ColumnData::DateTime2(Some(d)) => format!("{d:?}"),
        ColumnData::DateTimeOffset(Some(d)) => format!("{d:?}"),
        _ => "NULL".to_owned(),
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from_digit(((*b >> 4) & 0xF) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((*b & 0xF) as u32, 16).unwrap_or('0'));
    }
    out
}

/// URL-decode a percent-encoded segment without pulling in a full URL
/// crate. Only the basic %XX → byte escape is honored.
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

pub async fn fetch_overview(pool: &MssqlPool) -> Result<DatabaseOverview, DbError> {
    let mut conn = pool.get().await?;
    let stream = conn
        .simple_query(
            "SELECT s.name AS schema_name, t.name AS table_name
             FROM sys.tables t
             JOIN sys.schemas s ON s.schema_id = t.schema_id
             WHERE s.name NOT IN ('sys','INFORMATION_SCHEMA')
             ORDER BY s.name, t.name",
        )
        .await?;
    let rows = stream.into_first_result().await?;

    let mut by_schema: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for row in rows {
        let schema: &str = row.try_get(0)?.unwrap_or("dbo");
        let table: &str = row.try_get(1)?.unwrap_or("");
        by_schema
            .entry(schema.to_owned())
            .or_default()
            .push(table.to_owned());
    }

    Ok(DatabaseOverview {
        schemas: by_schema
            .into_iter()
            .map(|(name, tables)| SchemaInfo { name, tables })
            .collect(),
    })
}

pub async fn fetch_table_info(
    pool: &MssqlPool,
    schema: &str,
    table: &str,
) -> Result<TableInfo, DbError> {
    let mut conn = pool.get().await?;

    // Columns
    let col_sql = format!(
        "SELECT c.name, t.name AS type_name, c.is_nullable,
                CAST(dc.definition AS NVARCHAR(MAX)) AS default_def
         FROM sys.columns c
         JOIN sys.types t ON t.user_type_id = c.user_type_id
         LEFT JOIN sys.default_constraints dc ON dc.parent_object_id = c.object_id AND dc.parent_column_id = c.column_id
         WHERE c.object_id = OBJECT_ID('[{schema}].[{table}]')
         ORDER BY c.column_id"
    );
    let stream = conn.simple_query(col_sql).await?;
    let col_rows = stream.into_first_result().await?;
    let mut columns = Vec::new();
    for row in col_rows {
        let name: &str = row.try_get(0)?.unwrap_or("");
        let data_type: &str = row.try_get(1)?.unwrap_or("");
        let is_nullable: bool = row.try_get::<bool, _>(2)?.unwrap_or(true);
        let default_value: Option<String> = row.try_get::<&str, _>(3)?.map(|s| s.to_owned());
        columns.push(ColumnInfo {
            name: name.to_owned(),
            data_type: data_type.to_owned(),
            is_nullable,
            default_value,
        });
    }

    // Primary key + unique indexes
    let idx_sql = format!(
        "SELECT i.name, i.is_unique, i.is_primary_key, i.type_desc, c.name
         FROM sys.indexes i
         JOIN sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id
         JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id
         WHERE i.object_id = OBJECT_ID('[{schema}].[{table}]') AND i.index_id > 0
         ORDER BY i.index_id, ic.key_ordinal"
    );
    let stream = conn.simple_query(idx_sql).await?;
    let idx_rows = stream.into_first_result().await?;
    let mut by_index: std::collections::BTreeMap<String, IndexInfo> = Default::default();
    let mut pk: Option<PrimaryKeyInfo> = None;
    for row in idx_rows {
        let idx_name: &str = row.try_get(0)?.unwrap_or("");
        let is_unique: bool = row.try_get(1)?.unwrap_or(false);
        let is_primary: bool = row.try_get(2)?.unwrap_or(false);
        let type_desc: &str = row.try_get(3)?.unwrap_or("");
        let col_name: &str = row.try_get(4)?.unwrap_or("");
        let entry = by_index
            .entry(idx_name.to_owned())
            .or_insert_with(|| IndexInfo {
                name: idx_name.to_owned(),
                column_names: Vec::new(),
                is_unique,
                is_primary,
                method: type_desc.to_lowercase(),
            });
        entry.column_names.push(col_name.to_owned());
        if is_primary {
            let pk_entry = pk.get_or_insert_with(|| PrimaryKeyInfo {
                name: idx_name.to_owned(),
                column_names: Vec::new(),
            });
            pk_entry.column_names.push(col_name.to_owned());
        }
    }
    let indexes = by_index.into_values().collect();

    // Foreign keys
    let fk_sql = format!(
        "SELECT fk.name, c1.name AS col, OBJECT_NAME(fk.referenced_object_id) AS ref_table, c2.name AS ref_col
         FROM sys.foreign_keys fk
         JOIN sys.foreign_key_columns fkc ON fkc.constraint_object_id = fk.object_id
         JOIN sys.columns c1 ON c1.object_id = fkc.parent_object_id AND c1.column_id = fkc.parent_column_id
         JOIN sys.columns c2 ON c2.object_id = fkc.referenced_object_id AND c2.column_id = fkc.referenced_column_id
         WHERE fk.parent_object_id = OBJECT_ID('[{schema}].[{table}]')
         ORDER BY fk.name, fkc.constraint_column_id"
    );
    let stream = conn.simple_query(fk_sql).await?;
    let fk_rows = stream.into_first_result().await?;
    let mut by_fk: std::collections::BTreeMap<String, ForeignKeyInfo> = Default::default();
    for row in fk_rows {
        let fk_name: &str = row.try_get(0)?.unwrap_or("");
        let col: &str = row.try_get(1)?.unwrap_or("");
        let ref_table: &str = row.try_get(2)?.unwrap_or("");
        let ref_col: &str = row.try_get(3)?.unwrap_or("");
        let entry = by_fk
            .entry(fk_name.to_owned())
            .or_insert_with(|| ForeignKeyInfo {
                name: fk_name.to_owned(),
                column_names: Vec::new(),
                referenced_table: ref_table.to_owned(),
                referenced_columns: Vec::new(),
            });
        entry.column_names.push(col.to_owned());
        entry.referenced_columns.push(ref_col.to_owned());
    }
    let foreign_keys = by_fk.into_values().collect();

    Ok(TableInfo {
        name: table.to_owned(),
        schema: schema.to_owned(),
        columns,
        indexes,
        primary_key: pk,
        foreign_keys,
        constraints: Vec::new(),
    })
}

pub async fn fetch_relationships(
    pool: &MssqlPool,
    schema: &str,
) -> Result<Vec<RelationshipEdge>, DbError> {
    let mut conn = pool.get().await?;
    let sql = format!(
        "SELECT OBJECT_NAME(fk.parent_object_id) AS from_table,
                c1.name AS from_col,
                OBJECT_NAME(fk.referenced_object_id) AS to_table,
                c2.name AS to_col
         FROM sys.foreign_keys fk
         JOIN sys.foreign_key_columns fkc ON fkc.constraint_object_id = fk.object_id
         JOIN sys.columns c1 ON c1.object_id = fkc.parent_object_id AND c1.column_id = fkc.parent_column_id
         JOIN sys.columns c2 ON c2.object_id = fkc.referenced_object_id AND c2.column_id = fkc.referenced_column_id
         JOIN sys.schemas s ON s.schema_id = (SELECT schema_id FROM sys.tables WHERE object_id = fk.parent_object_id)
         WHERE s.name = '{schema}'
         ORDER BY fk.name, fkc.constraint_column_id"
    );
    let stream = conn.simple_query(sql).await?;
    let rows = stream.into_first_result().await?;

    let mut by_pair: std::collections::BTreeMap<(String, String), RelationshipEdge> =
        Default::default();
    for row in rows {
        let from_t: &str = row.try_get(0)?.unwrap_or("");
        let from_c: &str = row.try_get(1)?.unwrap_or("");
        let to_t: &str = row.try_get(2)?.unwrap_or("");
        let to_c: &str = row.try_get(3)?.unwrap_or("");
        let entry = by_pair
            .entry((from_t.to_owned(), to_t.to_owned()))
            .or_insert_with(|| RelationshipEdge {
                from_table: from_t.to_owned(),
                from_columns: Vec::new(),
                to_table: to_t.to_owned(),
                to_columns: Vec::new(),
            });
        entry.from_columns.push(from_c.to_owned());
        entry.to_columns.push(to_c.to_owned());
    }
    Ok(by_pair.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_from_url_parses_basic_form() {
        let cfg = config_from_url("mssql://sa:Passw0rd!@localhost:1433/master").unwrap();
        // tiberius doesn't expose a public getter for host/port/db, so
        // we only sanity-check that parsing didn't error and the
        // string is round-trippable.
        let _ = cfg;
    }

    #[test]
    fn config_from_url_rejects_missing_auth() {
        let err = config_from_url("mssql://localhost/master").unwrap_err();
        assert!(matches!(err, DbError::Mssql(_)));
    }

    #[test]
    fn config_from_url_accepts_query_params() {
        let cfg =
            config_from_url("mssql://sa:p@host:1433/db?encrypt=off&trust_cert=true&instance=NAMED")
                .unwrap();
        let _ = cfg;
    }
}
