use std::{collections::HashMap, path::PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub rpc: RpcConfig,
    pub scheduler: SchedulerConfig,
    pub broadcaster: BroadcasterConfig,
    pub watcher: WatcherConfig,
    pub api: ApiConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ServerConfig {
    pub bind: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RedisConfig {
    pub url: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RpcConfig {
    pub chains: HashMap<u64, Vec<String>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SchedulerConfig {
    pub poll_interval_ms: u64,
    pub lease_ttl_seconds: i64,
    pub max_concurrency: usize,
    pub retry_min_ms: u64,
    pub retry_max_ms: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BroadcasterConfig {
    pub fanout: usize,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WatcherConfig {
    pub poll_interval_ms: u64,
    pub use_websocket: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ApiConfig {
    pub max_body_bytes: usize,
}

#[derive(Debug, Deserialize)]
struct ConfigRaw {
    server: ServerConfig,
    database: DatabaseConfig,
    redis: RedisConfig,
    rpc: RpcConfigRaw,
    scheduler: SchedulerConfig,
    broadcaster: BroadcasterConfig,
    watcher: WatcherConfig,
    api: ApiConfig,
}

#[derive(Debug, Deserialize)]
struct RpcConfigRaw {
    chains: HashMap<String, Vec<String>>,
}

impl Config {
    pub fn load() -> Result<Self> {
        dotenvy::dotenv().ok();
        let path = std::env::var("CONFIG_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("config.toml"));

        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("read config file {}", path.display()))?;
        let expanded = shellexpand::env(&raw)
            .with_context(|| format!("expand env vars in {}", path.display()))?;
        let parsed: ConfigRaw = toml::from_str(expanded.as_ref())
            .with_context(|| format!("parse config file {}", path.display()))?;

        let mut chains = HashMap::new();
        for (key, urls) in parsed.rpc.chains {
            let chain_id: u64 = key
                .parse()
                .with_context(|| format!("rpc.chains key '{key}' must be a numeric chain id"))?;
            chains.insert(chain_id, urls);
        }

        Ok(Self {
            server: parsed.server,
            database: parsed.database,
            redis: parsed.redis,
            rpc: RpcConfig { chains },
            scheduler: parsed.scheduler,
            broadcaster: parsed.broadcaster,
            watcher: parsed.watcher,
            api: parsed.api,
        })
    }
}
