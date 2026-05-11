use std::sync::atomic::{AtomicU64, Ordering};

use tsqlx_core::DriverKind;
use tsqlx_db::{
    execute_script, fetch_overview, fetch_records, fetch_relationships, fetch_table_info,
};
use tsqlx_sql::SqlDocument;

static SCHEMA_SEQ: AtomicU64 = AtomicU64::new(0);

fn mssql_url() -> String {
    std::env::var("TSQLX_TEST_MSSQL_URL").expect("TSQLX_TEST_MSSQL_URL is set")
}

/// Generate a unique schema name per test run so parallel invocations
/// don't collide on the shared `dbo` schema.
fn unique_schema(prefix: &str) -> String {
    let id = SCHEMA_SEQ.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    format!("tsqlx_test_{prefix}_{pid}_{id}")
}

async fn drop_schema(url: &str, schema: &str) {
    // T-SQL has no `DROP SCHEMA … CASCADE`. Tables are dropped first,
    // then the schema. Best-effort: we ignore failures because the
    // schema may not exist on the first run.
    let drop = SqlDocument::new(format!(
        "DECLARE @sql NVARCHAR(MAX) = N'';
         SELECT @sql = @sql + 'DROP TABLE [{schema}].[' + name + '];' FROM sys.tables WHERE schema_id = SCHEMA_ID('{schema}');
         IF @sql <> '' EXEC sp_executesql @sql;
         IF SCHEMA_ID('{schema}') IS NOT NULL EXEC('DROP SCHEMA [{schema}]');"
    ));
    let _ = execute_script(DriverKind::Mssql, url, &drop).await;
}

#[tokio::test]
#[ignore = "requires TSQLX_TEST_MSSQL_URL"]
async fn executes_mssql_script() {
    let url = mssql_url();
    let schema = unique_schema("exec");
    drop_schema(&url, &schema).await;

    let document = SqlDocument::new(format!(
        "CREATE SCHEMA [{schema}];
         GO
         CREATE TABLE [{schema}].users (id INT IDENTITY(1,1) PRIMARY KEY, name NVARCHAR(64) NOT NULL);
         INSERT INTO [{schema}].users (name) VALUES ('ada'), ('grace');
         SELECT id, name FROM [{schema}].users ORDER BY id;"
    ));

    let output = execute_script(DriverKind::Mssql, &url, &document)
        .await
        .expect("mssql script executes");

    // T-SQL `GO` carved the script into 2 batches; the second batch
    // emits a SELECT result we can inspect.
    let last = output.statements.last().expect("at least one batch");
    assert_eq!(last.columns, ["id", "name"]);
    assert_eq!(last.rows.len(), 2);
    assert_eq!(last.rows[0][1], "ada");
    assert_eq!(last.rows[1][1], "grace");

    drop_schema(&url, &schema).await;
}

#[tokio::test]
#[ignore = "requires TSQLX_TEST_MSSQL_URL"]
async fn mssql_overview_lists_tables() {
    let url = mssql_url();
    let schema = unique_schema("ov");
    drop_schema(&url, &schema).await;

    let setup = SqlDocument::new(format!(
        "CREATE SCHEMA [{schema}];
         GO
         CREATE TABLE [{schema}].users (id INT PRIMARY KEY, name NVARCHAR(64));
         CREATE TABLE [{schema}].orders (id INT PRIMARY KEY, user_id INT REFERENCES [{schema}].users(id));"
    ));
    execute_script(DriverKind::Mssql, &url, &setup)
        .await
        .expect("setup runs");

    let ov = fetch_overview(DriverKind::Mssql, &url)
        .await
        .expect("overview fetches");

    let our = ov
        .schemas
        .into_iter()
        .find(|s| s.name == schema)
        .expect("schema present");
    assert!(our.tables.iter().any(|t| t == "users"));
    assert!(our.tables.iter().any(|t| t == "orders"));

    drop_schema(&url, &schema).await;
}

