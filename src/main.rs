use std::path::PathBuf;
use std::process;
use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use clap::{CommandFactory, Parser};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::EnvFilter;

use tempo_watchtower::config::Config;
use tempo_watchtower::rpc::RpcManager;
use tempo_watchtower::state::AppState;
use tempo_watchtower::{api, db, scheduler, watcher};

#[derive(Debug, Parser)]
#[command(name = "tempo-watchtower")]
struct Cli {
    #[arg(
        long,
        default_value = "config.toml",
        env = "CONFIG_PATH",
        value_name = "PATH"
    )]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if !cli.config.exists() {
        eprintln!("Config file not found: {}", cli.config.display());
        let mut cmd = Cli::command();
        cmd.print_help()?;
        println!();
        process::exit(1);
    }

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    let config = Arc::new(Config::load_from_path(cli.config)?);
    let db = db::connect(&config.database.url).await?;
    db::migrate(&db).await?;

    let redis = redis::Client::open(config.redis.url.as_str())?;
    let redis = redis::aio::ConnectionManager::new(redis).await?;

    let rpcs = Arc::new(RpcManager::new(&config).await?);

    let state = AppState {
        config: config.clone(),
        db,
        redis,
        rpcs,
    };

    scheduler::recover_after_restart(&state).await?;
    scheduler::start(state.clone());
    watcher::start(state.clone());

    let app = Router::new()
        .merge(api::router(state.clone()))
        .layer(RequestBodyLimitLayer::new(config.api.max_body_bytes))
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(&config.server.bind).await?;
    info!(bind = %config.server.bind, "listening");
    axum::serve(listener, app).await?;

    Ok(())
}
