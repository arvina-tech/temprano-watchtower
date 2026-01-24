use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

static E2E_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

const CHAIN_ID: u64 = 42431;

#[derive(Clone, Default)]
struct RpcState {
    seen_raw: Arc<Mutex<Vec<String>>>,
    current_nonce: Arc<AtomicU64>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_signed_tx_is_broadcast() -> anyhow::Result<()> {
    let _guard = acquire_e2e_lock().await;
    let (api_addr, rpc_state) = setup_e2e().await?;
    let raw_tx = build_signed_tx()?;

    send_signed_tx(&api_addr, &raw_tx).await?;

    wait_for_raw(&rpc_state, &raw_tx).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_rpc_send_raw_tx_is_broadcast() -> anyhow::Result<()> {
    let _guard = acquire_e2e_lock().await;
    let (api_addr, rpc_state) = setup_e2e().await?;
    let raw_tx = build_signed_tx()?;

    let result_hash = send_signed_tx_via_rpc(&api_addr, &raw_tx).await?;
    let expected_hash = json_hex_hash(&raw_tx)
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(result_hash, expected_hash);

    wait_for_raw(&rpc_state, &raw_tx).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_signed_tx_with_valid_after_is_broadcast() -> anyhow::Result<()> {
    let _guard = acquire_e2e_lock().await;
    let (api_addr, rpc_state) = setup_e2e().await?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let raw_tx = build_signed_tx_with_valid_after(Some(now + 2))?;

    send_signed_tx(&api_addr, &raw_tx).await?;

    assert_not_broadcast_within(&rpc_state, &raw_tx, Duration::from_secs(1)).await?;
    wait_for_raw_with_deadline(&rpc_state, &raw_tx, Duration::from_secs(6)).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_cancel_group_prevents_broadcast() -> anyhow::Result<()> {
    let _guard = acquire_e2e_lock().await;
    let (api_addr, rpc_state) = setup_e2e().await?;
    let signer = PrivateKeySigner::random();
    let nonce_key = build_group_nonce_key(1, 11);
    let group_id = group_id_from_nonce_key(nonce_key);
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let raw_tx = build_group_signed_tx_with_valid_after(&signer, nonce_key, Some(now + 2))?;

    send_signed_tx(&api_addr, &raw_tx).await?;

    let auth_header = build_cancel_auth(&signer, group_id)?;
    cancel_group(&api_addr, signer.address(), group_id, &auth_header).await?;

    assert_not_broadcast_within(&rpc_state, &raw_tx, Duration::from_secs(5)).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_cancel_single_tx_marks_stale_by_nonce() -> anyhow::Result<()> {
    let _guard = acquire_e2e_lock().await;
    let (api_addr, rpc_state) = setup_e2e().await?;
    let raw_tx = build_signed_tx()?;

    send_signed_tx(&api_addr, &raw_tx).await?;

    rpc_state.current_nonce.store(1, Ordering::SeqCst);
    let tx_hash = json_hex_hash(&raw_tx)
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing tx hash"))?
        .to_string();
    let response = cancel_transaction(&api_addr, &tx_hash, CHAIN_ID).await?;
    let status = response
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(status, "stale_by_nonce");

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_list_groups_includes_start_end_and_active_filter() -> anyhow::Result<()> {
    let _guard = acquire_e2e_lock().await;
    let (api_addr, _rpc_state) = setup_e2e().await?;
    let signer = PrivateKeySigner::random();
    let sender_hex = format!("0x{}", hex::encode(signer.address().as_slice()));
    let nonce_key_one = build_group_nonce_key(1, 11);
    let nonce_key_two = build_group_nonce_key(1, 22);
    let group_one = group_id_from_nonce_key(nonce_key_one);
    let group_two = group_id_from_nonce_key(nonce_key_two);
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

    let raw_one = build_group_signed_tx_with_valid_after(&signer, nonce_key_one, Some(now + 30))?;
    let raw_two = build_group_signed_tx_with_valid_after(&signer, nonce_key_one, Some(now + 60))?;
    let raw_three = build_group_signed_tx_with_valid_after(&signer, nonce_key_two, None)?;

    send_signed_tx(&api_addr, &raw_one).await?;
    send_signed_tx(&api_addr, &raw_two).await?;
    send_signed_tx(&api_addr, &raw_three).await?;

    let group_one_hex = format!("0x{}", hex::encode(group_one));
    let group_two_hex = format!("0x{}", hex::encode(group_two));

    let group_one_txs = list_transactions(
        &api_addr,
        &format!("sender={sender_hex}&groupId={group_one_hex}&chainId={CHAIN_ID}"),
    )
    .await?;
    let mut eligible_times: Vec<i64> = group_one_txs
        .iter()
        .filter_map(|tx| tx.get("eligibleAt").and_then(Value::as_i64))
        .collect();
    eligible_times.sort_unstable();
    let expected_start = *eligible_times
        .first()
        .ok_or_else(|| anyhow::anyhow!("missing eligibleAt for group one"))?;
    let expected_end = *eligible_times
        .last()
        .ok_or_else(|| anyhow::anyhow!("missing eligibleAt for group one"))?;

    let groups_all = list_groups(&api_addr, &sender_hex, "chainId=42431").await?;

    let group_one_json = find_group(&groups_all, &group_one_hex)
        .ok_or_else(|| anyhow::anyhow!("group one not found"))?;
    let start_at = group_one_json
        .get("startAt")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let end_at = group_one_json
        .get("endAt")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    assert_eq!(start_at as i64, expected_start);
    assert_eq!(end_at as i64, expected_end);

    assert!(find_group(&groups_all, &group_two_hex).is_some());

    tokio::time::sleep(Duration::from_secs(2)).await;
    let groups_active = list_groups(&api_addr, &sender_hex, "chainId=42431&active=true").await?;
    let now_ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    for group in &groups_active {
        let end_at = group
            .get("endAt")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        assert!(end_at > now_ts, "active group has endAt <= now");
    }
    assert!(find_group(&groups_active, &group_one_hex).is_some());
    assert!(find_group(&groups_active, &group_two_hex).is_none());

    Ok(())
}

async fn send_signed_tx(api_addr: &SocketAddr, raw_tx: &str) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{api_addr}/v1/transactions"))
        .json(&serde_json::json!({
            "chainId": CHAIN_ID,
            "transactions": [raw_tx]
        }))
        .send()
        .await?;

    assert!(resp.status().is_success());

    Ok(())
}

async fn cancel_group(
    api_addr: &SocketAddr,
    sender: Address,
    group_id: [u8; 16],
    auth_header: &str,
) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let sender_hex = format!("0x{}", hex::encode(sender.as_slice()));
    let group_hex = format!("0x{}", hex::encode(group_id));
    let resp = client
        .post(format!(
            "http://{api_addr}/v1/senders/{sender_hex}/groups/{group_hex}/cancel"
        ))
        .header("Authorization", auth_header)
        .send()
        .await?;

    assert!(resp.status().is_success());
    let body: Value = resp.json().await?;
    let canceled = body
        .get("canceled")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    assert_eq!(canceled, 1);

    Ok(())
}

async fn cancel_transaction(
    api_addr: &SocketAddr,
    tx_hash: &str,
    chain_id: u64,
) -> anyhow::Result<Value> {
    let client = reqwest::Client::new();
    let resp = client
        .delete(format!(
            "http://{api_addr}/v1/transactions/{tx_hash}?chainId={chain_id}"
        ))
        .send()
        .await?;

    assert!(resp.status().is_success());
    let body: Value = resp.json().await?;

    Ok(body)
}

async fn list_groups(
    api_addr: &SocketAddr,
    sender_hex: &str,
    query: &str,
) -> anyhow::Result<Vec<Value>> {
    let client = reqwest::Client::new();
    let url = if query.is_empty() {
        format!("http://{api_addr}/v1/senders/{sender_hex}/groups")
    } else {
        format!("http://{api_addr}/v1/senders/{sender_hex}/groups?{query}")
    };
    let resp = client.get(url).send().await?;
    assert!(resp.status().is_success());
    let body: Vec<Value> = resp.json().await?;
    Ok(body)
}

fn find_group<'a>(groups: &'a [Value], group_id: &str) -> Option<&'a Value> {
    groups.iter().find(|group| {
        group
            .get("groupId")
            .and_then(Value::as_str)
            .is_some_and(|value| value == group_id)
    })
}

async fn send_signed_tx_via_rpc(api_addr: &SocketAddr, raw_tx: &str) -> anyhow::Result<String> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{api_addr}/rpc"))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_sendRawTransaction",
            "params": [raw_tx],
        }))
        .send()
        .await?;

    assert!(resp.status().is_success());
    let body: Value = resp.json().await?;
    assert_eq!(body.get("jsonrpc").and_then(Value::as_str), Some("2.0"));
    let result = body
        .get("result")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing result in rpc response"))?;

    Ok(result.to_string())
}

