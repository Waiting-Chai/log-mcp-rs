use std::path::PathBuf;

use async_stream::try_stream;
use futures::{stream::BoxStream, StreamExt, TryStreamExt};
use regex::Regex;

use crate::error::Result;
use crate::model::LogEntry;

/// 日志解析器：根据 log_start_pattern 决定单行解析还是多行聚合。
#[derive(Clone, Default)]
pub struct LogParser;

impl LogParser {
    pub fn new() -> Self {
        Self
    }

    pub async fn parse(
        &self,
        file_path: PathBuf,
        lines: BoxStream<'static, Result<String>>,
        log_start_pattern: Option<Regex>,
    ) -> Result<BoxStream<'static, Result<LogEntry>>> {
        let stream = if let Some(re) = log_start_pattern {
            self.parse_multiline(file_path, lines, re).await
        } else {
            self.parse_single_line(file_path, lines).await
        };
        Ok(stream)
    }

    async fn parse_single_line(
        &self,
        file_path: PathBuf,
        mut lines: BoxStream<'static, Result<String>>,
    ) -> BoxStream<'static, Result<LogEntry>> {
        let stream = try_stream! {
            let mut line_no: usize = 0;
            while let Some(line) = lines.next().await {
                let line = line?;
                line_no += 1;
                yield LogEntry {
                    file_path: file_path.clone(),
                    start_line: line_no,
                    end_line: line_no,
                    content: line,
                };
            }
        };
        Box::pin(stream)
    }

    async fn parse_multiline(
        &self,
        file_path: PathBuf,
        mut lines: BoxStream<'static, Result<String>>,
        start_re: Regex,
    ) -> BoxStream<'static, Result<LogEntry>> {
        let stream = try_stream! {
            let mut line_no: usize = 0;
            let mut current_start: usize = 1;
            let mut current_end: usize = 0;
            let mut buf: Vec<String> = Vec::new();

            while let Some(line) = lines.try_next().await? {
                line_no += 1;
                let is_start = start_re.is_match(&line);
                if is_start {
                    if !buf.is_empty() {
                        let content = buf.join("");
                        yield LogEntry {
                            file_path: file_path.clone(),
                            start_line: current_start,
                            end_line: current_end,
                            content,
                        };
                        buf.clear();
                    }
                    current_start = line_no;
                    current_end = line_no;
                    buf.push(line);
                } else {
                    if buf.is_empty() {
                        // 尚未匹配到开始模式；开始一个新条目以保留每一行。
                        current_start = line_no;
                    }
                    current_end = line_no;
                    buf.push(line);
                }
            }

            if !buf.is_empty() {
                let content = buf.join("");
                yield LogEntry {
                    file_path: file_path.clone(),
                    start_line: current_start,
                    end_line: current_end,
                    content,
                };
            }
        };
        Box::pin(stream)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use futures::StreamExt;

    use super::LogParser;
    use crate::reader::FileReader;

    #[tokio::test]
    async fn parse_example_log_by_timestamp() {
        let reader = FileReader::new(64 * 1024);
        let path = Path::new("example.log");
        let lines = reader.read_lines(path).await.unwrap();

        let start_re = regex::Regex::new(
            r"^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3}\s+\w+",
        )
        .unwrap();

        let mut entries = LogParser::new()
            .parse(path.to_path_buf(), lines, Some(start_re))
            .await
            .unwrap();

        let mut collected = Vec::new();
        while let Some(entry) = entries.next().await {
            collected.push(entry.unwrap());
        }

        assert_eq!(collected.len(), 4);
        assert_eq!((collected[0].start_line, collected[0].end_line), (1, 109));
        assert_eq!((collected[1].start_line, collected[1].end_line), (110, 110));
        assert_eq!((collected[2].start_line, collected[2].end_line), (111, 111));
        assert_eq!((collected[3].start_line, collected[3].end_line), (112, 113));

        assert!(collected[0]
            .content
            .starts_with("2025-11-18 09:46:17.544 DEBUG"));
        assert!(collected[3]
            .content
            .contains("init check points success"));
    }
}
