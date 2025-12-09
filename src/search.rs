use std::path::PathBuf;
use std::time::Instant;

use futures::{stream, Stream, StreamExt};
use tokio::time::{timeout, Duration};
use tracing::{error, warn};

use crate::config::SearchConfig;
use crate::error::Result;
use crate::model::{HitResult, MatchPosition, SearchRequest, SearchResponse, TimeFilter};
use crate::parser::LogParser;
use crate::query::QueryProcessor;
use crate::reader::FileReader;
use crate::scanner::FileScanner;

use std::sync::{Arc, RwLock};
use crate::config::Config;

/// Search engine: orchestrates scanning, reading, parsing, and matching.
pub struct SearchEngine {
    config: Arc<RwLock<Config>>,
    scanner: FileScanner,
    reader: FileReader,
    parser: LogParser,
    query: QueryProcessor,
}

impl SearchEngine {
    pub fn new(config: Arc<RwLock<Config>>) -> Self {
        let buffer_size = config.read().unwrap().search.buffer_size;
        Self {
            reader: FileReader::new(buffer_size),
            config,
            scanner: FileScanner::new(),
            parser: LogParser::new(),
            query: QueryProcessor::new(),
        }
    }

    pub fn list_files(&self, config: &crate::model::FileScanConfig) -> Result<Vec<PathBuf>> {
        // Merge global paths if needed, though list_files is usually explicit.
        // But if config.root_path is empty, we might rely on global paths.
        // For now, let's just pass through, but if we wanted to support global paths here too:
        let global_cfg = self.config.read().unwrap();
        let global_paths = global_cfg.log_sources.log_file_paths.clone();
        
        if let Some(paths) = global_paths {
             // If scanner supports explicit paths, use them.
             // Currently scanner only supports root_path + globs.
             // We need to modify scanner.
             self.scanner.scan_with_paths(config, &Some(paths))
        } else {
             self.scanner.scan(config)
        }
    }

    pub async fn search(&self, request: SearchRequest) -> Result<SearchResponse> {
        self.validate_request(&request)?;
        let started = Instant::now();
        
        let (search_config, log_parser_config, log_sources) = {
            let cfg = self.config.read().unwrap();
            (cfg.search.clone(), cfg.log_parser.clone(), cfg.log_sources.clone())
        };

        // Scan files
        let files = if let Some(paths) = &log_sources.log_file_paths {
             self.scanner.scan_with_paths(&request.scan_config, &Some(paths.clone()))?
        } else {
             self.scanner.scan(&request.scan_config)?
        };

        let mut hits: Vec<HitResult> = Vec::new();
        let mut failed_files = Vec::new();
        let mut timed_out = false;
        let mut files_scanned = 0usize;

        let log_start_pattern = request
            .log_start_pattern
            .as_ref()
            .or(log_parser_config.default_log_start_pattern.as_ref())
            .cloned();

        let mut time_filter = request.time_filter.clone();
        if time_filter.is_none() {
            if let Some(ts) = &log_parser_config.default_timestamp_regex {
                time_filter = Some(TimeFilter {
                    time_start: None,
                    time_end: None,
                    timestamp_regex: Some(ts.clone()),
                });
            }
        }

        let log_start_re = if let Some(pat) = &log_start_pattern {
            Some(self.query.compile_regex(pat, true)?)
        } else {
            None
        };

        let max_concurrent = search_config.max_concurrent_files.max(1);

        let mut tasks = stream::iter(files.into_iter()).map(|path| {
            let reader = self.reader.clone();
            let parser = self.parser.clone();
            let query = self.query.clone();
            let request = request.clone();
            let log_start_re = log_start_re.clone();
            let default_timeout = search_config.default_timeout_ms;
            let time_filter = time_filter.clone();

            async move {
                if let Ok(meta) = std::fs::metadata(&path) {
                    const TEN_GB: u64 = 10 * 1024 * 1024 * 1024;
                    if meta.len() > TEN_GB {
                        warn!("file larger than 10GB: {}", path.display());
                    }
                }

                let single_file = async {
                    let lines = reader.read_lines(&path).await?;
                    let entries = parser.parse(path.clone(), lines, log_start_re).await?;
                    scan_entries_static(&query, entries, &request, time_filter).await
                };

                let effective_timeout = request
                    .hard_timeout_ms
                    .or(Some(default_timeout))
                    .filter(|ms| *ms > 0);

                let result = if let Some(ms) = effective_timeout {
                    match timeout(Duration::from_millis(ms), single_file).await {
                        Ok(res) => res.map(|v| (v, false)),
                        Err(_) => Ok((Vec::new(), true)),
                    }
                } else {
                    single_file.await.map(|v| (v, false))
                };

                match result {
                    Ok((hits, timed_out)) => TaskResult {
                        hits,
                        failed: None,
                        timed_out,
                    },
                    Err(e) => TaskResult {
                        hits: Vec::new(),
                        failed: Some((path, e.to_string())),
                        timed_out: false,
                    },
                }
            }
        })
        .buffer_unordered(max_concurrent);

        while let Some(task) = tasks.next().await {
            files_scanned += 1;
            if let Some(f) = task.failed {
                error!("failed to search {}: {}", f.0.display(), f.1);
                failed_files.push(f);
            } else {
                hits.extend(task.hits);
            }
            if task.timed_out {
                timed_out = true;
                break;
            }
            if let Some(limit) = request.max_hits {
                if hits.len() >= limit {
                    break;
                }
            }
        }

        let page_size = if request.page_size == 0 {
            search_config.default_page_size
        } else {
            request
                .page_size
                .min(search_config.max_page_size)
                .max(1)
        };

        let total_hits = hits.len();
        let total_pages = if page_size == 0 {
            0
        } else {
            (total_hits + page_size - 1) / page_size
        };

        let page = request.page.max(1);
        let start = page_size.saturating_mul(page.saturating_sub(1));
        let end = (start + page_size).min(total_hits);
        let hits = if start < end {
            hits[start..end].to_vec()
        } else {
            Vec::new()
        };

        let response = SearchResponse {
            total_hits,
            page,
            page_size,
            total_pages,
            hits,
            execution_time_ms: started.elapsed().as_millis() as u64,
            files_scanned,
            timed_out,
            failed_files,
        };

        Ok(response)
    }