#[tokio::test]
#[ignore = "requires TSQLX_TEST_MSSQL_URL"]
async fn mssql_table_info_reports_columns_pk_and_fk() {
    let url = mssql_url();
    let schema = unique_schema("ti");
    drop_schema(&url, &schema).await;

    let setup = SqlDocument::new(format!(
        "CREATE SCHEMA [{schema}];
         GO
         CREATE TABLE [{schema}].users (
            id INT IDENTITY(1,1) PRIMARY KEY,
            name NVARCHAR(64) NOT NULL,
            email NVARCHAR(128) NULL
         );
         CREATE TABLE [{schema}].orders (
            id INT IDENTITY(1,1) PRIMARY KEY,
            user_id INT NOT NULL,
            CONSTRAINT FK_orders_user FOREIGN KEY (user_id) REFERENCES [{schema}].users(id)
         );"
    ));
    execute_script(DriverKind::Mssql, &url, &setup)
        .await
        .expect("setup runs");

    let info = fetch_table_info(DriverKind::Mssql, &url, &schema, "users")
        .await
        .expect("table_info fetches");
    assert_eq!(info.name, "users");
    assert!(info
        .columns
        .iter()
        .any(|c| c.name == "name" && !c.is_nullable));
    assert!(info
        .columns
        .iter()
        .any(|c| c.name == "email" && c.is_nullable));
    let pk = info.primary_key.expect("pk present");
    assert_eq!(pk.column_names, ["id"]);

    let orders = fetch_table_info(DriverKind::Mssql, &url, &schema, "orders")
        .await
        .expect("orders table_info fetches");
    let fk = orders
        .foreign_keys
        .iter()
        .find(|fk| fk.name == "FK_orders_user")
        .expect("fk present");
    assert_eq!(fk.column_names, ["user_id"]);
    assert_eq!(fk.referenced_table, "users");
    assert_eq!(fk.referenced_columns, ["id"]);

    drop_schema(&url, &schema).await;
}

#[tokio::test]
#[ignore = "requires TSQLX_TEST_MSSQL_URL"]
async fn mssql_relationships_returns_edges() {
    let url = mssql_url();
    let schema = unique_schema("rel");
    drop_schema(&url, &schema).await;

    let setup = SqlDocument::new(format!(
        "CREATE SCHEMA [{schema}];
         GO
         CREATE TABLE [{schema}].users (id INT PRIMARY KEY);
         CREATE TABLE [{schema}].orders (
            id INT PRIMARY KEY,
            user_id INT NOT NULL,
            CONSTRAINT FK_o_u FOREIGN KEY (user_id) REFERENCES [{schema}].users(id)
         );"
    ));
    execute_script(DriverKind::Mssql, &url, &setup)
        .await
        .expect("setup runs");

    let edges = fetch_relationships(DriverKind::Mssql, &url, &schema)
        .await
        .expect("relationships fetch");
    let edge = edges
        .iter()
        .find(|e| e.from_table == "orders" && e.to_table == "users")
        .expect("orders→users edge present");
    assert_eq!(edge.from_columns, ["user_id"]);
    assert_eq!(edge.to_columns, ["id"]);

    drop_schema(&url, &schema).await;
}

#[tokio::test]
#[ignore = "requires TSQLX_TEST_MSSQL_URL"]
async fn mssql_fetch_records_pagination() {
    let url = mssql_url();
    let schema = unique_schema("rec");
    drop_schema(&url, &schema).await;

    let setup = SqlDocument::new(format!(
        "CREATE SCHEMA [{schema}];
         GO
         CREATE TABLE [{schema}].nums (n INT PRIMARY KEY);
         INSERT INTO [{schema}].nums (n) VALUES (1),(2),(3),(4),(5),(6);"
    ));
    execute_script(DriverKind::Mssql, &url, &setup)
        .await
        .expect("setup runs");

    let page0 = fetch_records(DriverKind::Mssql, &url, &schema, "nums", 3, 0)
        .await
        .expect("page 0");
    assert_eq!(page0.rows.len(), 3);

    let page1 = fetch_records(DriverKind::Mssql, &url, &schema, "nums", 3, 3)
        .await
        .expect("page 1");
    assert_eq!(page1.rows.len(), 3);

    drop_schema(&url, &schema).await;
}
