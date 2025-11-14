//! session_store.rs - Session lifecycle management

use std::{
    path::{PathBuf},
    time::Duration,
};

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{error, info, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub db_path: PathBuf,
    pub max_session_bytes: u64,
    pub session_ttl_secs: u64,
    pub busy_retry_ms: u64,
    pub busy_max_retries: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            db_path: PathBuf::from("./log_mcp.sqlite"),
            max_session_bytes: 5 * 1024 * 1024 * 1024, // 5GB
            session_ttl_secs: 7 * 24 * 60 * 60,        // 7 days
            busy_retry_ms: 100,
            busy_max_retries: 5,
        }
    }
}

#[derive(Debug, Error)]
pub enum LogMcpError {
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("quota exceeded for session {0}")]
    QuotaExceeded(String),
    #[error("database error: {0}")]
    DatabaseError(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("file denied: {0}")]
    FileDenied(String),
    #[error("invalid encoding: {0}")]
    InvalidEncoding(String),
    #[error("io error: {0}")]
    IOError(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<rusqlite::Error> for LogMcpError {
    fn from(e: rusqlite::Error) -> Self {
        LogMcpError::DatabaseError(e.to_string())
    }
}

impl From<std::io::Error> for LogMcpError {
    fn from(e: std::io::Error) -> Self { LogMcpError::IOError(e.to_string()) }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub tz: String,
    pub hint: Option<String>,
    pub files: Vec<FileInfo>,
    pub memories: Vec<Memory>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub path: String,
    pub size_bytes: u64,
    pub checksum: Option<String>,
    pub added_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRecord {
    pub query_json: String,
    pub result_count: i64,
    pub duration_ms: i64,
    pub ts: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub key: String,
    pub value: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    pub fact_json: String,
    pub ts: DateTime<Utc>,
}

#[derive(Clone)]
pub struct SessionManager {
    db_path: PathBuf,
    config: Config,
}

impl SessionManager {
    pub fn new(config: Config) -> Result<Self, LogMcpError> {
        let mgr = Self { db_path: config.db_path.clone(), config };
        mgr.init_db()?;
        Ok(mgr)
    }

    fn open_conn(&self) -> Result<Connection, LogMcpError> {
        let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
        flags.set(OpenFlags::SQLITE_OPEN_FULL_MUTEX, true);
        let conn = Connection::open_with_flags(&self.db_path, flags)?;
        conn.pragma_update(None, "journal_mode", &"WAL")
            .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
        conn.pragma_update(None, "synchronous", &"NORMAL")
            .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
        conn.busy_timeout(Duration::from_millis(5000))
            .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
        conn.pragma_update(None, "foreign_keys", &"ON")
            .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
        Ok(conn)
    }

    fn init_db(&self) -> Result<(), LogMcpError> {
        let conn = self.open_conn()?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                created_at INTEGER NOT NULL,
                tz TEXT NOT NULL,
                hint TEXT,
                last_access_ts INTEGER NOT NULL,
                owner TEXT
            );
            CREATE TABLE IF NOT EXISTS session_files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                path TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                checksum TEXT,
                added_at INTEGER NOT NULL,
                FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_files_session ON session_files(session_id);

            CREATE TABLE IF NOT EXISTS search_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                query_json TEXT NOT NULL,
                result_count INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL,
                ts INTEGER NOT NULL,
                FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_search_records_session ON search_records(session_id, ts DESC);

            CREATE TABLE IF NOT EXISTS search_hits (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                record_id INTEGER NOT NULL,
                file_path TEXT NOT NULL,
                line_no INTEGER,
                excerpt TEXT,
                ts INTEGER NOT NULL,
                FOREIGN KEY(record_id) REFERENCES search_records(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_hits_record ON search_hits(record_id);
            CREATE INDEX IF NOT EXISTS idx_hits_ts ON search_hits(ts);

            CREATE TABLE IF NOT EXISTS memories (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                UNIQUE(session_id, key),
                FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_memories_session ON memories(session_id);

            CREATE TABLE IF NOT EXISTS facts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                fact_json TEXT NOT NULL,
                ts INTEGER NOT NULL,
                FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_facts_session ON facts(session_id);
            "#,
        )?;
        Ok(())
    }

    async fn run_with_retry<F, T>(&self, mut f: F) -> Result<T, LogMcpError>
    where
        F: FnMut() -> Result<T, LogMcpError> + Send + 'static,
        T: Send + 'static,
    {
        let retries = self.config.busy_max_retries;
        let delay = Duration::from_millis(self.config.busy_retry_ms);
        tokio::task::spawn_blocking(move || {
            let mut attempt = 0;
            loop {
                match f() {
                    Ok(v) => return Ok(v),
                    Err(LogMcpError::DatabaseError(msg)) => {
                        let busy = msg.contains("database is locked") || msg.contains("busy");
                        if busy && attempt < retries {
                            attempt += 1;
                            std::thread::sleep(delay);
                            continue;
                        }
                        return Err(LogMcpError::DatabaseError(msg));
                    }
                    Err(e) => return Err(e),
                }
            }
        })
        .await
        .map_err(|e| LogMcpError::Internal(e.to_string()))?
    }

    pub async fn create_session(&self, hint: Option<String>, tz: String) -> Result<String, LogMcpError> {
        let db_path = self.db_path.clone();
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().timestamp();
        let hint_clone = hint.clone();
        let id_clone = id.clone();
        self
            .run_with_retry(move || {
                let conn = Connection::open(db_path.as_path()).map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                conn.pragma_update(None, "journal_mode", &"WAL").map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                let mut stmt = conn
                    .prepare("INSERT INTO sessions (id, created_at, tz, hint, last_access_ts) VALUES (?1, ?2, ?3, ?4, ?5)")
                    .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                stmt.execute(params![id_clone, now, tz, hint_clone, now])
                    .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                Ok(())
            })
            .await?;
        info!(session_id = %id, "session created");
        Ok(id)
    }

    pub async fn get_session(&self, id: &str) -> Result<Session, LogMcpError> {
        let id_s = id.to_string();
        let db_path = self.db_path.clone();
        self
            .run_with_retry(move || {
                let conn = Connection::open(db_path.as_path()).map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                conn.pragma_update(None, "journal_mode", &"WAL").map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;

                let mut stmt = conn
                    .prepare("SELECT id, created_at, tz, hint FROM sessions WHERE id = ?1")
                    .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                let mut rows = stmt.query(params![id_s.as_str()]).map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                let row = rows.next().map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                let row = match row {
                    Some(r) => r,
                    None => return Err(LogMcpError::SessionNotFound(id_s.clone())),
                };
                let created_at: i64 = row.get(1).map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                let tz: String = row.get(2).map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                let hint: Option<String> = row.get(3).map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;

                conn.execute(
                    "UPDATE sessions SET last_access_ts = ?2 WHERE id = ?1",
                    params![id_s.as_str(), Utc::now().timestamp()],
                )
                .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;

                let mut files_stmt = conn
                    .prepare("SELECT path, size_bytes, checksum, added_at FROM session_files WHERE session_id = ?1 ORDER BY id ASC")
                    .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                let files = files_stmt
                    .query_map(params![id_s.as_str()], |row| {
                        let ts: i64 = row.get(3)?;
                        Ok(FileInfo {
                            path: row.get(0)?,
                            size_bytes: row.get::<_, i64>(1)? as u64,
                            checksum: row.get(2).ok(),
                            added_at: Utc.timestamp_opt(ts, 0).single().unwrap_or_else(Utc::now),
                        })
                    })
                    .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;

                let mut mem_stmt = conn
                    .prepare("SELECT key, value, updated_at FROM memories WHERE session_id = ?1 ORDER BY id ASC")
                    .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                let memories = mem_stmt
                    .query_map(params![id_s.as_str()], |row| {
                        let ts: i64 = row.get(2)?;
                        Ok(Memory {
                            key: row.get(0)?,
                            value: row.get(1)?,
                            updated_at: Utc.timestamp_opt(ts, 0).single().unwrap_or_else(Utc::now),
                        })
                    })
                    .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;

                Ok(Session {
                    id: id_s.clone(),
                    created_at: Utc.timestamp_opt(created_at, 0).single().unwrap_or_else(Utc::now),
                    tz,
                    hint,
                    files,
                    memories,
                })
            })
            .await
    }

    pub async fn add_files(&self, session_id: &str, files: Vec<FileInfo>) -> Result<(), LogMcpError> {
        if files.is_empty() {
            return Ok(());
        }
        let sid = session_id.to_string();
        let db_path = self.db_path.clone();
        let cfg = self.config.clone();
        self
            .run_with_retry(move || {
                let mut conn = Connection::open(db_path.as_path()).map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                conn.pragma_update(None, "journal_mode", &"WAL").map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;

                let exists: Option<String> = conn
                    .query_row(
                        "SELECT id FROM sessions WHERE id = ?1",
                        params![sid.as_str()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                if exists.is_none() {
                    return Err(LogMcpError::SessionNotFound(sid.clone()));
                }

                let current_bytes: i64 = conn
                    .query_row(
                        "SELECT COALESCE(SUM(size_bytes),0) FROM session_files WHERE session_id = ?1",
                        params![sid.as_str()],
                        |row| row.get(0),
                    )
                    .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                let new_bytes: u64 = files.iter().map(|f| f.size_bytes).sum();
                let total = current_bytes.max(0) as u64 + new_bytes;
                if total > cfg.max_session_bytes {
                    warn!(session_id = %sid, total_bytes = total, "quota exceeded on add_files");
                    return Err(LogMcpError::QuotaExceeded(sid.clone()));
                }

                let tx = conn.transaction().map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                {
                    let mut stmt = tx
                        .prepare("INSERT INTO session_files (session_id, path, size_bytes, checksum, added_at) VALUES (?1, ?2, ?3, ?4, ?5)")
                        .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                    for f in files.iter() {
                        stmt.execute(params![sid.as_str(), &f.path, f.size_bytes as i64, &f.checksum, f.added_at.timestamp()])
                            .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                    }
                }
                tx.commit().map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                Ok(())
            })
            .await
    }

    pub async fn add_search_record(&self, session_id: &str, record: SearchRecord) -> Result<(), LogMcpError> {
        let sid = session_id.to_string();
        let db_path = self.db_path.clone();
        self
            .run_with_retry(move || {
                let conn = Connection::open(db_path.as_path()).map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                conn.pragma_update(None, "journal_mode", &"WAL").map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                let exists: Option<String> = conn
                    .query_row(
                        "SELECT id FROM sessions WHERE id = ?1",
                        params![sid.as_str()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                if exists.is_none() {
                    return Err(LogMcpError::SessionNotFound(sid.clone()));
                }
                let mut stmt = conn
                    .prepare("INSERT INTO search_records (session_id, query_json, result_count, duration_ms, ts) VALUES (?1, ?2, ?3, ?4, ?5)")
                    .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                stmt.execute(params![sid.as_str(), record.query_json, record.result_count, record.duration_ms, record.ts.timestamp()])
                    .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                Ok(())
            })
            .await
    }

    pub async fn set_memory(&self, session_id: &str, key: &str, value: &str) -> Result<(), LogMcpError> {
        let sid = session_id.to_string();
        let key_s = key.to_string();
        let value_s = value.to_string();
        let db_path = self.db_path.clone();
        let cfg = self.config.clone();
        self
            .run_with_retry(move || {
                let conn = Connection::open(db_path.as_path()).map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                conn.pragma_update(None, "journal_mode", &"WAL").map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                let exists: Option<String> = conn
                    .query_row(
                        "SELECT id FROM sessions WHERE id = ?1",
                        params![sid.as_str()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                if exists.is_none() {
                    return Err(LogMcpError::SessionNotFound(sid.clone()));
                }

                let files_bytes: i64 = conn
                    .query_row(
                        "SELECT COALESCE(SUM(size_bytes),0) FROM session_files WHERE session_id = ?1",
                        params![sid.as_str()],
                        |row| row.get(0),
                    )
                    .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                let mem_bytes: i64 = conn
                    .query_row(
                        "SELECT COALESCE(SUM(LENGTH(value)),0) FROM memories WHERE session_id = ?1",
                        params![sid.as_str()],
                        |row| row.get(0),
                    )
                    .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                let fact_bytes: i64 = conn
                    .query_row(
                        "SELECT COALESCE(SUM(LENGTH(fact_json)),0) FROM facts WHERE session_id = ?1",
                        params![sid.as_str()],
                        |row| row.get(0),
                    )
                    .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                let projected_total = files_bytes.max(0) as u64 + mem_bytes.max(0) as u64 + fact_bytes.max(0) as u64 + value_s.len() as u64;
                if projected_total > cfg.max_session_bytes {
                    warn!(session_id = %sid, total_bytes = projected_total, "quota exceeded on set_memory");
                    return Err(LogMcpError::QuotaExceeded(sid.clone()));
                }

                let now = Utc::now().timestamp();
                conn.execute(
                    "INSERT INTO memories (session_id, key, value, updated_at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(session_id, key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
                    params![sid.as_str(), key_s.as_str(), value_s.as_str(), now],
                )
                .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                Ok(())
            })
            .await
    }

    pub async fn remove_memory(&self, session_id: &str, key: &str) -> Result<(), LogMcpError> {
        let sid = session_id.to_string();
        let key_s = key.to_string();
        let db_path = self.db_path.clone();
        self
            .run_with_retry(move || {
                let conn = Connection::open(db_path.as_path()).map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                conn.pragma_update(None, "journal_mode", &"WAL").map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                let exists: Option<String> = conn
                    .query_row(
                        "SELECT id FROM sessions WHERE id = ?1",
                        params![sid.as_str()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                if exists.is_none() {
                    return Err(LogMcpError::SessionNotFound(sid.clone()));
                }
                conn.execute(
                    "DELETE FROM memories WHERE session_id = ?1 AND key = ?2",
                    params![sid.as_str(), key_s.as_str()],
                )
                .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                Ok(())
            })
            .await
    }

    pub async fn cleanup_expired(&self) -> Result<usize, LogMcpError> {
        let db_path = self.db_path.clone();
        let ttl = self.config.session_ttl_secs as i64;
        self
            .run_with_retry(move || {
                let conn = Connection::open(db_path.as_path()).map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                conn.pragma_update(None, "journal_mode", &"WAL").map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                let cutoff = Utc::now().timestamp() - ttl;
                let mut stmt = conn
                    .prepare("DELETE FROM sessions WHERE last_access_ts < ?1")
                    .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                let affected = stmt
                    .execute(params![cutoff])
                    .map_err(|e| LogMcpError::DatabaseError(e.to_string()))?;
                Ok(affected as usize)
            })
            .await
    }
}

trait OptionalRowExt<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalRowExt<T> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