    /// single file search, mainly for test composition
    pub async fn search_file(&self, path: PathBuf, request: &SearchRequest) -> Result<Vec<HitResult>> {
        let (log_parser_config, _) = {
            let cfg = self.config.read().unwrap();
            (cfg.log_parser.clone(), cfg.search.clone())
        };

        let lines = self.reader.read_lines(&path).await?;
        let log_start_pattern = request
            .log_start_pattern
            .as_ref()
            .or(log_parser_config.default_log_start_pattern.as_ref())
            .cloned();
        let log_start_re = if let Some(pat) = &log_start_pattern {
            Some(self.query.compile_regex(pat, true)?)
        } else {
            None
        };

        let mut time_filter = request.time_filter.clone();
        if time_filter.is_none() {
            if let Some(ts) = &log_parser_config.default_timestamp_regex {
                time_filter = Some(TimeFilter {
                    time_start: None,
                    time_end: None,
                    timestamp_regex: Some(ts.clone()),
                });
            }
        }

        let entries = self
            .parser
            .parse(path.clone(), lines, log_start_re)
            .await?;
        // Need to import scan_entries_static or move logic? 
        // Ah, scan_entries_static is likely a private helper function in search.rs. 
        // I need to check if it exists or if I need to implement it.
        // It seems I didn't see it in Read output earlier?
        // Wait, line 244 in Read output calls `scan_entries_static`.
        // So it must exist in the file.
        // I'll just keep the call.
        self.scan_entries(entries, request, time_filter).await
    }

    // Helper to replace scan_entries_static if it was not static method but I need access to self.query
    async fn scan_entries(&self, entries: impl Stream<Item = Result<crate::model::LogEntry>> + Unpin, request: &SearchRequest, time_filter: Option<TimeFilter>) -> Result<Vec<HitResult>> {
         scan_entries_static(&self.query, entries, request, time_filter).await
    }

