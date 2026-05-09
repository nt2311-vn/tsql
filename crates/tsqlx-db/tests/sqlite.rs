use std::sync::atomic::{AtomicU64, Ordering};

use tsqlx_core::DriverKind;
use tsqlx_db::{
    execute_script, fetch_overview, fetch_records, fetch_relationships, fetch_table_info,
};
use tsqlx_sql::SqlDocument;

static DB_SEQ: AtomicU64 = AtomicU64::new(0);

fn tmp_db_url() -> (String, String) {
    let id = DB_SEQ.fetch_add(1, Ordering::SeqCst);
    let path = format!("/tmp/tsqlx_test_{id}.db");
    let _ = std::fs::remove_file(&path);
    let url = format!("sqlite:{path}");
    (url, path)
}

#[tokio::test]
async fn executes_sqlite_script() {
    let (url, path) = tmp_db_url();
    let document = SqlDocument::new(
        r#"
        create table users(id integer primary key, name text not null);
        insert into users(name) values ('ada'), ('grace');
        select id, name from users order by id;
        "#,
    );

    let output = execute_script(DriverKind::Sqlite, &url, &document)
        .await
        .expect("sqlite script executes");

    let _ = std::fs::remove_file(path);
    assert_eq!(output.statements.len(), 3);
    assert_eq!(output.statements[2].columns, ["id", "name"]);
    assert_eq!(output.statements[2].rows[0], ["1", "ada"]);
    assert_eq!(output.statements[2].rows[1], ["2", "grace"]);
}

#[tokio::test]
async fn sqlite_overview_lists_tables() {
    let (url, path) = tmp_db_url();
    let setup = SqlDocument::new(
        "create table users(id integer primary key, name text not null);\
         create table orders(id integer primary key, user_id integer references users(id));",
    );
    execute_script(DriverKind::Sqlite, &url, &setup)
        .await
        .expect("setup");

    let ov = fetch_overview(DriverKind::Sqlite, &url)
        .await
        .expect("overview");

    let _ = std::fs::remove_file(path);
    assert_eq!(ov.schemas.len(), 1);
    assert_eq!(ov.schemas[0].name, "main");
    let tables = &ov.schemas[0].tables;
    assert!(tables.contains(&"users".to_owned()), "users in overview");
    assert!(tables.contains(&"orders".to_owned()), "orders in overview");
}

#[tokio::test]
async fn sqlite_table_info_columns_and_pk() {
    let (url, path) = tmp_db_url();
    let setup = SqlDocument::new(
        "create table users(id integer primary key, name text not null, email text);",
    );
    execute_script(DriverKind::Sqlite, &url, &setup)
        .await
        .expect("setup");

    let info = fetch_table_info(DriverKind::Sqlite, &url, "main", "users")
        .await
        .expect("table info");

    let _ = std::fs::remove_file(path);
    assert_eq!(info.name, "users");
    assert_eq!(info.columns.len(), 3);
    assert_eq!(info.columns[0].name, "id");
    assert_eq!(info.columns[1].name, "name");
    assert!(!info.columns[1].is_nullable, "name NOT NULL");
    assert!(info.columns[2].is_nullable, "email nullable");

    let pk = info.primary_key.expect("pk exists");
    assert!(pk.column_names.contains(&"id".to_owned()));
}

#[tokio::test]
async fn sqlite_table_info_foreign_keys() {
    let (url, path) = tmp_db_url();
    let setup = SqlDocument::new(
        "create table users(id integer primary key, name text);\
         create table orders(id integer primary key, user_id integer references users(id));",
    );
    execute_script(DriverKind::Sqlite, &url, &setup)
        .await
        .expect("setup");

    let info = fetch_table_info(DriverKind::Sqlite, &url, "main", "orders")
        .await
        .expect("table info");

    let _ = std::fs::remove_file(path);
    assert_eq!(info.foreign_keys.len(), 1);
    assert_eq!(info.foreign_keys[0].referenced_table, "users");
    assert!(info.foreign_keys[0]
        .column_names
        .contains(&"user_id".to_owned()));
}

#[tokio::test]
async fn sqlite_relationships_for_schema() {
    let (url, path) = tmp_db_url();
    let setup = SqlDocument::new(
        "create table users(id integer primary key);\
         create table orders(id integer primary key, user_id integer references users(id));\
         create table tags(id integer primary key);",
    );
    execute_script(DriverKind::Sqlite, &url, &setup)
        .await
        .expect("setup");

    let rels = fetch_relationships(DriverKind::Sqlite, &url, "main")
        .await
        .expect("relationships");

    let _ = std::fs::remove_file(path);
    assert_eq!(rels.len(), 1);
    assert_eq!(rels[0].from_table, "orders");
    assert_eq!(rels[0].to_table, "users");
}

#[tokio::test]
async fn sqlite_fetch_records_paginated() {
    let (url, path) = tmp_db_url();
    let setup = SqlDocument::new(
        "create table nums(n integer);\
         insert into nums values (1),(2),(3),(4),(5);",
    );
    execute_script(DriverKind::Sqlite, &url, &setup)
        .await
        .expect("setup");

    let page = fetch_records(DriverKind::Sqlite, &url, "main", "nums", 3, 0)
        .await
        .expect("records page 0");
    assert_eq!(page.rows.len(), 3);

    let page2 = fetch_records(DriverKind::Sqlite, &url, "main", "nums", 3, 3)
        .await
        .expect("records page 1");
    let _ = std::fs::remove_file(path);
    assert_eq!(page2.rows.len(), 2);
}
