use std::path::PathBuf;
use std::time::Instant;

use futures::{stream, Stream, StreamExt};
use tokio::time::{timeout, Duration};
use tracing::{error, warn};

use crate::error::Result;
use crate::model::{HitResult, MatchPosition, SearchRequest, SearchResponse, TimeFilter};
use crate::parser::LogParser;
use crate::query::{QueryProcessor, ParsedTimeFilter};
use crate::reader::FileReader;
use crate::scanner::FileScanner;

use std::sync::{Arc, RwLock};
use crate::config::Config;

fn parse_time_filter(tf: &crate::model::TimeFilter) -> ParsedTimeFilter {
    let parse_dt = |s: &str| -> Option<chrono::DateTime<chrono::Utc>> {
        // 优先尝试 RFC3339 格式
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
            return Some(dt.with_timezone(&chrono::Utc));
        }
        // 尝试用空格代替 T
        let normalized = s.replace('T', " ");
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%d %H:%M:%S") {
             return Some(chrono::DateTime::from_utc(dt, chrono::Utc));
        }
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%d %H:%M:%S%.3f") {
             return Some(chrono::DateTime::from_utc(dt, chrono::Utc));
        }
        None
    };

    ParsedTimeFilter {
        start: tf.time_start.as_deref().and_then(parse_dt),
        end: tf.time_end.as_deref().and_then(parse_dt),
        regex: tf.timestamp_regex.as_deref().and_then(|r| regex::Regex::new(r).ok()),
    }
}

/// 搜索引擎：协调扫描、读取、解析和匹配。
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
        let mut reader = FileReader::new(buffer_size);
        // 如果 is_gzip 为 true，FileReader 会自动处理 gzip。
        // 它通过扩展名检测。日志文件是 .log，但可能是纯文本。
        
        Self {
            reader,
            config,
            scanner: FileScanner::new(),
            parser: LogParser::new(),
            query: QueryProcessor::new(),
        }
    }

    pub fn list_files(&self, config: &crate::model::FileScanConfig) -> Result<Vec<PathBuf>> {
        // 如果需要，合并全局路径，尽管 list_files 通常是显式的。
        // 但如果 config.root_path 为空，我们可能会依赖全局路径。
        // 目前，我们直接传递，但如果我们也想在这里支持全局路径：
        let global_cfg = self.config.read().unwrap();
        let global_paths = global_cfg.log_sources.log_file_paths.clone();
        
        if let Some(paths) = global_paths {
             // 如果扫描器支持显式路径，请使用它们。
             // 目前扫描器仅支持 root_path + globs。
             // 我们需要修改扫描器。
             self.scanner.scan_with_paths(config, &Some(paths))
        } else {
             // 如果没有全局配置，且 root_path 为空，我们返回空列表？
             // 或者尝试扫描 root_path
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

        // 扫描文件
        // 关键调试点：确认是否真的扫描到了文件
        let files = if let Some(paths) = &log_sources.log_file_paths {
             // 如果配置了全局路径，直接使用
             self.scanner.scan_with_paths(&request.scan_config, &Some(paths.clone()))?
        } else {
             self.scanner.scan(&request.scan_config)?
        };
        
        // eprintln!("DEBUG: scanned files count: {}", files.len());
        // for f in &files {
        //    eprintln!("DEBUG: file: {:?}", f);
        // }

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
        if let Some(ref mut tf) = time_filter {
            if tf.timestamp_regex.is_none() {
                tf.timestamp_regex = log_parser_config.default_timestamp_regex.clone();
            }
        } else if let Some(ts) = &log_parser_config.default_timestamp_regex {
             time_filter = Some(TimeFilter {
                 time_start: None,
                 time_end: None,
                 timestamp_regex: Some(ts.clone()),
             });
        }
        let parsed_time_filter = time_filter.as_ref().map(parse_time_filter);

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
            let time_filter = parsed_time_filter.clone();

            async move {
                if let Ok(meta) = std::fs::metadata(&path) {
                    const TEN_GB: u64 = 10 * 1024 * 1024 * 1024;
                    if meta.len() > TEN_GB {
                        warn!("file larger than 10GB: {}", path.display());
                    }
                }

                let single_file = async {
                    // eprintln!("DEBUG: reading file {}", path.display());
                    let lines = reader.read_lines(&path).await?;
                    // eprintln!("DEBUG: read lines ok, parsing...");
                    let entries = parser.parse(path.clone(), lines, log_start_re).await?;
                    // eprintln!("DEBUG: parsing ok, scanning entries...");
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

    /// 单文件搜索，主要用于测试组合
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
        let parsed_time_filter = time_filter.as_ref().map(parse_time_filter);

        let entries = self
            .parser
            .parse(path.clone(), lines, log_start_re)
            .await?;
        self.scan_entries(entries, request, parsed_time_filter).await
    }

    // 如果 scan_entries_static 不是静态方法但我需要访问 self.query，则使用此辅助函数替代
    async fn scan_entries(&self, entries: impl Stream<Item = Result<crate::model::LogEntry>> + Unpin, request: &SearchRequest, time_filter: Option<ParsedTimeFilter>) -> Result<Vec<HitResult>> {
         scan_entries_static(&self.query, entries, request, time_filter).await
    }

    pub fn validate_request(&self, request: &SearchRequest) -> Result<()> {
        let global_cfg = self.config.read().unwrap();
        let has_global = global_cfg.log_sources.log_file_paths.as_ref().map(|v| !v.is_empty()).unwrap_or(false);
        
        if request.scan_config.root_path.as_os_str().is_empty() {
             if !has_global {
                 return Err(crate::error::LogSearchError::InvalidRequest("root_path is empty and no global log_file_paths configured".to_string()));
             }
             // if has global, we skip directory check for root_path
        } else {
            // 如果提供了 root_path，我们需要验证它是否存在
            // 但如果用户传入 "." 这种相对路径，fs::metadata 可能会基于当前工作目录去找
            // MCP server 的工作目录通常是启动它的目录。
            // 
            // 关键修复：如果 request.scan_config.root_path 指向一个不存在的目录，但我们有全局日志配置，
            // 我们应该宽容处理吗？或者，如果它是 ".", 且它存在，就没问题。
            
            let meta_res = std::fs::metadata(&request.scan_config.root_path);
            match meta_res {
                Ok(meta) => {
                     if !meta.is_dir() {
                        return Err(crate::error::LogSearchError::InvalidRequest(format!(
                            "{:?} is not a directory",
                            request.scan_config.root_path
                        )));
                    }
                },
                Err(e) => {
                     // 如果校验失败，但我们有全局配置，且 root_path 看起来像是一个为了绕过必填检查的占位符（比如 "." 但如果当前目录不可读？）
                     // 不过通常 "." 是存在的。
                     // 如果用户传了一个无效路径，报错是合理的。
                     return Err(crate::error::LogSearchError::FileAccessError {
                        path: request.scan_config.root_path.clone(),
                        reason: e.to_string(),
                    });
                }
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
    time_filter: Option<ParsedTimeFilter>,
) -> Result<Vec<HitResult>> {
    let mut hits = Vec::new();
    while let Some(entry) = entries.next().await {
        let entry = entry?;

        // 输出调试信息到 stderr（不会影响 stdout json-rpc）
        // eprintln!("DEBUG: checking entry: {}", entry.content.lines().next().unwrap_or(""));

        if !query.apply_time_filter(&entry.content, &time_filter) {
            // eprintln!("DEBUG: time filter rejected");
            continue;
        }
        if !query.matches(&entry.content, &request.logical_query) {
            // eprintln!("DEBUG: content match rejected");
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
