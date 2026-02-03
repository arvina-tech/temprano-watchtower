use std::sync::Arc;

use chrono::{DateTime, Utc};
use redis::aio::ConnectionManager;
use sqlx::PgPool;

use crate::{config::Config, rpc::RpcManager};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: PgPool,
    pub redis: ConnectionManager,
    pub rpcs: Arc<RpcManager>,
    pub started_at: DateTime<Utc>,
}
