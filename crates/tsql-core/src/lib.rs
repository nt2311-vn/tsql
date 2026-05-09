use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;
use tokio::fs;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file `{path}`: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("failed to parse TOML config `{path}`: {source}")]
    Parse {
        path: String,
        source: toml::de::Error,
    },
    #[error("connection `{0}` not found")]
    MissingConnection(String),
    #[error("unsupported database driver `{0}`")]
    UnsupportedDriver(String),
    #[error("environment variable `{0}` is not set")]
    MissingEnvironmentVariable(String),
    #[error("failed to write config file `{path}`: {source}")]
    Write {
        path: String,
        source: std::io::Error,
    },
    #[error("connection name `{0}` is invalid (must be a non-empty TOML key)")]
    InvalidConnectionName(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInfo {
    pub name: &'static str,
    pub version: &'static str,
}

impl Default for ProjectInfo {
    fn default() -> Self {
        Self {
            name: "tsql",
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DriverKind {
    Postgres,
    Sqlite,
    /// MySQL and MariaDB share the wire protocol; sqlx's `mysql`
    /// feature speaks both. URLs starting with either `mysql://` or
    /// `mariadb://` resolve here. `driver = "mariadb"` is also
    /// accepted in the TOML config as a friendly alias.
    #[serde(alias = "mariadb")]
    Mysql,
}

impl DriverKind {
    pub fn from_url(url: &str) -> Result<Self, ConfigError> {
        if url.starts_with("postgres://") || url.starts_with("postgresql://") {
            Ok(Self::Postgres)
        } else if url.starts_with("sqlite:") {
            Ok(Self::Sqlite)
        } else if url.starts_with("mysql://") || url.starts_with("mariadb://") {
            Ok(Self::Mysql)
        } else {
            Err(ConfigError::UnsupportedDriver(
                url.split(':').next().unwrap_or(url).to_owned(),
            ))
        }
    }

    /// MySQL deserves both spellings of its scheme. sqlx's `MySqlPool`
    /// only accepts `mysql://`, so we normalise on connect.
    pub fn normalise_url(self, url: &str) -> String {
        if matches!(self, Self::Mysql) {
            if let Some(rest) = url.strip_prefix("mariadb://") {
                return format!("mysql://{rest}");
            }
        }
        url.to_owned()
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct AppConfig {
    #[serde(default)]
    pub editor: EditorConfig,
    #[serde(default)]
    pub connections: BTreeMap<String, ConnectionConfig>,
}

impl AppConfig {
    pub async fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let raw = tokio::fs::read_to_string(path)
            .await
            .map_err(|source| ConfigError::Read {
                path: path.display().to_string(),
                source,
            })?;
        let expanded = expand_environment_variables(&raw)?;

        toml::from_str(&expanded).map_err(|source| ConfigError::Parse {
            path: path.display().to_string(),
            source,
        })
    }

    pub fn connection(&self, name: &str) -> Result<ConnectionConfig, ConfigError> {
        self.connections
            .get(name)
            .cloned()
            .ok_or_else(|| ConfigError::MissingConnection(name.to_owned()))
    }

    pub async fn load_default() -> Result<Option<Self>, ConfigError> {
        let path = default_config_path();
        if !path.exists() {
            return Ok(None);
        }
        Self::load(&path).await.map(Some)
    }
}

/// Append a `[connections.<name>]` block to the user's TOML config so a
/// freshly-typed connection survives across sessions. We deliberately
/// append raw text rather than round-tripping the parsed `AppConfig` —
/// that way any `${ENV_VAR}` placeholders, comments, and ordering already
/// in the file stay byte-for-byte intact. The file (and parent dir) are
/// created on demand if they don't exist.
///
/// The `name` is validated as a bare TOML key (alphanumerics, `_`, `-`).
/// Anything else returns `InvalidConnectionName` rather than producing
/// a malformed file.
pub async fn append_connection(
    path: &Path,
    name: &str,
    connection: &ConnectionConfig,
) -> Result<(), ConfigError> {
    if !is_valid_connection_name(name) {
        return Err(ConfigError::InvalidConnectionName(name.to_owned()));
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent)
                .await
                .map_err(|source| ConfigError::Write {
                    path: parent.display().to_string(),
                    source,
                })?;
        }
    }

    let driver = match connection.driver {
        DriverKind::Postgres => "postgres",
        DriverKind::Sqlite => "sqlite",
        DriverKind::Mysql => "mysql",
    };
    let escaped_url = connection.url.replace('\\', "\\\\").replace('"', "\\\"");
    let mut block = String::new();
    if path.exists() {
        // Make sure we start the new block on its own line even if the
        // previous file didn't end in a newline.
        let existing = fs::read_to_string(path)
            .await
            .map_err(|source| ConfigError::Write {
                path: path.display().to_string(),
                source,
            })?;
        if !existing.is_empty() && !existing.ends_with('\n') {
            block.push('\n');
        }
        if !existing.is_empty() {
            block.push('\n');
        }
    }
    block.push_str(&format!(
        "[connections.{name}]\ndriver = \"{driver}\"\nurl = \"{escaped_url}\"\n"
    ));

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .map_err(|source| ConfigError::Write {
            path: path.display().to_string(),
            source,
        })?;
    file.write_all(block.as_bytes())
        .await
        .map_err(|source| ConfigError::Write {
            path: path.display().to_string(),
            source,
        })?;
    file.flush().await.map_err(|source| ConfigError::Write {
        path: path.display().to_string(),
        source,
    })?;
    Ok(())
}

fn is_valid_connection_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub fn default_config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".config")
        });
    base.join("tsql").join("config.toml")
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct EditorConfig {
    pub tab_width: u8,
    pub indent: IndentStyle,
    pub theme: String,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            tab_width: 4,
            indent: IndentStyle::Spaces,
            theme: "catppuccin-mocha".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum IndentStyle {
    Spaces,
    Tabs,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ConnectionConfig {
    pub driver: DriverKind,
    pub url: String,
}

pub fn expand_environment_variables(input: &str) -> Result<String, ConfigError> {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '$' && chars.peek() == Some(&'{') {
            chars.next();
            let mut name = String::new();

            for next in chars.by_ref() {
                if next == '}' {
                    break;
                }

                name.push(next);
            }

            let value = std::env::var(&name)
                .map_err(|_| ConfigError::MissingEnvironmentVariable(name.clone()))?;
            output.push_str(&value);
        } else {
            output.push(ch);
        }
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::{
        append_connection, default_config_path, expand_environment_variables,
        is_valid_connection_name, AppConfig, ConnectionConfig, DriverKind, ProjectInfo,
    };
    use tempfile::tempdir;

    #[test]
    fn default_project_info_has_name() {
        let info = ProjectInfo::default();

        assert_eq!(info.name, "tsql");
    }

    #[test]
    fn parses_config() {
        let config: AppConfig = toml::from_str(
            r#"
            [connections.local]
            driver = "sqlite"
            url = "sqlite::memory:"
            "#,
        )
        .expect("valid config");

        let connection = config.connection("local").expect("connection exists");

        assert_eq!(connection.driver, DriverKind::Sqlite);
        assert_eq!(connection.url, "sqlite::memory:");
        assert_eq!(config.editor.theme, "catppuccin-mocha");
    }

    #[test]
    fn default_config_uses_xdg() {
        std::env::set_var("XDG_CONFIG_HOME", "/tmp/tsql_xdg_test");
        let path = default_config_path();
        assert_eq!(
            path.to_str().unwrap(),
            "/tmp/tsql_xdg_test/tsql/config.toml"
        );
        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn expands_environment_variables() {
        std::env::set_var("TSQL_TEST_URL", "sqlite::memory:");

        let expanded =
            expand_environment_variables(r#"url = "${TSQL_TEST_URL}""#).expect("expanded");

        assert_eq!(expanded, r#"url = "sqlite::memory:""#);
    }

    #[test]
    fn connection_name_validator_rejects_unsafe_keys() {
        assert!(is_valid_connection_name("prod"));
        assert!(is_valid_connection_name("prod-2"));
        assert!(is_valid_connection_name("dev_box_1"));
        assert!(!is_valid_connection_name(""));
        assert!(!is_valid_connection_name("has space"));
        assert!(!is_valid_connection_name("a.b"));
        assert!(!is_valid_connection_name("\"quoted\""));
    }

    #[tokio::test]
    async fn append_connection_creates_file_when_missing() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("config.toml");
        let conn = ConnectionConfig {
            driver: DriverKind::Sqlite,
            url: "sqlite::memory:".to_owned(),
        };

        append_connection(&path, "local", &conn)
            .await
            .expect("write succeeds");

        let cfg = AppConfig::load(&path).await.expect("reloads");
        assert_eq!(cfg.connection("local").unwrap().url, "sqlite::memory:");
    }

    #[tokio::test]
    async fn append_connection_preserves_existing_env_var_placeholders() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let original = "[connections.prod]\ndriver = \"postgres\"\nurl = \"${DATABASE_URL}\"\n";
        tokio::fs::write(&path, original).await.unwrap();

        let conn = ConnectionConfig {
            driver: DriverKind::Postgres,
            url: "postgres://user:pass@localhost/dev".to_owned(),
        };
        append_connection(&path, "dev", &conn).await.unwrap();

        // The raw text still contains ${DATABASE_URL}; the writer never
        // expanded the placeholder when round-tripping.
        let raw = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(raw.contains("${DATABASE_URL}"), "placeholder lost: {raw:?}");
        assert!(raw.contains("[connections.dev]"));
    }

    #[tokio::test]
    async fn append_connection_rejects_invalid_names() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let conn = ConnectionConfig {
            driver: DriverKind::Sqlite,
            url: "sqlite::memory:".to_owned(),
        };
        let err = append_connection(&path, "has space", &conn)
            .await
            .expect_err("must reject");
        assert!(matches!(err, super::ConfigError::InvalidConnectionName(_)));
        assert!(!path.exists(), "no file should have been created");
    }
}
