use tsql_core::DriverKind;
use tsql_db::execute_script;
use tsql_sql::SqlDocument;

#[tokio::test]
#[ignore = "requires TSQL_TEST_POSTGRES_URL"]
async fn executes_postgres_script() {
    let url = std::env::var("TSQL_TEST_POSTGRES_URL").expect("TSQL_TEST_POSTGRES_URL is set");
    let document = SqlDocument::new(
        r#"
        drop table if exists tsql_test_users;
        create table tsql_test_users(id serial primary key, name text not null);
        insert into tsql_test_users(name) values ('ada'), ('grace');
        select id, name from tsql_test_users order by id;
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
