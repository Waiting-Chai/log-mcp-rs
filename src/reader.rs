use std::path::Path;

use async_compression::tokio::bufread::GzipDecoder;
use async_stream::try_stream;
use chardetng::EncodingDetector;
use encoding_rs::Encoding;
use futures::stream::{self, BoxStream};
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, BufReader};
use tokio::io::SeekFrom;

use crate::error::{LogSearchError, Result};

/// File reader: stream lines with auto encoding detection and gzip support.
#[derive(Clone)]
pub struct FileReader {
    pub buffer_size: usize,
}

impl FileReader {
    pub fn new(buffer_size: usize) -> Self {
        Self { buffer_size }
    }

    /// Stream text lines with auto encoding detection; gz files decoded as UTF-8.
    pub async fn read_lines(&self, path: &Path) -> Result<BoxStream<'static, Result<String>>> {
        if is_gz(path) {
            return self.read_gzip_lines(path).await;
        }
        let mut file = File::open(path).await.map_err(LogSearchError::from)?;
        let encoding = self.detect_encoding(&mut file).await?;
        if encoding == encoding_rs::UTF_16LE || encoding == encoding_rs::UTF_16BE {
            let mut buf = Vec::new();
            file.read_to_end(&mut buf).await?;
            let (cow, _, _) = encoding.decode(&buf);
            let content = cow.into_owned();
            let lines: Vec<String> = content
                .split_inclusive('\n')
                .map(|s| s.to_string())
                .collect();
            return Ok(Box::pin(stream::iter(lines.into_iter().map(Ok))));
        }

        let reader = BufReader::with_capacity(self.buffer_size, file);
        let stream = try_stream! {
            let mut reader = reader;
            let mut buf = Vec::new();
            loop {
                buf.clear();
                let n = reader.read_until(b'\n', &mut buf).await?;
                if n == 0 {
                    break;
                }
                let (cow, _, _) = encoding.decode(&buf);
                yield cow.into_owned();
            }
        };
        Ok(Box::pin(stream))
    }

    async fn read_gzip_lines(&self, path: &Path) -> Result<BoxStream<'static, Result<String>>> {
        let file = File::open(path).await.map_err(LogSearchError::from)?;
        let reader = BufReader::with_capacity(self.buffer_size, file);
        let decoder = GzipDecoder::new(reader);
        let mut decoder = BufReader::with_capacity(self.buffer_size, decoder);
        let path_buf = path.to_path_buf();

        let stream = try_stream! {
            let mut buf = Vec::new();
            loop {
                buf.clear();
                let n = decoder.read_until(b'\n', &mut buf).await?;
                if n == 0 {
                    break;
                }
                let line = String::from_utf8(buf.clone()).map_err(|e| LogSearchError::EncodingError { path: path_buf.clone(), reason: e.to_string() })?;
                yield line;
            }
        };
        Ok(Box::pin(stream))
    }

    /// Detect file encoding, defaulting to UTF-8. Repositions file cursor after any BOM.
    async fn detect_encoding(&self, file: &mut File) -> Result<&'static Encoding> {
        let mut buf = vec![0u8; 8192];
        let read = file.read(&mut buf).await?;
        let (encoding, bom_len) = detect_from_prefix(&buf[..read]);
        file.seek(SeekFrom::Start(bom_len as u64)).await?;
        Ok(encoding)
    }
}

fn is_gz(path: &Path) -> bool {
    matches!(path.extension().and_then(|s| s.to_str()), Some("gz"))
}

fn detect_from_prefix(prefix: &[u8]) -> (&'static Encoding, usize) {
    if prefix.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return (encoding_rs::UTF_8, 3);
    }
    if prefix.starts_with(&[0xFF, 0xFE]) {
        return (encoding_rs::UTF_16LE, 2);
    }
    if prefix.starts_with(&[0xFE, 0xFF]) {
        return (encoding_rs::UTF_16BE, 2);
    }

    let mut detector = EncodingDetector::new();
    detector.feed(prefix, true);
    (detector.guess(None, true), 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};
    use futures::StreamExt;
    use std::io::Write;
    use tempfile::tempdir;

    #[tokio::test]
    async fn read_utf8_lines() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sample.log");
        std::fs::write(&path, "first\nsecond\n").unwrap();

        let reader = FileReader::new(16 * 1024);
        let mut stream = reader.read_lines(&path).await.unwrap();
        let mut lines = Vec::new();
        while let Some(line) = stream.next().await {
            lines.push(line.unwrap());
        }

        assert_eq!(lines, vec!["first\n", "second\n"]);
    }

    #[tokio::test]
    async fn read_gzip_lines() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sample.log.gz");
        {
            let file = std::fs::File::create(&path).unwrap();
            let mut enc = GzEncoder::new(file, Compression::default());
            enc.write_all(b"gz-line-1\n gz-line-2\n").unwrap();
            enc.finish().unwrap();
        }

        let reader = FileReader::new(16 * 1024);
        let mut stream = reader.read_lines(&path).await.unwrap();
        let mut lines = Vec::new();
        while let Some(line) = stream.next().await {
            lines.push(line.unwrap());
        }

        assert_eq!(lines, vec!["gz-line-1\n", " gz-line-2\n"]);
    }

    #[tokio::test]
    async fn detect_utf16_with_bom() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("utf16.log");
        let content = "你好UTF16\n第二行\n";
        let mut bytes = vec![0xFF, 0xFE];
        for u in content.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        std::fs::write(&path, bytes).unwrap();

        let reader = FileReader::new(16 * 1024);
        let mut stream = reader.read_lines(&path).await.unwrap();
        let mut lines = Vec::new();
        while let Some(line) = stream.next().await {
            lines.push(line.unwrap());
        }

        assert_eq!(lines, vec!["你好UTF16\n", "第二行\n"]);
    }
}
