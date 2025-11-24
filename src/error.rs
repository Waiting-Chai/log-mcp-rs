use std::path::PathBuf;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, LogSearchError>;

#[derive(Debug, Error)]
pub enum LogSearchError {
    #[error("配置错误: {0}")]
    ConfigError(String),

    #[error("文件访问错误: {path} - {reason}")]
    FileAccessError { path: PathBuf, reason: String },

    #[error("正则表达式错误: {pattern} - {reason}")]
    RegexError { pattern: String, reason: String },

    #[error("编码检测失败: {path} - {reason}")]
    EncodingError { path: PathBuf, reason: String },

    #[error("时间解析错误: {input}")]
    TimeParseError { input: String },

    #[error("搜索超时")]
    TimeoutError,

    #[error("无效请求: {0}")]
    InvalidRequest(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}
