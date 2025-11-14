use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use chardetng::EncodingDetector;
use encoding_rs::{UTF_16BE, UTF_16LE, UTF_8};
use flate2::read::GzDecoder;
use globset::{Glob, GlobSet, GlobSetBuilder};
use globwalk::GlobWalkerBuilder;
use memmap2::Mmap;
use regex::Regex;

use crate::config::Config;
use crate::error::LogMcpError;

#[derive(Debug, Clone, PartialEq)]
pub enum CompressionType {
    None,
    Gzip,
}

#[derive(Debug, Clone)]
pub struct FileInfo {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub encoding: String,
    pub compression: CompressionType,
    pub timestamp_examples: Vec<String>,
    pub added_at: DateTime<Utc>,
}

pub struct FileManager {
    cfg: Config,
}

impl FileManager {
    pub fn new(cfg: Config) -> Self { Self { cfg } }

    /// 扫描目录并返回所有匹配的日志文件
    /// - 支持 glob 模式
    /// - 应用 allow_roots 与 deny_patterns
    pub fn scan_directory(&self, root: &str, patterns: &[String]) -> Result<Vec<FileInfo>, LogMcpError> {
        let allow_roots = canonical_roots(&self.cfg)?;
        let root_abs = fs::canonicalize(root).map_err(|e| LogMcpError::IOError(e.to_string()))?;
        if !allow_roots.iter().any(|r| root_abs.starts_with(r)) {
            return Err(LogMcpError::FileDenied(format!("root not allowed: {}", root_abs.display())));
        }

        let deny = compile_globset(&self.cfg.fs.deny_patterns)?;

        let mut out = Vec::new();
        let walker = GlobWalkerBuilder::from_patterns(&root_abs, patterns)
            .follow_links(false)
            .case_insensitive(true)
            .build()
            .map_err(|e| LogMcpError::InvalidInput(e.to_string()))?;

        for entry in walker.filter_map(Result::ok).filter(|e| e.file_type().is_file()) {
            let path = entry.path().to_path_buf();
            if !is_path_allowed(&path, &allow_roots, &deny)? {
                continue;
            }
            let info = self.inspect_file(&path)?;
            out.push(info);
        }
        Ok(out)
    }

    /// 检测文件编码、压缩类型、时间戳示例
    pub fn inspect_file(&self, path: &PathBuf) -> Result<FileInfo, LogMcpError> {
        let allow_roots = canonical_roots(&self.cfg)?;
        let deny = compile_globset(&self.cfg.fs.deny_patterns)?;
        if !is_path_allowed(path, &allow_roots, &deny)? {
            return Err(LogMcpError::FileDenied(format!("denied: {}", path.display())));
        }

        let meta = fs::symlink_metadata(path).map_err(LogMcpError::from)?;
        if meta.file_type().is_symlink() {
            return Err(LogMcpError::FileDenied(format!("symlink not allowed: {}", path.display())));
        }
        if !meta.is_file() {
            return Err(LogMcpError::InvalidInput(format!("not a file: {}", path.display())));
        }

        let size_bytes = meta.len();
        let compression = if is_gzip_path(path) { CompressionType::Gzip } else { CompressionType::None };

        let sample = if compression == CompressionType::Gzip {
            let f = File::open(path)?;
            let mut gz = GzDecoder::new(f);
            let mut buf = vec![0u8; 4096];
            let n = gz.read(&mut buf).unwrap_or(0);
            buf.truncate(n);
            buf
        } else {
            let mut f = File::open(path)?;
            let mut buf = vec![0u8; 4096];
            let n = f.read(&mut buf).unwrap_or(0);
            buf.truncate(n);
            buf
        };

        let (encoding_label, decoded_sample) = detect_and_decode(&sample)?;
        let timestamp_examples = extract_timestamps(&decoded_sample);

        Ok(FileInfo {
            path: path.clone(),
            size_bytes,
            encoding: encoding_label,
            compression,
            timestamp_examples,
            added_at: Utc::now(),
        })
    }

    /// 打开并映射文件（mmap 或 buffered）
    pub fn map_file(&self, path: &PathBuf) -> Result<MappedFile, LogMcpError> {
        let info = self.inspect_file(path)?;

        if info.compression == CompressionType::Gzip {
            let f = File::open(path)?;
            let mut gz = GzDecoder::new(f);
            let mut out = Vec::new();
            let mut buf = [0u8; 16 * 1024];
            loop {
                let n = gz.read(&mut buf)?;
                if n == 0 { break; }
                out.extend_from_slice(&buf[..n]);
            }
            return Ok(MappedFile { path: path.clone(), mmap: None, buffer: Some(out), encoding: info.encoding, compression: info.compression });
        }

        let io_mode = self.cfg.fs.io_mode.as_str();
        match io_mode {
            "buffered" => {
                let data = fs::read(path)?;
                Ok(MappedFile { path: path.clone(), mmap: None, buffer: Some(data), encoding: info.encoding, compression: info.compression })
            }
            "mmap" => map_regular_file(path, info.encoding, info.compression),
            _ => {
                // auto: prefer mmap, fallback to buffered
                match map_regular_file(path, info.encoding.clone(), info.compression.clone()) {
                    Ok(m) => Ok(m),
                    Err(_) => {
                        let data = fs::read(path)?;
                        Ok(MappedFile { path: path.clone(), mmap: None, buffer: Some(data), encoding: info.encoding, compression: info.compression })
                    }
                }
            }
        }
    }
}

