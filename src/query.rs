use std::borrow::Cow;

use chrono::{DateTime, Utc};
use regex::{Regex, RegexBuilder};

use crate::error::Result;
use crate::model::{LogicalQuery, MatchPosition, SearchQuery};

/// 用于高效应用时间过滤器的内部结构
#[derive(Debug, Clone)]
pub struct ParsedTimeFilter {
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
    pub regex: Option<Regex>,
}

/// 查询处理器：文本/正则匹配、逻辑组合和时间过滤。
#[derive(Clone, Default)]
pub struct QueryProcessor;

impl QueryProcessor {
    pub fn new() -> Self {
        Self
    }

    pub fn matches(&self, text: &str, query: &LogicalQuery) -> bool {
        if !query.must.iter().all(|q| self.single_match(text, q)) {
            return false;
        }
        if !query.any.is_empty() && !query.any.iter().any(|q| self.single_match(text, q)) {
            return false;
        }
        if query.none.iter().any(|q| self.single_match(text, q)) {
            return false;
        }
        true
    }

    pub fn find_positions(&self, text: &str, query: &SearchQuery) -> Vec<MatchPosition> {
        if query.query.is_none() {
            return Vec::new();
        }
        let needle = query.query.as_ref().unwrap();
        if query.regex {
            if let Ok(re) = self.compile_regex(needle, query.case_sensitive) {
                return re
                    .find_iter(text)
                    .map(|m| MatchPosition {
                        offset: m.start(),
                        length: m.end() - m.start(),
                    })
                    .collect();
            }
            return Vec::new();
        }

        // 纯文本匹配（可选全字匹配）
        let haystack = if query.case_sensitive {
            Cow::Borrowed(text)
        } else {
            Cow::Owned(text.to_lowercase())
        };
        let keyword = if query.case_sensitive {
            Cow::Borrowed(needle.as_str())
        } else {
            Cow::Owned(needle.to_lowercase())
        };

        if query.whole_word {
            let target = keyword.into_owned();
            let bytes = haystack.as_bytes();
            let needle = target.as_bytes();
            let mut positions = Vec::new();
            let mut idx = 0usize;
            while idx + needle.len() <= bytes.len() {
                if &bytes[idx..idx + needle.len()] == needle {
                    let before_ok = idx == 0 || !is_word(bytes[idx - 1]);
                    let after_ok = idx + needle.len() == bytes.len() || !is_word(bytes[idx + needle.len()]);
                    if before_ok && after_ok {
                        positions.push(MatchPosition {
                            offset: idx,
                            length: needle.len(),
                        });
                    }
                }
                idx += 1;
            }
            return positions;
        }

        let mut positions = Vec::new();
        let mut start = 0usize;
        while let Some(pos) = haystack[start..].find(&*keyword) {
            let abs = start + pos;
            positions.push(MatchPosition {
                offset: abs,
                length: keyword.len(),
            });
            start = abs + keyword.len();
        }
        positions
    }

    pub fn compile_regex(&self, pattern: &str, case_sensitive: bool) -> Result<Regex> {
        let mut builder = RegexBuilder::new(pattern);
        builder.case_insensitive(!case_sensitive);
        builder.build().map_err(|e| crate::error::LogSearchError::RegexError {
            pattern: pattern.to_string(),
            reason: e.to_string(),
        })
    }

    pub fn apply_time_filter(&self, text: &str, filter: &Option<ParsedTimeFilter>) -> bool {
        let Some(filter) = filter else { return true; };
        let Some(re) = &filter.regex else { return true; };
        
        let ts_str = match re.find(text) {
            Some(m) => &text[m.start()..m.end()],
            None => return true, 
        };
        
        // 尝试多种格式解析
        // 优先 RFC3339, 其次常见的日志格式
        let ts = if let Ok(dt) = DateTime::parse_from_rfc3339(ts_str) {
            dt.with_timezone(&Utc)
        } else if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(ts_str, "%Y-%m-%d %H:%M:%S") {
            // 假设是本地时间，或者 UTC
             DateTime::from_utc(dt, Utc)
        } else if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(ts_str, "%Y-%m-%d %H:%M:%S%.3f") {
             DateTime::from_utc(dt, Utc)
        } else {
             // 尝试把 T 换成空格
             let normalized = ts_str.replace('T', " ");
             if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%d %H:%M:%S") {
                 DateTime::from_utc(dt, Utc)
             } else if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%d %H:%M:%S%.3f") {
                 DateTime::from_utc(dt, Utc)
             } else {
                 return true; // 解析失败，默认不过滤
             }
        };

        if let Some(start) = filter.start {
            if ts < start {
                return false;
            }
        }
        if let Some(end) = filter.end {
            if ts > end {
                return false;
            }
        }
        true
    }

    fn single_match(&self, text: &str, query: &SearchQuery) -> bool {
        let Some(pattern) = &query.query else {
            return true;
        };
        if query.regex {
            return self
                .compile_regex(pattern, query.case_sensitive)
                .ok()
                .map(|re| re.is_match(text))
                .unwrap_or(false);
        }

        if query.whole_word {
            let escaped = regex::escape(pattern);
            return RegexBuilder::new(&format!(r"\b{escaped}\b"))
                .case_insensitive(!query.case_sensitive)
                .build()
                .ok()
                .map(|re| re.is_match(text))
                .unwrap_or(false);
        }

        if query.case_sensitive {
            text.contains(pattern)
        } else {
            text.to_lowercase().contains(&pattern.to_lowercase())
        }
    }
}

fn is_word(byte: u8) -> bool {
    let c = byte as char;
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn sq(text: &str) -> SearchQuery {
        SearchQuery {
            query: Some(text.to_string()),
            regex: false,
            case_sensitive: false,
            whole_word: false,
        }
    }

    #[test]
    fn logical_combinations_work() {
        let qp = QueryProcessor::new();
        let query = LogicalQuery {
            must: vec![sq("error")],
            any: vec![sq("traffic"), sq("network")],
            none: vec![sq("fatal")],
        };
        assert!(qp.matches("traffic error occurred", &query));
        assert!(!qp.matches("info traffic ok", &query)); // must not satisfied
        assert!(!qp.matches("traffic fatal error", &query)); // none matched
    }

    #[test]
    fn whole_word_and_regex_positions() {
        let qp = QueryProcessor::new();
        let query = SearchQuery {
            query: Some("err".into()),
            regex: false,
            case_sensitive: false,
            whole_word: true,
        };
        let positions = qp.find_positions("err and terror", &query);
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].offset, 0);

        let re_query = SearchQuery {
            query: Some(r"t[a-z]{3}or".into()),
            regex: true,
            case_sensitive: false,
            whole_word: false,
        };
        let re_pos = qp.find_positions("err and terror", &re_query);
        assert_eq!(re_pos.len(), 1);
        assert_eq!(re_pos[0].offset, 8);
    }

    #[test]
    fn time_filter_respects_range() {
        let qp = QueryProcessor::new();
        let tf = ParsedTimeFilter {
            start: Some(
                Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0)
                    .single()
                    .unwrap(),
            ),
            end: Some(
                Utc.with_ymd_and_hms(2024, 1, 2, 0, 0, 0)
                    .single()
                    .unwrap(),
            ),
            regex: Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z").ok(),
        };
        let log_in = "2024-01-01T12:00:00Z something";
        let log_out = "2024-01-03T00:00:00Z late";
        assert!(qp.apply_time_filter(log_in, &Some(tf.clone())));
        assert!(!qp.apply_time_filter(log_out, &Some(tf)));
    }
}
