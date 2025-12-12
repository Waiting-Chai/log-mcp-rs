use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use axum::{
    extract::{
        rejection::{JsonRejection, QueryRejection},
        Query, State,
    },
    http::StatusCode,
    response::{sse::{Event, Sse, KeepAlive}, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use futures::stream::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::model::{FileScanConfig, SearchRequest};
use crate::search::SearchEngine;
use crate::{config::Config, error::Result};

#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<SearchEngine>,
    pub sessions: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<Event>>>>,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

impl IntoResponse for ErrorResponse {
    fn into_response(self) -> axum::response::Response {
        (StatusCode::BAD_REQUEST, Json(self)).into_response()
    }
}

#[derive(Debug, Deserialize)]
pub struct ListFilesQuery {
    pub root_path: String,
    #[serde(default)]
    pub include_globs: Vec<String>,
    #[serde(default)]
    pub exclude_globs: Vec<String>,
}

async fn search_handler(
    State(state): State<AppState>,
    payload: std::result::Result<Json<SearchRequest>, JsonRejection>,
) -> impl IntoResponse {
    let req = match payload {
        Ok(Json(req)) => req,
        Err(e) => {
            return ErrorResponse {
                error: format!("invalid request body: {e}"),
            }
            .into_response()
        }
    };

    match state.engine.search(req).await {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => ErrorResponse {
            error: e.to_string(),
        }
        .into_response(),
    }
}

async fn list_files_handler(
    State(state): State<AppState>,
    q: std::result::Result<Query<ListFilesQuery>, QueryRejection>,
) -> impl IntoResponse {
    let q = match q {
        Ok(Query(q)) => q,
        Err(e) => {
            return ErrorResponse {
                error: format!("invalid query: {e}"),
            }
            .into_response()
        }
    };
    let config = FileScanConfig {
        root_path: q.root_path.into(),
        include_globs: q.include_globs,
        exclude_globs: q.exclude_globs,
    };
    match state.engine.list_files(&config) {
        Ok(files) => {
            let as_str: Vec<String> = files
                .into_iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            (StatusCode::OK, Json(as_str)).into_response()
        }
        Err(e) => ErrorResponse {
            error: e.to_string(),
        }
        .into_response(),
    }
}

async fn sse_handler(State(state): State<AppState>) -> Sse<impl Stream<Item = std::result::Result<Event, axum::Error>>> {
    let (tx, rx) = mpsc::unbounded_channel();
    let session_id = format!("{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
    
    // Send endpoint event
    let endpoint_url = format!("/message?session_id={}", session_id);
    // Note: In a real deployment behind a proxy, this URL might need to be absolute or relative correctly.
    // For now, we assume relative path works if client handles it, or client is local.
    // MCP spec usually expects relative URI reference.
    let _ = tx.send(Event::default().event("endpoint").data(endpoint_url));
    
    state.sessions.write().unwrap().insert(session_id.clone(), tx);
    
    let stream = UnboundedReceiverStream::new(rx).map(Ok::<_, axum::Error>);
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[derive(Deserialize)]
struct MessageQuery {
    session_id: String,
}

async fn message_handler(
    State(state): State<AppState>,
    Query(q): Query<MessageQuery>,
    Json(req): Json<crate::mcp::RpcRequest>,
) -> impl IntoResponse {
    let sender = {
        let sessions = state.sessions.read().unwrap();
        sessions.get(&q.session_id).cloned()
    };

    if let Some(sender) = sender {
        let engine = state.engine.clone();
        tokio::spawn(async move {
            let resp = crate::mcp::process_request(engine, req).await;
            if let Ok(json_str) = serde_json::to_string(&resp) {
                let _ = sender.send(Event::default().event("message").data(json_str));
            }
        });
        StatusCode::ACCEPTED
    } else {
        StatusCode::NOT_FOUND
    }
}

pub fn build_router(engine: Arc<SearchEngine>) -> Router {
    let state = AppState { 
        engine,
        sessions: Arc::new(RwLock::new(HashMap::new())),
    };
    Router::new()
        .route("/search", post(search_handler))
        .route("/files", get(list_files_handler))
        .route("/sse", get(sse_handler))
        .route("/message", post(message_handler))
        .with_state(state)
}

pub async fn serve_http(config: Config) -> Result<()> {
    let config_arc = Arc::new(RwLock::new(config.clone()));
    let engine = Arc::new(SearchEngine::new(config_arc));
    let router = build_router(engine);

    let addr = format!(
        "{}:{}",
        config.server.http_addr.unwrap_or_else(|| "0.0.0.0".to_string()),
        config.server.http_port.unwrap_or(3000)
    );
    let listener = TcpListener::bind(&addr)
        .await
        .map_err(|e| crate::error::LogSearchError::ConfigError(format!("bind {addr} failed: {e}")))?;
    println!("HTTP server listening on http://{}", addr);
    axum::serve(listener, router).await.map_err(|e| e.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use serde_json::json;
    use tempfile::tempdir;
    use tower::util::ServiceExt;

    use crate::config::{Config, LogParserConfig, LogSourceConfig, SearchConfig, ServerConfig, ServerMode};
    use crate::model::{SearchQuery, SearchResponse};

    fn create_test_engine(buffer_size: usize) -> Arc<SearchEngine> {
        let mut cfg = Config {
             server: ServerConfig { mode: ServerMode::Stdio, http_addr: None, http_port: None },
             log_parser: LogParserConfig { default_log_start_pattern: None, default_timestamp_regex: None },
             search: SearchConfig::default(),
             log_sources: LogSourceConfig::default(),
        };
        cfg.search.buffer_size = buffer_size;
        Arc::new(SearchEngine::new(Arc::new(RwLock::new(cfg))))
    }

    fn sq(text: &str) -> SearchQuery {
        SearchQuery {
            query: Some(text.to_string()),
            regex: false,
            case_sensitive: false,
            whole_word: false,
        }
    }

    #[tokio::test]
    async fn list_files_endpoint_returns_logs() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let log_path = root.join("a.log");
        std::fs::write(&log_path, "hello").unwrap();

        let engine = create_test_engine(16 * 1024);
        let direct = FileScanConfig {
            root_path: root.to_path_buf(),
            include_globs: vec!["**/*.log".to_string()],
            exclude_globs: Vec::new(),
        };
        let direct_files = engine.list_files(&direct).unwrap();
        assert!(direct_files.contains(&log_path));
        let app = build_router(engine);

        let normalized = root.to_string_lossy().replace('\\', "/");
        let uri = format!("/files?root_path={}", normalized);
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let status = resp.status();
        let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        if status != StatusCode::OK {
            panic!("status {:?}, body {:?}", status, String::from_utf8_lossy(&body));
        }
        let list: Vec<String> = serde_json::from_slice(&body).unwrap();
        assert!(list.iter().any(|p| p.ends_with("a.log")));
    }

    #[tokio::test]
    async fn search_endpoint_returns_hits() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let log_path = root.join("demo.log");
        std::fs::write(&log_path, "traffic error\nok\n").unwrap();

        let engine = create_test_engine(16 * 1024);
        let app = build_router(engine);

        let request_body = json!({
            "scan_config": {
                "root_path": root.to_string_lossy().replace('\\', "/"),
                "include_globs": ["**/*.log"],
                "exclude_globs": []
            },
            "logical_query": {
                "must": [sq("error")],
                "any": [],
            "none": []
        },
        "time_filter": null,
        "log_start_pattern": null,
        "page_size": 10,
        "page": 1,
        "max_hits": null,
        "hard_timeout_ms": null,
        "include_content": true
        });

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/search")
                    .header("content-type", "application/json")
                    .body(Body::from(request_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        let status = resp.status();
        let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        if status != StatusCode::OK {
            panic!("status {:?}, body {:?}", status, String::from_utf8_lossy(&body));
        }
        let result: SearchResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(result.total_hits, 1);
        assert_eq!(result.hits.len(), 1);
        assert!(result.hits[0].content.contains("traffic error"));
    }

    #[tokio::test]
    async fn search_endpoint_invalid_body_returns_400() {
        let engine = create_test_engine(16 * 1024);
        let app = build_router(engine);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/search")
                    .header("content-type", "application/json")
                    .body(Body::from("not-json"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
