use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use walkdir::WalkDir;

use crate::error::{LogSearchError, Result};
use crate::model::FileScanConfig;

/// 文件扫描器：根据包含/排除 globs 递归收集日志文件。
#[derive(Clone, Default)]
pub struct FileScanner;

const DEFAULT_INCLUDE_GLOBS: &[&str] = &["**/*.log", "**/*.log.gz", "**/*.gz"];

impl FileScanner {
    pub fn new() -> Self {
        Self
    }

    pub fn scan(&self, config: &FileScanConfig) -> Result<Vec<PathBuf>> {
        self.scan_with_paths(config, &None)
    }

    pub fn scan_with_paths(
        &self,
        config: &FileScanConfig,
        explicit_paths: &Option<Vec<String>>,
    ) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        
        // Debug log
        use std::io::Write;
        let log_file_path = "/tmp/log-mcp-debug.log";

        if let Some(paths) = explicit_paths {
            for p_str in paths {
                let p = PathBuf::from(p_str);
                let exists = p.exists();
                let is_file = p.is_file();
                
                if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(log_file_path) {
                     let _ = writeln!(file, "Checking explicit path: {:?}, exists: {}, is_file: {}", p, exists, is_file);
                     if !exists {
                         // 尝试列出父目录以查看内容
                         if let Some(parent) = p.parent() {
                             let _ = writeln!(file, "Listing parent {:?}:", parent);
                             if let Ok(entries) = std::fs::read_dir(parent) {
                                 for entry in entries.flatten() {
                                     let _ = writeln!(file, "  - {:?}", entry.path());
                                 }
                             } else {
                                 let _ = writeln!(file, "  Failed to read parent directory");
                             }
                         }
                     }
                }

                if exists {
                     // 简单地检查是否存在，不强制检查是否是 file (可能是 symlink)
                     // 但我们还是希望只处理文件。
                     if is_file {
                        files.push(p);
                     }
                }
            }
        }

        if config.root_path.as_os_str().is_empty() {
            return Ok(files);
        }

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

        files.sort();
        files.dedup();
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