    fn validate_request(&self, request: &SearchRequest) -> Result<()> {
        let global_cfg = self.config.read().unwrap();
        let has_global = global_cfg.log_sources.log_file_paths.as_ref().map(|v| !v.is_empty()).unwrap_or(false);
        
        if request.scan_config.root_path.as_os_str().is_empty() {
             if !has_global {
                 return Err(crate::error::LogSearchError::InvalidRequest("root_path is empty and no global log_file_paths configured".to_string()));
             }
             // if has global, we skip directory check for root_path
        } else {
            let meta = std::fs::metadata(&request.scan_config.root_path).map_err(|e| {
                crate::error::LogSearchError::FileAccessError {
                    path: request.scan_config.root_path.clone(),
                    reason: e.to_string(),
                }
            })?;
            if !meta.is_dir() {
                return Err(crate::error::LogSearchError::InvalidRequest(format!(
                    "{:?} is not a directory",
                    request.scan_config.root_path
                )));
            }
        }

        if request.page == 0 {
            return Err(crate::error::LogSearchError::InvalidRequest(
                "page must be >= 1".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
struct TaskResult {
    hits: Vec<HitResult>,
    failed: Option<(PathBuf, String)>,
    timed_out: bool,
}

async fn scan_entries_static(
    query: &QueryProcessor,
    mut entries: impl Stream<Item = Result<crate::model::LogEntry>> + Unpin,
    request: &SearchRequest,
    time_filter: Option<TimeFilter>,
) -> Result<Vec<HitResult>> {
    let mut hits = Vec::new();
    while let Some(entry) = entries.next().await {
        let entry = entry?;

        if !query.apply_time_filter(&entry.content, &time_filter) {
            continue;
        }
        if !query.matches(&entry.content, &request.logical_query) {
            continue;
        }

        let positions = collect_positions_static(query, &entry.content, &request.logical_query);
        hits.push(HitResult {
            file_path: entry.file_path.clone(),
            start_line: entry.start_line,
            end_line: entry.end_line,
            content: if request.include_content {
                entry.content.clone()
            } else {
                String::new()
            },
            match_positions: positions,
        });

        if let Some(limit) = request.max_hits {
            if hits.len() >= limit {
                break;
            }
        }
    }
    Ok(hits)
}

fn collect_positions_static(
    query: &QueryProcessor,
    text: &str,
    logical: &crate::model::LogicalQuery,
) -> Vec<MatchPosition> {
    let mut positions = Vec::new();
    for q in logical
        .must
        .iter()
        .chain(logical.any.iter())
        .chain(logical.none.iter())
    {
        positions.extend(query.find_positions(text, q));
    }
    positions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileScanConfig, LogicalQuery, SearchQuery};
    use crate::config::{Config, LogParserConfig, LogSourceConfig, SearchConfig, ServerConfig, ServerMode};
    use tempfile::tempdir;

    fn create_test_engine(buffer_size: usize) -> SearchEngine {
         let mut cfg = Config {
              server: ServerConfig { mode: ServerMode::Stdio, http_addr: None, http_port: None },
              log_parser: LogParserConfig { default_log_start_pattern: None, default_timestamp_regex: None },
              search: SearchConfig::default(),
              log_sources: LogSourceConfig::default(),
         };
         cfg.search.buffer_size = buffer_size;
         SearchEngine::new(Arc::new(RwLock::new(cfg)))
    }

    fn sq(text: &str) -> SearchQuery {
        SearchQuery {
            query: Some(text.to_string()),
            regex: false,
            case_sensitive: false,
            whole_word: false,
        }
    }

    fn base_request(root: PathBuf, logical_query: LogicalQuery) -> SearchRequest {
        SearchRequest {
            scan_config: FileScanConfig {
                root_path: root,
                include_globs: vec!["**/*.log".to_string()],
                exclude_globs: vec![],
            },
            logical_query,
            time_filter: None,
            log_start_pattern: None,
            page_size: 10,
            page: 1,
            max_hits: None,
            hard_timeout_ms: None,
            include_content: true,
        }
    }

    #[tokio::test]
    async fn single_line_search_respects_must_and_none() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("demo.log");
        std::fs::write(
            &path,
            "ok line\nerror traffic\nfatal error\n",
        )
        .unwrap();

        let logical = LogicalQuery {
            must: vec![sq("error")],
            any: vec![],
            none: vec![sq("fatal")],
        };
        let req = base_request(dir.path().to_path_buf(), logical);
        let engine = create_test_engine(32 * 1024);

        let hits = engine.search_file(path, &req).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].content.contains("error traffic"));
    }

    #[tokio::test]
    async fn multiline_search_aggregates_entries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("multi.log");
        std::fs::write(
            &path,
            "2025-01-01 INFO start\nline-a\n2025-01-01 ERROR boom\nline-b\n",
        )
        .unwrap();

        let logical = LogicalQuery {
            must: vec![sq("ERROR")],
            any: vec![],
            none: vec![],
        };
        let mut req = base_request(dir.path().to_path_buf(), logical);
        req.log_start_pattern = Some(r"^\d{4}-\d{2}-\d{2}".to_string());

        let engine = create_test_engine(32 * 1024);
        let hits = engine.search_file(path, &req).await.unwrap();

        assert_eq!(hits.len(), 1);
        assert!(hits[0].content.starts_with("2025-01-01 ERROR"));
        assert!(hits[0].content.contains("line-b"));
        assert!(hits[0].start_line <= hits[0].end_line);
    }

    #[tokio::test]
    async fn search_invalid_root_returns_error() {
        let root = std::path::PathBuf::from("D:/path/does/not/exist");
        let logical = LogicalQuery {
            must: vec![sq("anything")],
            any: vec![],
            none: vec![],
        };
        let req = SearchRequest {
            scan_config: FileScanConfig {
                root_path: root,
                include_globs: vec!["**/*.log".to_string()],
                exclude_globs: vec![],
            },
            logical_query: logical,
            time_filter: None,
            log_start_pattern: None,
            page_size: 10,
            page: 1,
            max_hits: None,
            hard_timeout_ms: None,
            include_content: true,
        };
        let engine = create_test_engine(32 * 1024);
        let err = engine.search(req).await.unwrap_err().to_string();
        assert!(err.contains("文件访问错误") || err.contains("not a directory"));
    }
}
