use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::{Address, Bytes, TxKind, U256, keccak256};
use alloy::signers::SignerSync;
use alloy::signers::local::PrivateKeySigner;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::Value;
use tempo_alloy::primitives::transaction::{Call, PrimitiveSignature};
use tempo_alloy::primitives::{AASigned, TempoSignature, TempoTransaction};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::time::timeout;

use tempo_watchtower::api;
use tempo_watchtower::config::{
    ApiConfig, BroadcasterConfig, Config, DatabaseConfig, RedisConfig, RpcConfig, SchedulerConfig,
    ServerConfig, WatcherConfig,
};
use tempo_watchtower::db;
use tempo_watchtower::rpc::RpcManager;
use tempo_watchtower::scheduler;
use tempo_watchtower::state::AppState;

#[derive(Clone, Default)]
struct RpcState {
    seen_raw: Arc<Mutex<Vec<String>>>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_signed_tx_is_broadcast() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let db_url = env_var("DB_USER")
        .and_then(|user| {
            let host = env_var("DB_HOST")?;
            let port = env_var("DB_PORT")?;
            let name = env_var("DB_NAME")?;
            Some(format!("postgres://{user}@{host}:{port}/{name}"))
        })
        .ok_or_else(|| anyhow::anyhow!("missing DB env vars"))?;

    let redis_url = env_var("REDIS_HOST")
        .and_then(|host| {
            let port = env_var("REDIS_PORT")?;
            let db = env_var("REDIS_DB")?;
            Some(format!("redis://{host}:{port}/{db}"))
        })
        .ok_or_else(|| anyhow::anyhow!("missing REDIS env vars"))?;

    let (rpc_addr, rpc_state) = start_fake_rpc().await?;
    let rpc_url = format!("http://{rpc_addr}");

    let config = Config {
        server: ServerConfig {
            bind: "127.0.0.1:0".to_string(),
        },
        database: DatabaseConfig { url: db_url },
        redis: RedisConfig { url: redis_url },
        rpc: RpcConfig {
            chains: vec![(42431u64, vec![rpc_url])].into_iter().collect(),
        },
        scheduler: SchedulerConfig {
            poll_interval_ms: 100,
            lease_ttl_seconds: 10,
            max_concurrency: 10,
            retry_min_ms: 100,
            retry_max_ms: 500,
        },
        broadcaster: BroadcasterConfig {
            fanout: 1,
            timeout_ms: 500,
        },
        watcher: WatcherConfig {
            poll_interval_ms: 1000,
            use_websocket: false,
        },
        api: ApiConfig {
            max_body_bytes: 1024 * 1024,
        },
    };

    let db_pool = db::connect(&config.database.url).await?;
    db::migrate(&db_pool).await?;
    sqlx::query("TRUNCATE txs").execute(&db_pool).await?;

    let redis_client = redis::Client::open(config.redis.url.as_str())?;
    let redis_conn = redis::aio::ConnectionManager::new(redis_client).await?;
    let mut redis_flush = redis_conn.clone();
    redis::cmd("FLUSHDB")
        .query_async::<()>(&mut redis_flush)
        .await?;

    let rpcs = Arc::new(RpcManager::new(&config).await?);
    let state = AppState {
        config: Arc::new(config),
        db: db_pool,
        redis: redis_conn,
        rpcs,
    };

    scheduler::start(state.clone());

    let app = api::router(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let api_addr = listener.local_addr()?;
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("api server failed");
    });

    let raw_tx = build_signed_tx()?;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{api_addr}/v1/transactions"))
        .json(&serde_json::json!({
            "chainId": 42431,
            "transactions": [raw_tx.clone()]
        }))
        .send()
        .await?;

    assert!(resp.status().is_success());

    wait_for_raw(&rpc_state, &raw_tx).await?;

    Ok(())
}

async fn start_fake_rpc() -> anyhow::Result<(SocketAddr, RpcState)> {
    let state = RpcState::default();
    let app = Router::new()
        .route("/", post(rpc_handler))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("rpc server failed");
    });

    Ok((addr, state))
}

async fn rpc_handler(
    axum::extract::State(state): axum::extract::State<RpcState>,
    Json(payload): Json<Value>,
) -> Json<Value> {
    let id = payload.get("id").cloned().unwrap_or(Value::from(1));
    let method = payload
        .get("method")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let params = payload
        .get("params")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    let result = match method {
        "eth_sendRawTransaction" => {
            let raw = params
                .first()
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            if !raw.is_empty() {
                state.seen_raw.lock().await.push(raw.clone());
            }
            json_hex_hash(&raw)
        }
        "eth_chainId" => Value::from("0xa5bf"),
        "eth_getTransactionCount" => Value::from("0x0"),
        "eth_getTransactionReceipt" => Value::Null,
        "web3_clientVersion" => Value::from("tempo-watchtower-test"),
        _ => Value::Null,
    };

    Json(serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    }))
}

fn json_hex_hash(raw: &str) -> Value {
    let raw = raw.strip_prefix("0x").unwrap_or(raw);
    let bytes = hex::decode(raw).unwrap_or_default();
    let hash = keccak256(&bytes);
    Value::from(format!("0x{}", hex::encode(hash)))
}

async fn wait_for_raw(state: &RpcState, expected: &str) -> anyhow::Result<()> {
    let expected = expected.to_string();
    let deadline = Duration::from_secs(5);

    timeout(deadline, async {
        loop {
            let seen = state.seen_raw.lock().await;
            if seen.iter().any(|raw| raw == &expected) {
                return;
            }
            drop(seen);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for broadcast"))?;

    Ok(())
}

fn build_signed_tx() -> anyhow::Result<String> {
    let signer = PrivateKeySigner::random();
    let call = Call {
        to: TxKind::Call(Address::ZERO),
        value: U256::ZERO,
        input: Bytes::default(),
    };

    let tx = TempoTransaction {
        chain_id: 42431,
        fee_token: None,
        max_priority_fee_per_gas: 1,
        max_fee_per_gas: 1,
        gas_limit: 21000,
        calls: vec![call],
        access_list: alloy::rpc::types::AccessList::default(),
        nonce_key: U256::ZERO,
        nonce: 0,
        fee_payer_signature: None,
        valid_before: None,
        valid_after: None,
        key_authorization: None,
        tempo_authorization_list: Vec::new(),
    };

    let signature = signer.sign_hash_sync(&tx.signature_hash())?;
    let tempo_sig = TempoSignature::Primitive(PrimitiveSignature::Secp256k1(signature));
    let signed: AASigned = tx.into_signed(tempo_sig);

    let mut buf = Vec::new();
    signed.eip2718_encode(&mut buf);

    Ok(format!("0x{}", hex::encode(buf)))
}

fn env_var(key: &str) -> Option<String> {
    std::env::var(key).ok()
}
