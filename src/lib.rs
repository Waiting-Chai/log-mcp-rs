//! 日志搜索 MCP 工具核心库
//! 模块划分清晰，便于后续扩展与解耦。

pub mod config;
pub mod error;
pub mod model;
pub mod scanner;
pub mod reader;
pub mod parser;
pub mod query;
pub mod search;
pub mod http;
pub mod mcp;
