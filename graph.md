# TSQL Architecture Graph

```mermaid
flowchart TD
    User[User] --> CLI[tsql CLI]
    User --> TUI[Ratatui TUI]

    CLI --> Core[tsql-core]
    CLI --> SQL[tsql-sql]
    CLI --> DB[tsql-db]
    CLI --> TUI

    TUI --> Core
    TUI --> SQL
    TUI --> DB

    Core --> Config[TOML Config]
    Core --> Env[Environment Variables]

    SQL --> Document[SqlDocument]
    SQL --> Splitter[Multi-statement Splitter]

    DB --> SQLite[SQLite]
    DB --> Postgres[Postgres]
    DB --> Output[QueryOutput]

    Output --> CLI
    Output --> TUI

    CI[GitHub Actions CI] --> Fmt[cargo fmt]
    CI --> Clippy[cargo clippy]
    CI --> Tests[cargo test]
    CI --> PgTest[Postgres Integration]
    CI --> Audit[cargo audit]

    Security[Security Workflow] --> TruffleHog[TruffleHog]
    Security --> Gitleaks[Gitleaks]
    Security --> Semgrep[Semgrep]
    Security --> Trivy[Trivy]
```

## Dependency Direction

```text
tsql-app -> tsql-core, tsql-sql, tsql-db, tsql-tui
tsql-tui -> tsql-core, tsql-sql, tsql-db
tsql-db  -> tsql-core, tsql-sql
tsql-sql -> no internal crates
tsql-core -> no internal crates
```

## Release Gate

```text
feature branch -> PR -> required CI/security -> owner review -> merge main -> tag v0.1.0 -> manual release workflow
```
