use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::error::{LogSearchError, Result};
use crate::model::{FileScanConfig, SearchRequest};
use crate::search::SearchEngine;

fn debug_log(msg: &str) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/log-mcp-debug.log") {
        let _ = writeln!(file, "[MCP] {}", msg);
    }
}

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
                if req.id.is_null() {
                    continue;
                }
                RpcResponse {
                    jsonrpc: "2.0",
                    id: req.id,
                    result: Some(Value::Bool(true)),
                    error: None,
                }
            }

            "tools/call" | "call_tool" => handle_tool_call(&engine, &req).await,
            
            "list_log_files" => handle_list_files(&engine, &req).await,
            "search_logs" => handle_search(&engine, &req).await,
            "tools/list" | "list_tools" => handle_list_tools(&req),
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

async fn handle_tool_call(engine: &SearchEngine, req: &RpcRequest) -> RpcResponse {
    // 解析 tools/call 的参数
    // params 应该包含 name 和 arguments
    #[derive(Deserialize)]
    struct ToolCallParams {
        name: String,
        arguments: serde_json::Value,
    }

    debug_log(&format!("handle_tool_call: params={}", req.params));

    let params: std::result::Result<ToolCallParams, _> = serde_json::from_value(req.params.clone());
    
    match params {
        Ok(mut p) => {
            debug_log(&format!("Tool call name: {}", p.name));
            
            // 处理 arguments 为字符串（双重编码 JSON）的情况
            if let Some(arg_str) = p.arguments.as_str() {
                if let Ok(parsed_args) = serde_json::from_str::<serde_json::Value>(arg_str) {
                    debug_log("Parsed arguments from string");
                    p.arguments = parsed_args;
                }
            }

            // 构造一个新的 RpcRequest，把 arguments 当作 params 传给具体处理函数
            let sub_req = RpcRequest {
                // RpcRequest 结构定义中没有 jsonrpc 字段！
                method: p.name.clone(),
                params: p.arguments,
                id: req.id.clone(),
            };
            
            match p.name.as_str() {
                "list_log_files" => handle_list_files(engine, &sub_req).await,
                "search_logs" => handle_search(engine, &sub_req).await,
                _ => rpc_error(req, -32601, format!("tool not found: {}", p.name)),
            }
        }
        Err(e) => {
            debug_log(&format!("Tool call parse error: {}", e));
            rpc_error(req, -32700, format!("invalid tool call params: {}", e))
        },
    }
}

async fn handle_list_files(engine: &SearchEngine, req: &RpcRequest) -> RpcResponse {
    debug_log("handle_list_files called");
    let params: Result<ListFilesParams> = serde_json::from_value(req.params.clone())
        .map_err(|e| LogSearchError::InvalidRequest(format!("invalid params: {e}")))
        .map_err(Into::into);

    match params {
        Ok(p) => {
            // 检查 root_path 是否为空。如果是，则依赖全局 log_file_paths。
            // 但必须正确构造 FileScanConfig。
            // 如果 p.root_path 为空字符串（由于默认值），则传递空的 PathBuf。
            
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
                    
                    // MCP 要求结果包装在 content 数组中
                    let content_text = serde_json::to_string_pretty(&serde_json::json!({ "files": list })).unwrap_or_default();
                    
                    RpcResponse {
                        jsonrpc: "2.0",
                        id: req.id.clone(),
                        result: Some(serde_json::json!({
                            "content": [{
                                "type": "text",
                                "text": content_text
                            }],
                            "isError": false
                        })),
                        error: None,
                    }
                }
                Err(e) => {
                    // 如果可能，应用错误也应作为工具结果返回，
                    // 但此处暂时保持使用 rpc_error 或切换到 isError=true
                    rpc_error(req, -32001, e.to_string())
                },
            }
        }
        Err(e) => rpc_error(req, -32602, e.to_string()),
    }
}

async fn handle_search(engine: &SearchEngine, req: &RpcRequest) -> RpcResponse {
    debug_log(&format!("handle_search: params={}", req.params));
    let params: Result<SearchRequest> = serde_json::from_value(req.params.clone())
        .map_err(|e| LogSearchError::InvalidRequest(format!("invalid params: {e}")))
        .map_err(Into::into);

    match params {
        Ok(p) => {
            debug_log(&format!("Search request parsed: {:?}", p));
            match engine.search(p).await {
                Ok(res) => {
                    debug_log(&format!("Search success. Hits: {}", res.hits.len()));
                    
                    // 将结果序列化为格式化的 JSON 字符串
                    let content_text = serde_json::to_string_pretty(&res).unwrap_or_else(|_| "{}".to_string());
                    
                    RpcResponse {
                        jsonrpc: "2.0",
                        id: req.id.clone(),
                        result: Some(serde_json::json!({
                            "content": [{
                                "type": "text",
                                "text": content_text
                            }],
                            "isError": false
                        })),
                        error: None,
                    }
                },
                Err(e) => {
                    debug_log(&format!("Search failed: {}", e));
                    // 将应用错误作为工具结果返回，以便模型可以看到它
                    let error_text = format!("Search failed: {}", e);
                    RpcResponse {
                        jsonrpc: "2.0",
                        id: req.id.clone(),
                        result: Some(serde_json::json!({
                            "content": [{
                                "type": "text",
                                "text": error_text
                            }],
                            "isError": true
                        })),
                        error: None,
                    }
                },
            }
        },
        Err(e) => {
            debug_log(&format!("Search params parse error: {}", e));
            // 参数解析错误仍然是协议/请求错误，但我们也可以将其作为文本返回
            rpc_error(req, -32602, e.to_string())
        },
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
                    "root_path": { "type": "string", "description": "Optional root path. If omitted, uses globally configured log files." },
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
                            "root_path": { "type": "string", "description": "Root directory to scan. Optional if system logs are configured." },
                            "include_globs": { "type": "array", "items": { "type": "string" } },
                            "exclude_globs": { "type": "array", "items": { "type": "string" } }
                        }
                    },
                    "logical_query": {
                        "type": "object",
                        "properties": {
                            "must": { 
                                "type": "array",
                                "items": {
                                    "anyOf": [
                                        { "type": "string" },
                                        {
                                            "type": "object",
                                            "properties": {
                                                "query": { "type": "string" },
                                                "regex": { "type": "boolean" },
                                                "case_sensitive": { "type": "boolean" },
                                                "whole_word": { "type": "boolean" }
                                            }
                                        }
                                    ]
                                }
                            },
                            "any": { 
                                "type": "array",
                                "items": {
                                    "anyOf": [
                                        { "type": "string" },
                                        {
                                            "type": "object",
                                            "properties": {
                                                "query": { "type": "string" },
                                                "regex": { "type": "boolean" },
                                                "case_sensitive": { "type": "boolean" },
                                                "whole_word": { "type": "boolean" }
                                            }
                                        }
                                    ]
                                }
                            },
                            "none": { 
                                "type": "array",
                                "items": {
                                    "anyOf": [
                                        { "type": "string" },
                                        {
                                            "type": "object",
                                            "properties": {
                                                "query": { "type": "string" },
                                                "regex": { "type": "boolean" },
                                                "case_sensitive": { "type": "boolean" },
                                                "whole_word": { "type": "boolean" }
                                            }
                                        }
                                    ]
                                }
                            }
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
