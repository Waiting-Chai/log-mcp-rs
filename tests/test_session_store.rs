use std::path::PathBuf;

use chrono::Utc;
use log_mcp_rs::session_store::{Config, FileInfo, LogMcpError, SearchRecord, SessionManager};
use tempfile::tempdir;

fn test_config(db_path: PathBuf) -> Config {
    Config {
        db_path,
        max_session_bytes: 1024 * 1024,
        session_ttl_secs: 2, // short for tests
        busy_retry_ms: 50,
        busy_max_retries: 10,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_session_create_and_get_unique() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let mgr = SessionManager::new(test_config(db)).unwrap();

    let mut ids = vec![];
    for _ in 0..10u8 {
        let id = mgr.create_session(Some("hint".to_string()), "UTC".to_string()).await.unwrap();
        ids.push(id);
    }
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 10);

    let sess = mgr.get_session(ids[0].as_str()).await.unwrap();
    assert_eq!(sess.id, ids[0]);
    assert_eq!(sess.tz, "UTC");
    assert_eq!(sess.hint.as_deref(), Some("hint"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_access_no_conflict() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let mgr = SessionManager::new(test_config(db)).unwrap();

    let sid = mgr.create_session(None, "UTC".to_string()).await.unwrap();

    let mut handles = vec![];
    for i in 0..20u32 {
        let mgr_ref = mgr.clone();
        let sidc = sid.clone();
        handles.push(tokio::spawn(async move {
            let key = format!("k{}", i);
            let val = format!("v{}", i);
            mgr_ref.set_memory(&sidc, &key, &val).await.unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    let sess = mgr.get_session(&sid).await.unwrap();
    assert!(sess.memories.len() >= 20);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_set_and_remove_memory_persistence() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let mgr = SessionManager::new(test_config(db)).unwrap();
    let sid = mgr.create_session(None, "UTC".to_string()).await.unwrap();

    mgr.set_memory(&sid, "foo", "bar").await.unwrap();
    let sess = mgr.get_session(&sid).await.unwrap();
    assert!(sess.memories.iter().any(|m| m.key == "foo" && m.value == "bar"));

    mgr.remove_memory(&sid, "foo").await.unwrap();
    let sess2 = mgr.get_session(&sid).await.unwrap();
    assert!(!sess2.memories.iter().any(|m| m.key == "foo"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_quota_exceeded_on_files() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let mut cfg = test_config(db);
    cfg.max_session_bytes = 100; // small quota
    let mgr = SessionManager::new(cfg).unwrap();

    let sid = mgr.create_session(None, "UTC".to_string()).await.unwrap();

    let files1 = vec![FileInfo { path: "a.log".into(), size_bytes: 80, checksum: None, added_at: Utc::now() }];
    mgr.add_files(&sid, files1).await.unwrap();

    let files2 = vec![FileInfo { path: "b.log".into(), size_bytes: 30, checksum: None, added_at: Utc::now() }];
    let err = mgr.add_files(&sid, files2).await.err().expect("should exceed quota");
    match err {
        LogMcpError::QuotaExceeded(s) => assert_eq!(s, sid),
        e => panic!("unexpected error: {:?}", e),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_ttl_cleanup() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let mut cfg = test_config(db);
    cfg.session_ttl_secs = 1; // expire quickly
    let mgr = SessionManager::new(cfg).unwrap();

    let sid = mgr.create_session(None, "UTC".to_string()).await.unwrap();
    // Access it to set last_access_ts, then sleep to exceed TTL
    let _ = mgr.get_session(&sid).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let cleaned = mgr.cleanup_expired().await.unwrap();
    assert!(cleaned >= 1);

    let not_found = mgr.get_session(&sid).await.err().unwrap();
    match not_found {
        LogMcpError::SessionNotFound(s) => assert_eq!(s, sid),
        e => panic!("unexpected error: {:?}", e),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_add_search_record() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let mgr = SessionManager::new(test_config(db)).unwrap();

    let sid = mgr.create_session(None, "UTC".to_string()).await.unwrap();
    let rec = SearchRecord { query_json: "{\"q\":\"error\"}".into(), result_count: 42, duration_ms: 12, ts: Utc::now() };
    mgr.add_search_record(&sid, rec).await.unwrap();
}
