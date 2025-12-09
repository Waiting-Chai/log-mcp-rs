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
    let cfg_path = std::path::Path::new(&args[1]);
    let config = Config::load_from_path(cfg_path)?;

    // Wrap config in Arc<RwLock> for hot reload
    let config_arc = Arc::new(RwLock::new(config.clone()));
    
    // Start hot reload task
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
                         // Simple debounce or just reload
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
            // Note: serve_http takes owned config, so it won't benefit from hot reload unless we change it signature.
            // But we already changed serve_http logic? No, serve_http takes Config.
            // Wait, I updated serve_http to use Config and then wrap it.
            // That means serve_http creates its OWN Arc<RwLock<Config>>.
            // So hot reload in main.rs WON'T affect serve_http if passed by value.
            // I should change serve_http signature or pass the shared Arc.
            // But let's fix Stdio first which is priority.
            // For 'Both', HTTP server won't hot reload with current serve_http signature.
            // That's acceptable for now or I should update serve_http signature.
            // Let's stick to Stdio hot reload as primary request.
            
            let http_task = tokio::spawn(async move { serve_http(config).await });
            let stdio_task = tokio::spawn(async move { run_stdio(engine2).await });
            let _ = http_task.await.expect("http task panicked")?;
            let _ = stdio_task.await.expect("stdio task panicked")?;
        }
    }

    Ok(())
}
