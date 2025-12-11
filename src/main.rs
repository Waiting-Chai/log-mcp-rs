use std::env;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::time::sleep;

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
    
    // 调试日志输出到文件
    use std::io::Write;
    let log_file_path = "/tmp/log-mcp-debug.log";
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(log_file_path) {
        let _ = writeln!(file, "\n--- MCP Server Starting at {:?} ---", std::time::SystemTime::now());
        let _ = writeln!(file, "CWD: {:?}", env::current_dir());
        let _ = writeln!(file, "Args: {:?}", args);
        let _ = writeln!(file, "Config Path: {:?}", args[1]);
    }

    // 调试信息输出到 stderr
    eprintln!("MCP Server Starting...");
    eprintln!("CWD: {:?}", env::current_dir());
    eprintln!("Args: {:?}", args);
    
    let cfg_path = std::path::Path::new(&args[1]);
    
    // 尝试解析绝对路径以提高清晰度
    if let Ok(abs_path) = std::fs::canonicalize(cfg_path) {
        eprintln!("Resolved config path: {:?}", abs_path);
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(log_file_path) {
             let _ = writeln!(file, "Resolved config path: {:?}", abs_path);
        }
    } else {
        eprintln!("Could not resolve config path: {:?}", cfg_path);
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(log_file_path) {
             let _ = writeln!(file, "Could not resolve config path: {:?}", cfg_path);
        }
    }

    let config = Config::load_from_path(cfg_path).map_err(|e| {
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(log_file_path) {
             let _ = writeln!(file, "Config load error: {:?}", e);
        }
        e
    })?;
    
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(log_file_path) {
         let _ = writeln!(file, "Config loaded successfully.");
         let _ = writeln!(file, "Log files: {:?}", config.log_sources.log_file_paths);
    }
    
    eprintln!("Config loaded successfully.");
    if let Some(paths) = &config.log_sources.log_file_paths {
        eprintln!("Global log files configured: {:?}", paths);
    } else {
        eprintln!("No global log files configured!");
    }

    // 将配置包装在 Arc<RwLock> 中以支持热重载
    let config_arc = Arc::new(RwLock::new(config.clone()));
    
    // 启动热重载任务
    let config_path_owned = cfg_path.to_path_buf();
    let config_for_update = config_arc.clone();
    
    tokio::spawn(async move {
        let mut last_mtime = match std::fs::metadata(&config_path_owned) {
            Ok(m) => m.modified().ok(),
            Err(_) => None,
        };
        
        loop {
            sleep(Duration::from_secs(5)).await;
            
            match std::fs::metadata(&config_path_owned) {
                Ok(m) => {
                     let mtime = m.modified().ok();
                     if mtime != last_mtime {
                         // 简单的去抖动或直接重载
                         eprintln!("Config changed, reloading...");
                         match Config::load_from_path(&config_path_owned) {
                             Ok(new_cfg) => {
                                 let mut w = config_for_update.write().unwrap();
                                 *w = new_cfg;
                                 last_mtime = mtime;
                                 eprintln!("Config reloaded successfully.");
                             },
                             Err(e) => {
                                 eprintln!("Failed to reload config: {}", e);
                             }
                         }
                     }
                },
                Err(_) => {} 
            }
        }
    });

    match config.server.mode {
        log_search_mcp::config::ServerMode::Http => {
            serve_http(config).await?;
        }
        log_search_mcp::config::ServerMode::Stdio => {
            let engine = std::sync::Arc::new(log_search_mcp::search::SearchEngine::new(config_arc));
            run_stdio(engine).await?;
        }
        log_search_mcp::config::ServerMode::Both => {
            let engine = std::sync::Arc::new(log_search_mcp::search::SearchEngine::new(config_arc));
            let engine2 = engine.clone();
            // 注意：serve_http 接收 Config 所有权，因此 HTTP 服务目前不支持热重载配置。
            
            let http_task = tokio::spawn(async move { serve_http(config).await });
            let stdio_task = tokio::spawn(async move { run_stdio(engine2).await });
            let _ = http_task.await.expect("http task panicked")?;
            let _ = stdio_task.await.expect("stdio task panicked")?;
        }
    }

    Ok(())
}
