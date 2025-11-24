use serde::{Deserialize, Serialize};
use std::env;
use std::path::Path;

use crate::error::{LogSearchError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub log_parser: LogParserConfig,
    pub search: SearchConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub mode: ServerMode,
    pub http_addr: Option<String>,
    pub http_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerMode {
    Stdio,
    Http,
    Both,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogParserConfig {
    pub default_log_start_pattern: Option<String>,
    pub default_timestamp_regex: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    pub default_page_size: usize,
    pub max_page_size: usize,
    pub default_timeout_ms: u64,
    pub max_concurrent_files: usize,
    pub buffer_size: usize,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            default_page_size: 10,
            max_page_size: 100,
            default_timeout_ms: 1_000,
            max_concurrent_files: 4,
            buffer_size: 64 * 1024,
        }
    }
}

impl Config {
    pub fn load_from_path(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| LogSearchError::ConfigError(format!("read {path:?} failed: {e}")))?;
        let is_yaml = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml"))
            .unwrap_or(false);
        let cfg: Config = if is_yaml {
            serde_yaml::from_str(&content)
                .map_err(|e| LogSearchError::ConfigError(format!("parse {path:?} failed: {e}")))?
        } else {
            serde_json::from_str(&content)
                .map_err(|e| LogSearchError::ConfigError(format!("parse {path:?} failed: {e}")))?
        };
        cfg.apply_env_overrides()
    }

    fn apply_env_overrides(mut self) -> Result<Self> {
        if let Ok(mode) = env::var("LOG_SEARCH_MCP__SERVER__MODE") {
            self.server.mode = parse_server_mode(&mode)?;
        }
        if let Ok(addr) = env::var("LOG_SEARCH_MCP__SERVER__HTTP_ADDR") {
            self.server.http_addr = Some(addr);
        }
        if let Ok(port) = env::var("LOG_SEARCH_MCP__SERVER__HTTP_PORT") {
            self.server.http_port = Some(parse_num(&port, "http_port")?);
        }
        if let Ok(pat) = env::var("LOG_SEARCH_MCP__LOG_PARSER__DEFAULT_LOG_START_PATTERN") {
            self.log_parser.default_log_start_pattern = Some(pat);
        }
        if let Ok(ts) = env::var("LOG_SEARCH_MCP__LOG_PARSER__DEFAULT_TIMESTAMP_REGEX") {
            self.log_parser.default_timestamp_regex = Some(ts);
        }
        if let Ok(n) = env::var("LOG_SEARCH_MCP__SEARCH__DEFAULT_PAGE_SIZE") {
            self.search.default_page_size = parse_num(&n, "default_page_size")?;
        }
        if let Ok(n) = env::var("LOG_SEARCH_MCP__SEARCH__MAX_PAGE_SIZE") {
            self.search.max_page_size = parse_num(&n, "max_page_size")?;
        }
        if let Ok(n) = env::var("LOG_SEARCH_MCP__SEARCH__DEFAULT_TIMEOUT_MS") {
            self.search.default_timeout_ms = parse_num(&n, "default_timeout_ms")?;
        }
        if let Ok(n) = env::var("LOG_SEARCH_MCP__SEARCH__MAX_CONCURRENT_FILES") {
            self.search.max_concurrent_files = parse_num(&n, "max_concurrent_files")?;
        }
        if let Ok(n) = env::var("LOG_SEARCH_MCP__SEARCH__BUFFER_SIZE") {
            self.search.buffer_size = parse_num(&n, "buffer_size")?;
        }
        Ok(self.validate()?)
    }

    pub fn validate(self) -> Result<Self> {
        if let Some(port) = self.server.http_port {
            if port == 0 {
                return Err(LogSearchError::ConfigError(
                    "server.http_port must be > 0".into(),
                ));
            }
        }
        if self.search.default_page_size == 0 {
            return Err(LogSearchError::ConfigError(
                "search.default_page_size must be > 0".into(),
            ));
        }
        if self.search.max_page_size == 0 {
            return Err(LogSearchError::ConfigError(
                "search.max_page_size must be > 0".into(),
            ));
        }
        if self.search.max_page_size < self.search.default_page_size {
            return Err(LogSearchError::ConfigError(
                "search.max_page_size must be >= default_page_size".into(),
            ));
        }
        if self.search.buffer_size == 0 {
            return Err(LogSearchError::ConfigError(
                "search.buffer_size must be > 0".into(),
            ));
        }
        Ok(self)
    }
}

fn parse_server_mode(s: &str) -> Result<ServerMode> {
    match s.to_ascii_lowercase().as_str() {
        "stdio" => Ok(ServerMode::Stdio),
        "http" => Ok(ServerMode::Http),
        "both" => Ok(ServerMode::Both),
        other => Err(LogSearchError::ConfigError(format!(
            "invalid server.mode: {other}"
        ))),
    }
}

fn parse_num<T>(s: &str, key: &str) -> Result<T>
where
    T: std::str::FromStr,
{
    s.parse::<T>()
        .map_err(|_| LogSearchError::ConfigError(format!("invalid number for {key}: {s}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn load_yaml_and_env_override() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        fs::write(
            &path,
            r#"
server:
  mode: http
  http_addr: "0.0.0.0"
  http_port: 8080
log_parser:
  default_log_start_pattern: null
  default_timestamp_regex: null
search:
  default_page_size: 10
  max_page_size: 100
  default_timeout_ms: 1000
  max_concurrent_files: 4
  buffer_size: 65536
"#,
        )
        .unwrap();

        env::set_var("LOG_SEARCH_MCP__SERVER__MODE", "stdio");
        env::set_var("LOG_SEARCH_MCP__SEARCH__BUFFER_SIZE", "1024");
        let cfg = Config::load_from_path(&path).unwrap();
        env::remove_var("LOG_SEARCH_MCP__SERVER__MODE");
        env::remove_var("LOG_SEARCH_MCP__SEARCH__BUFFER_SIZE");

        assert!(matches!(cfg.server.mode, ServerMode::Stdio));
        assert_eq!(cfg.search.buffer_size, 1024);
        assert_eq!(cfg.server.http_port, Some(8080));
    }

    #[test]
    fn invalid_page_size_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        fs::write(
            &path,
            r#"
server:
  mode: http
  http_addr: "0.0.0.0"
  http_port: 8080
log_parser:
  default_log_start_pattern: null
  default_timestamp_regex: null
search:
  default_page_size: 0
  max_page_size: 100
  default_timeout_ms: 1000
  max_concurrent_files: 4
  buffer_size: 65536
"#,
        )
        .unwrap();

        let err = Config::load_from_path(&path).unwrap_err().to_string();
        assert!(err.contains("default_page_size"));
    }
}
