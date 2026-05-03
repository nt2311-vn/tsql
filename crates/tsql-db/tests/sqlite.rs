use tsql_core::DriverKind;
use tsql_db::execute_script;
use tsql_sql::SqlDocument;

#[tokio::test]
async fn executes_sqlite_script() {
    let document = SqlDocument::new(
        r#"
        create table users(id integer primary key, name text not null);
        insert into users(name) values ('ada'), ('grace');
        select id, name from users order by id;
        "#,
    );

    let output = execute_script(DriverKind::Sqlite, "sqlite::memory:", &document)
        .await
        .expect("sqlite script executes");

    assert_eq!(output.statements.len(), 3);
    assert_eq!(output.statements[2].columns, ["id", "name"]);
    assert_eq!(output.statements[2].rows[0], ["1", "ada"]);
    assert_eq!(output.statements[2].rows[1], ["2", "grace"]);
}
