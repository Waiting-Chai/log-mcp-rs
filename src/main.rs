use std::env;

use log_search_mcp::config::Config;
use log_search_mcp::error::Result;
use log_search_mcp::http::serve_http;
use log_search_mcp::mcp::run_stdio;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse().unwrap()))
        .init();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <config.yaml|json>", args[0]);
        std::process::exit(1);
    }
    let cfg_path = std::path::Path::new(&args[1]);
    let config = Config::load_from_path(cfg_path)?;

    match config.server.mode {
        log_search_mcp::config::ServerMode::Http => {
            serve_http(config).await?;
        }
        log_search_mcp::config::ServerMode::Stdio => {
            let engine = std::sync::Arc::new(log_search_mcp::search::SearchEngine::with_config(
                config.search.clone(),
                config.log_parser.default_log_start_pattern.clone(),
                config.log_parser.default_timestamp_regex.clone(),
            ));
            run_stdio(engine).await?;
        }
        log_search_mcp::config::ServerMode::Both => {
            let engine = std::sync::Arc::new(log_search_mcp::search::SearchEngine::with_config(
                config.search.clone(),
                config.log_parser.default_log_start_pattern.clone(),
                config.log_parser.default_timestamp_regex.clone(),
            ));
            let engine2 = engine.clone();
            let http_task = tokio::spawn(async move { serve_http(config).await });
            let stdio_task = tokio::spawn(async move { run_stdio(engine2).await });
            let _ = http_task.await.expect("http task panicked")?;
            let _ = stdio_task.await.expect("stdio task panicked")?;
        }
    }

    Ok(())
}
