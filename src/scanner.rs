use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use walkdir::WalkDir;

use crate::error::{LogSearchError, Result};
use crate::model::FileScanConfig;

/// File scanner: recursively collect log files by include/exclude globs.
#[derive(Clone, Default)]
pub struct FileScanner;

const DEFAULT_INCLUDE_GLOBS: &[&str] = &["**/*.log", "**/*.log.gz", "**/*.gz"];

impl FileScanner {
    pub fn new() -> Self {
        Self
    }

    pub fn scan(&self, config: &FileScanConfig) -> Result<Vec<PathBuf>> {
        let include_fallback: Vec<String>;
        let include_slice: &[String] = if config.include_globs.is_empty() {
            include_fallback = DEFAULT_INCLUDE_GLOBS
                .iter()
                .map(|s| s.to_string())
                .collect();
            &include_fallback
        } else {
            &config.include_globs
        };

        let include = build_globset(include_slice)?;
        let exclude = build_globset(&config.exclude_globs)?;
        let mut files = Vec::new();

        for entry in WalkDir::new(&config.root_path)
            .into_iter()
            .filter_map(std::result::Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }

            let path = entry.path();
            if !exclude.is_empty() && matches(&exclude, path) {
                continue;
            }
            if include.is_empty() || matches(&include, path) {
                files.push(path.to_path_buf());
            }
        }

        Ok(files)
    }
}

fn build_globset(patterns: &[String]) -> Result<GlobSet> {
    if patterns.is_empty() {
        let builder = GlobSetBuilder::new();
        return Ok(
            builder
                .build()
                .map_err(|e| LogSearchError::ConfigError(e.to_string()))?,
        );
    }

    let mut builder = GlobSetBuilder::new();
    for pat in patterns {
        let glob = Glob::new(pat).map_err(|e| LogSearchError::ConfigError(e.to_string()))?;
        builder.add(glob);
    }
    builder.build().map_err(|e| LogSearchError::ConfigError(e.to_string()))
}

fn matches(globset: &GlobSet, path: &Path) -> bool {
    if globset.is_empty() {
        return true;
    }
    if globset.is_match(path) {
        return true;
    }
    let normalized = path.to_string_lossy().replace('\\', "/");
    globset.is_match(normalized.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn touch(path: &Path) {
        std::fs::write(path, b"test").unwrap();
    }

    #[test]
    fn scan_with_default_include_and_exclude() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let keep_log = root.join("a.log");
        let keep_gz = root.join("b.gz");
        let drop_txt = root.join("c.txt");
        let skip_dir = root.join("skip");
        let skip_log = skip_dir.join("d.log");

        std::fs::create_dir_all(&skip_dir).unwrap();
        touch(&keep_log);
        touch(&keep_gz);
        touch(&drop_txt);
        touch(&skip_log);

        let cfg = FileScanConfig {
            root_path: root.to_path_buf(),
            include_globs: Vec::new(),
            exclude_globs: vec!["**/skip/**".to_string()],
        };

        let mut paths = FileScanner::new().scan(&cfg).unwrap();
        paths.sort();

        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&keep_log));
        assert!(paths.contains(&keep_gz));
        assert!(!paths.contains(&drop_txt));
        assert!(!paths.contains(&skip_log));
    }
}
