use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;

use flate2::write::GzEncoder;
use flate2::Compression;
use log_mcp_rs::config::{Config, FsConfig};
use log_mcp_rs::error::LogMcpError;
use log_mcp_rs::file_manager::{CompressionType, FileManager};
use tempfile::tempdir;

fn cfg_with_root(root: PathBuf, io_mode: &str, deny: Vec<String>) -> Config {
    Config { fs: FsConfig { allow_roots: vec![root], deny_patterns: deny, io_mode: io_mode.to_string() } }
}

#[tokio::test]
async fn test_glob_match_and_filter() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let a = root.join("a.log");
    let b = root.join("b.txt");
    let c = root.join("deny.log");
    fs::write(&a, b"2025-11-13 08:14:22 ok\n").unwrap();
    fs::write(&b, b"hello\n").unwrap();
    fs::write(&c, b"deny\n").unwrap();

    let cfg = cfg_with_root(root.clone(), "auto", vec!["**/deny*".to_string()]);
    let fm = FileManager::new(cfg);
    let files = fm.scan_directory(root.to_str().unwrap(), &["**/*.log".to_string()]).unwrap();
    assert_eq!(files.len(), 1);
    assert!(files[0].path.ends_with("a.log"));
}

#[tokio::test]
async fn test_encoding_detection_utf8_vs_gbk() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let utf8 = root.join("utf8.log");
    let gbk = root.join("gbk.log");

    fs::write(&utf8, "错误: 失败 at 2025-11-13 08:14:22\n".as_bytes()).unwrap();

    // encode GBK
    let encoding = encoding_rs::Encoding::for_label(b"gbk").unwrap();
    let (bytes, _, _) = encoding.encode("错误: 失败 at 2025-11-13 08:14:22\n");
    fs::write(&gbk, bytes).unwrap();

    let cfg = cfg_with_root(root.clone(), "auto", vec![]);
    let fm = FileManager::new(cfg);

    let info1 = fm.inspect_file(&utf8).unwrap();
    let info2 = fm.inspect_file(&gbk).unwrap();
    assert_eq!(info1.encoding, "UTF-8");
    assert_eq!(info2.encoding, "GBK");
    assert!(info1.timestamp_examples.len() >= 1);
}

#[tokio::test]
async fn test_gzip_decompress_and_verify() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let gz = root.join("a.log.gz");

    let mut enc = GzEncoder::new(File::create(&gz).unwrap(), Compression::default());
    let payload = b"2025-11-13T08:14:22Z hello gzip\n";
    enc.write_all(payload).unwrap();
    enc.finish().unwrap();

    let cfg = cfg_with_root(root.clone(), "auto", vec![]);
    let fm = FileManager::new(cfg);
    let info = fm.inspect_file(&gz).unwrap();
    assert_eq!(info.compression, CompressionType::Gzip);
    let mapped = fm.map_file(&gz).unwrap();
    let bytes = mapped.read_bytes().unwrap();
    assert_eq!(bytes, payload);
}

#[tokio::test]
async fn test_mmap_mode_read() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let p = root.join("mmap.log");
    let payload = b"line1\nline2\n";
    fs::write(&p, payload).unwrap();
    let cfg = cfg_with_root(root.clone(), "mmap", vec![]);
    let fm = FileManager::new(cfg);
    let mapped = fm.map_file(&p).unwrap();
    assert!(mapped.mmap.is_some());
    let bytes = mapped.read_bytes().unwrap();
    assert_eq!(bytes, payload);
}

#[tokio::test]
async fn test_deny_patterns_on_map_file() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let p = root.join("secret.log");
    fs::write(&p, b"secret\n").unwrap();
    let cfg = cfg_with_root(root.clone(), "auto", vec!["**/secret.*".to_string()]);
    let fm = FileManager::new(cfg);
    let err = fm.map_file(&p).err().unwrap();
    match err { LogMcpError::FileDenied(_) => {}, _ => panic!("expected FileDenied, got {:?}", err) }
}

#[test]
fn test_scan_and_inspect_basic() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("a.log");
    fs::write(&path, "2025-11-14 09:00:00 INFO hello").unwrap();

    let cfg = Config {
        fs: FsConfig {
            allow_roots: vec![dir.path().to_path_buf()],
            deny_patterns: vec![],
            io_mode: "auto".into(),
        },
    };
    let mgr = FileManager::new(cfg);
    let files = mgr.scan_directory(dir.path().to_str().unwrap(), &["*.log".to_string()]).unwrap();
    assert_eq!(files.len(), 1);
    let info = &files[0];
    assert_eq!(info.compression, CompressionType::None);
    assert!(info.timestamp_examples.len() >= 1);
}

#[test]
fn test_gzip_file_inspection() {
    let dir = tempdir().unwrap();
    let gz_path = dir.path().join("b.log.gz");
    let mut gz = GzEncoder::new(File::create(&gz_path).unwrap(), Compression::default());
    gz.write_all(b"2025-11-14T09:00:00Z something").unwrap();
    gz.finish().unwrap();

    let cfg = Config {
        fs: FsConfig {
            allow_roots: vec![dir.path().to_path_buf()],
            deny_patterns: vec![],
            io_mode: "auto".into(),
        },
    };
    let mgr = FileManager::new(cfg);
    let info = mgr.inspect_file(&gz_path).unwrap();
    assert_eq!(info.compression, CompressionType::Gzip);
    assert!(info.timestamp_examples.len() >= 1);
}
