use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct FsConfig {
    pub allow_roots: Vec<PathBuf>,
    pub deny_patterns: Vec<String>,
    pub io_mode: String,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub fs: FsConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            fs: FsConfig { allow_roots: vec![], deny_patterns: vec![], io_mode: "auto".to_string() },
        }
    }
}