pub struct MappedFile {
    pub path: PathBuf,
    pub mmap: Option<Mmap>,
    pub buffer: Option<Vec<u8>>,
    pub encoding: String,
    pub compression: CompressionType,
}

impl MappedFile {
    /// 返回解压后的原始字节流（零拷贝或解压缓存）
    pub fn read_bytes(&self) -> Result<Vec<u8>, LogMcpError> {
        if let Some(buf) = &self.buffer { return Ok(buf.clone()); }
        if let Some(mmap) = &self.mmap { return Ok((&mmap[..]).to_vec()); }
        Err(LogMcpError::Internal(format!("no data: {}", self.path.display())))
    }
}

fn map_regular_file(path: &PathBuf, encoding: String, compression: CompressionType) -> Result<MappedFile, LogMcpError> {
    let file = OpenOptions::new().read(true).open(path)?;
    // Ensure not a symlink
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(LogMcpError::FileDenied(format!("symlink not allowed: {}", path.display())));
    }
    unsafe {
        let mmap = Mmap::map(&file).map_err(|e| LogMcpError::IOError(e.to_string()))?;
        Ok(MappedFile { path: path.clone(), mmap: Some(mmap), buffer: None, encoding, compression })
    }
}

fn is_gzip_path(path: &Path) -> bool { path.extension().map(|e| e.eq_ignore_ascii_case("gz")).unwrap_or(false) }

fn canonical_roots(cfg: &Config) -> Result<Vec<PathBuf>, LogMcpError> {
    let mut v = Vec::new();
    for r in cfg.fs.allow_roots.iter() {
        let c = fs::canonicalize(r).map_err(|e| LogMcpError::IOError(e.to_string()))?;
        v.push(c);
    }
    Ok(v)
}

fn compile_globset(patterns: &[String]) -> Result<GlobSet, LogMcpError> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        let g = Glob::new(p).map_err(|e| LogMcpError::InvalidInput(e.to_string()))?;
        b.add(g);
    }
    b.build().map_err(|e| LogMcpError::InvalidInput(e.to_string()))
}

fn is_path_allowed(path: &PathBuf, allow_roots: &[PathBuf], deny: &GlobSet) -> Result<bool, LogMcpError> {
    let canon = fs::canonicalize(path).map_err(|e| LogMcpError::IOError(e.to_string()))?;
    if !allow_roots.iter().any(|r| canon.starts_with(r)) { return Ok(false); }
    if deny.is_match(&canon) { return Ok(false); }
    let meta = fs::symlink_metadata(&canon)?;
    if meta.file_type().is_symlink() { return Ok(false); }
    Ok(true)
}

fn detect_and_decode(sample: &[u8]) -> Result<(String, String), LogMcpError> {
    if sample.starts_with(&[0xEF, 0xBB, 0xBF]) {
        let s = UTF_8.decode_without_bom_handling(&sample[3..]).0.into_owned();
        return Ok(("UTF-8".to_string(), s));
    }
    if sample.starts_with(&[0xFF, 0xFE]) {
        let (s, _, _) = UTF_16LE.decode(&sample[2..]);
        return Ok(("UTF-16LE".to_string(), s.into_owned()));
    }
    if sample.starts_with(&[0xFE, 0xFF]) {
        let (s, _, _) = UTF_16BE.decode(&sample[2..]);
        return Ok(("UTF-16BE".to_string(), s.into_owned()));
    }

    let mut det = EncodingDetector::new();
    det.feed(sample, true);
    let enc = det.guess(None, true);
    let label = enc.name();
    let label = if label.eq_ignore_ascii_case("gb18030") || label.eq_ignore_ascii_case("gbk") { "GBK" } else { label };
    let label_string = label.to_string();
    let (s, _, had_errors) = enc.decode(sample);
    let out = if had_errors { String::from_utf8_lossy(sample).into_owned() } else { s.into_owned() };
    Ok((label_string, out))
}

fn extract_timestamps(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let re1 = Regex::new(r"\b\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?\b").unwrap();
    let re2 = Regex::new(r"\b\d{4}/\d{2}/\d{2}[ T]\d{2}:\d{2}:\d{2}\b").unwrap();
    for cap in re1.find_iter(s).take(3) { out.push(cap.as_str().to_string()); }
    if out.len() < 3 {
        for cap in re2.find_iter(s).take(3 - out.len()) { out.push(cap.as_str().to_string()); }
    }
    out
}
