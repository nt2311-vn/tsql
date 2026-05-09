use std::sync::atomic::{AtomicU64, Ordering};

use tsqlx_core::DriverKind;
use tsqlx_db::{
    execute_script, fetch_overview, fetch_records, fetch_relationships, fetch_table_info,
};
use tsqlx_sql::SqlDocument;

static SCHEMA_SEQ: AtomicU64 = AtomicU64::new(0);

fn pg_url() -> String {
    std::env::var("TSQLX_TEST_POSTGRES_URL").expect("TSQLX_TEST_POSTGRES_URL is set")
}

/// Each test creates and tears down its own dedicated schema so parallel runs
/// don't collide on the shared `public` schema.
fn unique_schema(prefix: &str) -> String {
    let id = SCHEMA_SEQ.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    format!("tsqlx_test_{prefix}_{pid}_{id}")
}

async fn drop_schema(url: &str, schema: &str) {
    let drop = SqlDocument::new(format!("drop schema if exists {schema} cascade"));
    let _ = execute_script(DriverKind::Postgres, url, &drop).await;
}

#[tokio::test]
#[ignore = "requires TSQLX_TEST_POSTGRES_URL"]
async fn executes_postgres_script() {
    let url = pg_url();
    let document = SqlDocument::new(
        r#"
        drop table if exists tsqlx_test_users;
        create table tsqlx_test_users(id serial primary key, name text not null);
        insert into tsqlx_test_users(name) values ('ada'), ('grace');
        select id, name from tsqlx_test_users order by id;
        "#,
    );

    let output = execute_script(DriverKind::Postgres, &url, &document)
        .await
        .expect("postgres script executes");

    assert_eq!(output.statements.len(), 4);
    assert_eq!(output.statements[3].columns, ["id", "name"]);
    assert_eq!(output.statements[3].rows[0], ["1", "ada"]);
    assert_eq!(output.statements[3].rows[1], ["2", "grace"]);
}

#[tokio::test]
#[ignore = "requires TSQLX_TEST_POSTGRES_URL"]
async fn postgres_overview_lists_tables_and_schemas() {
    let url = pg_url();
    let schema = unique_schema("ov");
    drop_schema(&url, &schema).await;

    let setup = SqlDocument::new(format!(
        "create schema {schema}; \
         create table {schema}.users(id serial primary key, name text not null); \
         create table {schema}.orders(id serial primary key, user_id integer references {schema}.users(id));"
    ));
    execute_script(DriverKind::Postgres, &url, &setup)
        .await
        .expect("setup");

    let ov = fetch_overview(DriverKind::Postgres, &url)
        .await
        .expect("overview");

    drop_schema(&url, &schema).await;

    let target = ov
        .schemas
        .iter()
        .find(|s| s.name == schema)
        .unwrap_or_else(|| panic!("schema {schema} present in overview"));
    assert!(
        target.tables.contains(&"users".to_owned()),
        "users in overview: {:?}",
        target.tables
    );
    assert!(
        target.tables.contains(&"orders".to_owned()),
        "orders in overview: {:?}",
        target.tables
    );
}

#[tokio::test]
#[ignore = "requires TSQLX_TEST_POSTGRES_URL"]
async fn postgres_table_info_columns_and_pk() {
    let url = pg_url();
    let schema = unique_schema("cols");
    drop_schema(&url, &schema).await;

    let setup = SqlDocument::new(format!(
        "create schema {schema}; \
         create table {schema}.users(id serial primary key, name text not null, email text);"
    ));
    execute_script(DriverKind::Postgres, &url, &setup)
        .await
        .expect("setup");

    let info = fetch_table_info(DriverKind::Postgres, &url, &schema, "users")
        .await
        .expect("table info");

    drop_schema(&url, &schema).await;

    assert_eq!(info.name, "users");
    assert_eq!(info.schema, schema);
    assert_eq!(info.columns.len(), 3);
    let by_name = |n: &str| {
        info.columns
            .iter()
            .find(|c| c.name == n)
            .unwrap_or_else(|| panic!("column {n} present"))
    };
    assert!(!by_name("id").is_nullable, "id NOT NULL");
    assert!(!by_name("name").is_nullable, "name NOT NULL");
    assert!(by_name("email").is_nullable, "email nullable");

    let pk = info.primary_key.expect("pk exists");
    assert!(pk.column_names.contains(&"id".to_owned()));
}

/// Regression test for the `FROM ,` regression that landed via the
/// `feat/xdg-config-connection-picker` branch: the FK metadata query was
/// silently broken because no integration test exercised the metadata path.
#[tokio::test]
#[ignore = "requires TSQLX_TEST_POSTGRES_URL"]
async fn postgres_table_info_foreign_keys() {
    let url = pg_url();
    let schema = unique_schema("fk");
    drop_schema(&url, &schema).await;

    let setup = SqlDocument::new(format!(
        "create schema {schema}; \
         create table {schema}.users(id serial primary key, name text); \
         create table {schema}.orders(id serial primary key, user_id integer references {schema}.users(id));"
    ));
    execute_script(DriverKind::Postgres, &url, &setup)
        .await
        .expect("setup");

    let info = fetch_table_info(DriverKind::Postgres, &url, &schema, "orders")
        .await
        .expect("table info");

    drop_schema(&url, &schema).await;

    assert_eq!(
        info.foreign_keys.len(),
        1,
        "exactly one FK on orders, got {:?}",
        info.foreign_keys
    );
    assert_eq!(info.foreign_keys[0].referenced_table, "users");
    assert!(info.foreign_keys[0]
        .column_names
        .contains(&"user_id".to_owned()));
}

#[tokio::test]
#[ignore = "requires TSQLX_TEST_POSTGRES_URL"]
async fn postgres_relationships_for_schema() {
    let url = pg_url();
    let schema = unique_schema("rel");
    drop_schema(&url, &schema).await;

    let setup = SqlDocument::new(format!(
        "create schema {schema}; \
         create table {schema}.users(id serial primary key); \
         create table {schema}.orders(id serial primary key, user_id integer references {schema}.users(id)); \
         create table {schema}.tags(id serial primary key);"
    ));
    execute_script(DriverKind::Postgres, &url, &setup)
        .await
        .expect("setup");

    let rels = fetch_relationships(DriverKind::Postgres, &url, &schema)
        .await
        .expect("relationships");

    drop_schema(&url, &schema).await;

    assert_eq!(rels.len(), 1, "one FK edge in schema, got {rels:?}");
    assert_eq!(rels[0].from_table, "orders");
    assert_eq!(rels[0].to_table, "users");
}

#[tokio::test]
#[ignore = "requires TSQLX_TEST_POSTGRES_URL"]
async fn postgres_fetch_records_paginated() {
    let url = pg_url();
    let schema = unique_schema("pg");
    drop_schema(&url, &schema).await;

    let setup = SqlDocument::new(format!(
        "create schema {schema}; \
         create table {schema}.nums(n integer); \
         insert into {schema}.nums values (1),(2),(3),(4),(5);"
    ));
    execute_script(DriverKind::Postgres, &url, &setup)
        .await
        .expect("setup");

    let page0 = fetch_records(DriverKind::Postgres, &url, &schema, "nums", 3, 0)
        .await
        .expect("records page 0");
    assert_eq!(page0.rows.len(), 3);

    let page1 = fetch_records(DriverKind::Postgres, &url, &schema, "nums", 3, 3)
        .await
        .expect("records page 1");

    drop_schema(&url, &schema).await;

    assert_eq!(page1.rows.len(), 2);
}
