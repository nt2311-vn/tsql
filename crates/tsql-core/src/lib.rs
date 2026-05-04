use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

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
#[serde(rename_all = "kebab-case")]
pub enum DriverKind {
    Postgres,
    Sqlite,
}

impl DriverKind {
    pub fn from_url(url: &str) -> Result<Self, ConfigError> {
        if url.starts_with("postgres://") || url.starts_with("postgresql://") {
            Ok(Self::Postgres)
        } else if url.starts_with("sqlite:") {
            Ok(Self::Sqlite)
        } else {
            Err(ConfigError::UnsupportedDriver(
                url.split(':').next().unwrap_or(url).to_owned(),
            ))
        }
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
        default_config_path, expand_environment_variables, AppConfig, DriverKind, ProjectInfo,
    };

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
}
