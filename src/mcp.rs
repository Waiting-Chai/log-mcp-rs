use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::error::{LogSearchError, Result};
use crate::model::{FileScanConfig, SearchRequest};
use crate::search::SearchEngine;

#[derive(Debug, Deserialize)]
struct RpcRequest {
    #[serde(default)]
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Serialize)]
struct RpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

pub async fn run_stdio(engine: Arc<SearchEngine>) -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = reader.next_line().await? {
        let req: RpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                write_response(
                    &mut stdout,
                    RpcResponse {
                        jsonrpc: "2.0",
                        id: Value::Null,
                        result: None,
                        error: Some(RpcError {
                            code: -32700,
                            message: format!("parse error: {e}"),
                        }),
                    },
                )
                .await?;
                continue;
            }
        };

        let resp = match req.method.as_str() {
            "initialize" => handle_initialize(&req),
            "notifications/initialized" => {
                // Client confirmed initialization; no response needed for notification,
                // but we should probably just continue reading loop or log it.
                // However, run_stdio expects to write a response for every request logic structure here,
                // but 'notifications' usually don't have IDs.
                // If req.id is null, it's a notification.
                if req.id.is_null() {
                    continue;
                }
                // If it has an ID, we acknowledge it (though standard MCP says initialized is a notification)
                RpcResponse {
                    jsonrpc: "2.0",
                    id: req.id,
                    result: Some(Value::Bool(true)),
                    error: None,
                }
            }
            "list_log_files" => handle_list_files(&engine, &req).await,
            "search_logs" => handle_search(&engine, &req).await,
            "tools/list" | "list_tools" => handle_list_tools(&req), // Support both standard and custom method name if needed
            _ => RpcResponse {
                jsonrpc: "2.0",
                id: req.id,
                result: None,
                error: Some(RpcError {
                    code: -32601,
                    message: format!("method not found: {}", req.method),
                }),
            },
        };

        write_response(&mut stdout, resp).await?;
    }

    Ok(())
}

fn handle_initialize(req: &RpcRequest) -> RpcResponse {
    RpcResponse {
        jsonrpc: "2.0",
        id: req.id.clone(),
        result: Some(serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": "log-search-mcp",
                "version": "0.1.0"
            }
        })),
        error: None,
    }
}

async fn handle_list_files(engine: &SearchEngine, req: &RpcRequest) -> RpcResponse {
    let params: Result<ListFilesParams> = serde_json::from_value(req.params.clone())
        .map_err(|e| LogSearchError::InvalidRequest(format!("invalid params: {e}")))
        .map_err(Into::into);

    match params {
        Ok(p) => {
            let cfg = FileScanConfig {
                root_path: p.root_path.into(),
                include_globs: p.include_globs.unwrap_or_default(),
                exclude_globs: p.exclude_globs.unwrap_or_default(),
            };
            match engine.list_files(&cfg) {
                Ok(files) => {
                    let list: Vec<String> = files
                        .into_iter()
                        .map(|p| p.to_string_lossy().to_string())
                        .collect();
                    RpcResponse {
                        jsonrpc: "2.0",
                        id: req.id.clone(),
                        result: Some(serde_json::json!({ "files": list })),
                        error: None,
                    }
                }
                Err(e) => rpc_error(req, -32001, e.to_string()),
            }
        }
        Err(e) => rpc_error(req, -32602, e.to_string()),
    }
}

async fn handle_search(engine: &SearchEngine, req: &RpcRequest) -> RpcResponse {
    let params: Result<SearchRequest> = serde_json::from_value(req.params.clone())
        .map_err(|e| LogSearchError::InvalidRequest(format!("invalid params: {e}")))
        .map_err(Into::into);

    match params {
        Ok(p) => match engine.search(p).await {
            Ok(res) => RpcResponse {
                jsonrpc: "2.0",
                id: req.id.clone(),
                result: Some(serde_json::to_value(res).unwrap_or(Value::Null)),
                error: None,
            },
            Err(e) => rpc_error(req, -32002, e.to_string()),
        },
        Err(e) => rpc_error(req, -32602, e.to_string()),
    }
}

async fn write_response(stdout: &mut tokio::io::Stdout, resp: RpcResponse) -> Result<()> {
    let line = serde_json::to_string(&resp).unwrap_or_else(|_| "{}".to_string());
    stdout.write_all(line.as_bytes()).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await?;
    Ok(())
}

fn rpc_error(req: &RpcRequest, code: i32, message: String) -> RpcResponse {
    RpcResponse {
        jsonrpc: "2.0",
        id: req.id.clone(),
        result: None,
        error: Some(RpcError { code, message }),
    }
}

#[derive(Debug, Deserialize)]
struct ListFilesParams {
    #[serde(default)]
    pub root_path: String,
    pub include_globs: Option<Vec<String>>,
    pub exclude_globs: Option<Vec<String>>,
}

fn handle_list_tools(req: &RpcRequest) -> RpcResponse {
    let tools = vec![
        serde_json::json!({
            "name": "list_log_files",
            "description": "List log files under a root path with optional include/exclude globs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "root_path": { "type": "string" },
                    "include_globs": { "type": "array", "items": { "type": "string" } },
                    "exclude_globs": { "type": "array", "items": { "type": "string" } }
                }
            }
        }),
        serde_json::json!({
            "name": "search_logs",
            "description": "Search log files with logical queries, optional time filter and multiline pattern.",
            "inputSchema": {
                "type": "object",
                "required": ["scan_config", "logical_query"],
                "properties": {
                    "scan_config": {
                        "type": "object",
                        "properties": {
                            "root_path": { "type": "string" },
                            "include_globs": { "type": "array", "items": { "type": "string" } },
                            "exclude_globs": { "type": "array", "items": { "type": "string" } }
                        }
                    },
                    "logical_query": {
                        "type": "object",
                        "properties": {
                            "must": { "type": "array" },
                            "any": { "type": "array" },
                            "none": { "type": "array" }
                        }
                    },
                    "time_filter": { "type": ["object", "null"] },
                    "log_start_pattern": { "type": ["string", "null"] },
                    "page_size": { "type": "integer" },
                    "page": { "type": "integer" },
                    "max_hits": { "type": ["integer", "null"] },
                    "hard_timeout_ms": { "type": ["integer", "null"] },
                    "include_content": { "type": "boolean" }
                }
            }
        })
    ];

    RpcResponse {
        jsonrpc: "2.0",
        id: req.id.clone(),
        result: Some(serde_json::json!({ "tools": tools })),
        error: None,
    }
}
