use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 日志条目。多行聚合或单行均用该结构承载。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub file_path: PathBuf,
    pub start_line: usize,
    pub end_line: usize,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileScanConfig {
    #[serde(default)]
    pub root_path: PathBuf,
    #[serde(default)]
    pub include_globs: Vec<String>,
    #[serde(default)]
    pub exclude_globs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "SearchQueryInput")]
pub struct SearchQuery {
    pub query: Option<String>,
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SearchQueryInput {
    Simple(String),
    Full {
        query: Option<String>,
        #[serde(default)]
        regex: bool,
        #[serde(default)]
        case_sensitive: bool,
        #[serde(default)]
        whole_word: bool,
    },
}

impl From<SearchQueryInput> for SearchQuery {
    fn from(input: SearchQueryInput) -> Self {
        match input {
            SearchQueryInput::Simple(s) => SearchQuery {
                query: Some(s),
                regex: false,
                case_sensitive: false,
                whole_word: false,
            },
            SearchQueryInput::Full {
                query,
                regex,
                case_sensitive,
                whole_word,
            } => SearchQuery {
                query,
                regex,
                case_sensitive,
                whole_word,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogicalQuery {
    pub must: Vec<SearchQuery>,
    #[serde(default)]
    pub any: Vec<SearchQuery>,
    #[serde(default)]
    pub none: Vec<SearchQuery>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeFilter {
    #[serde(alias = "start_time", alias = "startTime", alias = "after")]
    pub time_start: Option<String>,
    #[serde(alias = "end_time", alias = "endTime", alias = "before")]
    pub time_end: Option<String>,
    pub timestamp_regex: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchPosition {
    pub offset: usize,
    pub length: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub scan_config: FileScanConfig,
    pub logical_query: LogicalQuery,
    pub time_filter: Option<TimeFilter>,
    pub log_start_pattern: Option<String>,
    #[serde(default)]
    pub page_size: usize,
    #[serde(default = "default_page")]
    pub page: usize,
    #[serde(default)]
    pub max_hits: Option<usize>,
    #[serde(default)]
    pub hard_timeout_ms: Option<u64>,
    #[serde(default = "default_include_content")]
    pub include_content: bool,
}

fn default_include_content() -> bool {
    true
}

fn default_page() -> usize {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HitResult {
    pub file_path: PathBuf,
    pub start_line: usize,
    pub end_line: usize,
    pub content: String,
    pub match_positions: Vec<MatchPosition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub total_hits: usize,
    pub page: usize,
    pub page_size: usize,
    pub total_pages: usize,
    pub hits: Vec<HitResult>,
    pub execution_time_ms: u64,
    pub files_scanned: usize,
    pub timed_out: bool,
    pub failed_files: Vec<(PathBuf, String)>,
}
