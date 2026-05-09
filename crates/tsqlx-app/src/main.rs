use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tokio::io::AsyncReadExt;
use tsqlx_core::{AppConfig, ConnectionConfig, DriverKind};
use tsqlx_db::{execute_script, QueryOutput};
use tsqlx_sql::SqlDocument;

#[derive(Debug, Parser)]
#[command(name = "tsqlx")]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    Exec {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(short, long)]
        connection: Option<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        driver: Option<DriverArg>,
        #[arg(short, long)]
        file: Option<PathBuf>,
    },
    Tui {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(short, long)]
        connection: Option<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        driver: Option<DriverArg>,
    },
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    Check {
        #[arg(long)]
        config: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum DriverArg {
    Postgres,
    Sqlite,
}

impl From<DriverArg> for DriverKind {
    fn from(value: DriverArg) -> Self {
        match value {
            DriverArg::Postgres => Self::Postgres,
            DriverArg::Sqlite => Self::Sqlite,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Config {
            command: ConfigCommand::Check { config },
        }) => check_config(config).await,
        Some(Command::Exec {
            config,
            connection,
            url,
            driver,
            file,
        }) => exec(config, connection, url, driver, file).await,
        Some(Command::Tui {
            config,
            connection,
            url,
            driver,
        }) => {
            if url.is_some() || connection.is_some() {
                let conn = resolve_connection(config, connection, url, driver).await?;
                tsqlx_tui::run(conn.driver, conn.url).await
            } else {
                let saved = load_saved_connections(config).await;
                tsqlx_tui::run_connect(saved).await
            }
        }
        None => {
            let saved = load_saved_connections(None).await;
            tsqlx_tui::run_connect(saved).await
        }
    }
}

async fn check_config(config: PathBuf) -> Result<()> {
    let config = AppConfig::load(&config).await?;

    println!("config ok");
    println!("connections: {}", config.connections.len());

    for (name, connection) in config.connections {
        println!("- {name}: {:?}", connection.driver);
    }

    Ok(())
}

async fn exec(
    config: Option<PathBuf>,
    connection: Option<String>,
    url: Option<String>,
    driver: Option<DriverArg>,
    file: Option<PathBuf>,
) -> Result<()> {
    let connection = resolve_connection(config, connection, url, driver).await?;
    let sql = read_sql(file).await?;
    let document = SqlDocument::new(sql);
    let output = execute_script(connection.driver, &connection.url, &document).await?;

    print_output(&output);
    Ok(())
}

async fn resolve_connection(
    config: Option<PathBuf>,
    connection: Option<String>,
    url: Option<String>,
    driver: Option<DriverArg>,
) -> Result<ConnectionConfig> {
    if let Some(url) = url {
        let driver =
            driver.map_or_else(|| DriverKind::from_url(&url), |driver| Ok(driver.into()))?;

        return Ok(ConnectionConfig { driver, url });
    }

    let config_path = config.context("missing --config or --url")?;
    let connection_name = connection.context("missing --connection when using --config")?;
    let config = AppConfig::load(config_path).await?;

    config.connection(&connection_name).map_err(Into::into)
}

async fn read_sql(file: Option<PathBuf>) -> Result<String> {
    if let Some(file) = file {
        tokio::fs::read_to_string(&file)
            .await
            .with_context(|| format!("failed to read SQL file `{}`", file.display()))
    } else {
        let mut input = String::new();
        tokio::io::stdin()
            .read_to_string(&mut input)
            .await
            .context("failed to read SQL from stdin")?;
        Ok(input)
    }
}

async fn load_saved_connections(config: Option<PathBuf>) -> Vec<(String, ConnectionConfig)> {
    let cfg = if let Some(path) = config {
        AppConfig::load(path).await.ok()
    } else {
        AppConfig::load_default().await.ok().flatten()
    };
    cfg.map(|c| c.connections.into_iter().collect())
        .unwrap_or_default()
}

fn print_output(output: &QueryOutput) {
    for (index, statement) in output.statements.iter().enumerate() {
        println!("-- statement {}", index + 1);

        if statement.columns.is_empty() {
            println!("rows affected: {}", statement.rows_affected);
            continue;
        }

        println!("{}", statement.columns.join("\t"));

        for row in &statement.rows {
            println!("{}", row.join("\t"));
        }
    }
}