async fn list_transactions(api_addr: &SocketAddr, query: &str) -> anyhow::Result<Vec<Value>> {
    let client = reqwest::Client::new();
    let url = if query.is_empty() {
        format!("http://{api_addr}/v1/transactions")
    } else {
        format!("http://{api_addr}/v1/transactions?{query}")
    };
    let resp = client.get(url).send().await?;
    assert!(resp.status().is_success());
    let body: Vec<Value> = resp.json().await?;
    Ok(body)
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
        "eth_getTransactionCount" => {
            Value::from(format!("0x{:x}", state.current_nonce.load(Ordering::SeqCst)))
        }
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
    wait_for_raw_with_deadline(state, expected, Duration::from_secs(5)).await
}

async fn wait_for_raw_with_deadline(
    state: &RpcState,
    expected: &str,
    deadline: Duration,
) -> anyhow::Result<()> {
    let expected = expected.to_string();

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

async fn assert_not_broadcast_within(
    state: &RpcState,
    expected: &str,
    window: Duration,
) -> anyhow::Result<()> {
    let expected = expected.to_string();
    let early = timeout(window, async {
        loop {
            let seen = state.seen_raw.lock().await;
            if seen.iter().any(|raw| raw == &expected) {
                return true;
            }
            drop(seen);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;

    if early.is_ok() {
        anyhow::bail!("broadcast occurred before valid_after");
    }

    Ok(())
}

fn build_signed_tx() -> anyhow::Result<String> {
    build_signed_tx_with_valid_after(None)
}

fn build_signed_tx_with_valid_after(valid_after: Option<u64>) -> anyhow::Result<String> {
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
        valid_after,
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

fn build_group_signed_tx_with_valid_after(
    signer: &PrivateKeySigner,
    nonce_key: U256,
    valid_after: Option<u64>,
) -> anyhow::Result<String> {
    let call = Call {
        to: TxKind::Call(Address::ZERO),
        value: U256::ZERO,
        input: Bytes::default(),
    };

    let tx = TempoTransaction {
        chain_id: CHAIN_ID,
        fee_token: None,
        max_priority_fee_per_gas: 1,
        max_fee_per_gas: 1,
        gas_limit: 21000,
        calls: vec![call],
        access_list: alloy::rpc::types::AccessList::default(),
        nonce_key,
        nonce: 0,
        fee_payer_signature: None,
        valid_before: None,
        valid_after,
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

fn build_cancel_auth(signer: &PrivateKeySigner, group_id: [u8; 16]) -> anyhow::Result<String> {
    let hash = keccak256(group_id);
    let signature = signer.sign_hash_sync(&hash)?;
    Ok(format!("Signature 0x{}", hex::encode(signature.as_bytes())))
}

fn group_id_from_nonce_key(nonce_key: U256) -> [u8; 16] {
    let bytes = nonce_key.to_be_bytes::<32>();
    let hash = keccak256(bytes);
    let mut group_id = [0u8; 16];
    group_id.copy_from_slice(&hash[..16]);
    group_id
}

fn build_group_nonce_key(scope_id: u64, group_id: u32) -> U256 {
    let mut bytes = [0u8; 32];
    bytes[..4].copy_from_slice(b"NKG1");
    bytes[4] = 0x01;
    bytes[5] = 0x01;
    bytes[6..8].copy_from_slice(&0u16.to_be_bytes());
    bytes[8..16].copy_from_slice(&scope_id.to_be_bytes());
    bytes[16..20].copy_from_slice(&group_id.to_be_bytes());
    U256::from_be_slice(&bytes)
}

async fn setup_e2e() -> anyhow::Result<(SocketAddr, RpcState)> {
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
            expiry_soon_window_seconds: 3600,
            expiry_soon_retry_max_ms: 5000,
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

    let app = api::router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let api_addr = listener.local_addr()?;
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("api server failed");
    });

    Ok((api_addr, rpc_state))
}

async fn acquire_e2e_lock() -> tokio::sync::MutexGuard<'static, ()> {
    E2E_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

fn env_var(key: &str) -> Option<String> {
    std::env::var(key).ok()
}
