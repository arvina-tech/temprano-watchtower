use std::collections::{BTreeMap, BTreeSet};

use alloy::network::TransactionBuilder;
use alloy::providers::Provider;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, TimeZone, Utc};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::error;

use crate::db;
use crate::models::{NewTx, TxRecord, TxStatus};
use crate::scheduler;
use crate::state::AppState;
use crate::tx::parse_raw_tx;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/rpc", post(rpc_handler))
        .route(
            "/v1/transactions",
            post(submit_transactions).get(list_transactions),
        )
        .route("/v1/transactions/:tx_hash", get(get_transaction))
        .route("/v1/senders/:sender/groups/:group_id", get(get_group))
        .route(
            "/v1/senders/:sender/groups/:group_id/cancel",
            post(cancel_group),
        )
        .with_state(state)
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(serde_json::json!({ "error": self.message }));
        (self.status, body).into_response()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitRequest {
    chain_id: u64,
    transactions: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SubmitResponse {
    results: Vec<SubmitResult>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SubmitResult {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tx_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sender: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nonce_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nonce: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    eligible_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<GroupInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    already_known: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GroupInfo {
    group_id: String,
    aux: String,
    version: u8,
    flags: u8,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TxInfo {
    chain_id: u64,
    tx_hash: String,
    sender: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    fee_payer: Option<String>,
    nonce_key: String,
    nonce: u64,
    valid_after: Option<i64>,
    valid_before: Option<i64>,
    eligible_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<i64>,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<GroupInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_action_at: Option<i64>,
    attempts: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_broadcast_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    receipt: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TxListQuery {
    chain_id: Option<u64>,
    sender: Option<String>,
    group_id: Option<String>,
    status: Option<String>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChainQuery {
    chain_id: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GroupResponse {
    sender: String,
    group_id: String,
    members: Vec<GroupMember>,
    cancel_plan: CancelPlan,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GroupMember {
    tx_hash: String,
    nonce_key: String,
    nonce: u64,
    status: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CancelPlan {
    by_nonce_key: Vec<CancelPlanNonceKey>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CancelPlanNonceKey {
    nonce_key: String,
    nonces: Vec<u64>,
    already_invalidated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CancelResponse {
    canceled: usize,
    tx_hashes: Vec<String>,
}

#[derive(Debug)]
struct RpcRequest {
    id: Value,
    method: String,
    params: Vec<Value>,
}

#[derive(Debug)]
struct RpcError {
    code: i64,
    message: String,
}

async fn submit_transactions(
    State(state): State<AppState>,
    Json(payload): Json<SubmitRequest>,
) -> Result<Json<SubmitResponse>, ApiError> {
    let SubmitRequest {
        chain_id,
        transactions,
    } = payload;
    if state.rpcs.chain(chain_id).is_none() {
        return Err(ApiError::bad_request(format!(
            "unsupported chainId {}",
            chain_id
        )));
    }
    let mut prepared = Vec::with_capacity(transactions.len());
    for (index, raw_tx) in transactions.into_iter().enumerate() {
        let new_tx = match prepare_new_tx(chain_id, &raw_tx) {
            Ok(new_tx) => new_tx,
            Err(err) => {
                let message = format!("transaction {index} invalid: {}", err.message);
                error!(error = %message, "failed to submit transactions");
                return Err(ApiError::bad_request(message));
            }
        };
        prepared.push(new_tx);
    }

    let (records, already_known_flags) = store_transactions(&state, prepared).await?;

    let mut results = Vec::with_capacity(records.len());
    for (record, already_known) in records.into_iter().zip(already_known_flags) {
        results.push(SubmitResult {
            ok: true,
            tx_hash: Some(bytes_to_hex(&record.tx_hash)),
            sender: Some(bytes_to_hex(&record.sender)),
            nonce_key: Some(u256_bytes_to_hex(&record.nonce_key)),
            nonce: Some(record.nonce as u64),
            eligible_at: Some(record.eligible_at.timestamp()),
            expires_at: record.expires_at.map(|ts| ts.timestamp()),
            group: group_info_from_record(&record),
            status: Some(record.status.clone()),
            already_known: Some(already_known),
            error: None,
        });
    }

    Ok(Json(SubmitResponse { results }))
}

fn prepare_new_tx(chain_id: u64, raw_tx: &str) -> Result<NewTx, ApiError> {
    let parsed = parse_raw_tx(raw_tx).map_err(|err| ApiError::bad_request(err.to_string()))?;
    if parsed.chain_id != chain_id {
        return Err(ApiError::bad_request(format!(
            "tx chainId {} does not match request chainId {}",
            parsed.chain_id, chain_id
        )));
    }

    prepare_new_tx_from_parsed(parsed)
}

fn prepare_new_tx_from_parsed(parsed: crate::tx::ParsedTx) -> Result<NewTx, ApiError> {
    let now = Utc::now();
    let valid_after = parsed.valid_after.map(|v| v as i64);
    let valid_before = parsed.valid_before.map(|v| v as i64);

    if let (Some(after), Some(before)) = (valid_after, valid_before)
        && before <= after
    {
        return Err(ApiError::bad_request("invalid validity window"));
    }

    let expires_at = match valid_before {
        Some(ts) => Some(datetime_from_ts(ts)?),
        None => None,
    };

    if let Some(expires_at) = expires_at
        && expires_at <= now
    {
        return Err(ApiError::bad_request("transaction already expired"));
    }

    let eligible_at = match valid_after {
        Some(ts) if ts > now.timestamp() => datetime_from_ts(ts)?,
        _ => now,
    };

    Ok(NewTx {
        chain_id: parsed.chain_id as i64,
        tx_hash: parsed.tx_hash.as_slice().to_vec(),
        raw_tx: parsed.raw_tx.clone(),
        sender: parsed.sender.as_slice().to_vec(),
        fee_payer: parsed.fee_payer.map(|addr| addr.as_slice().to_vec()),
        nonce_key: u256_to_bytes(parsed.nonce_key),
        nonce: parsed.nonce as i64,
        valid_after,
        valid_before,
        eligible_at,
        expires_at,
        status: TxStatus::Queued.as_str().to_string(),
        group_id: parsed.group.as_ref().map(|g| g.group_id.to_vec()),
        group_aux: parsed.group.as_ref().map(|g| g.aux.to_vec()),
        group_version: parsed.group.as_ref().map(|g| g.version as i16),
        group_flags: parsed.group.as_ref().map(|g| g.flags as i16),
        next_action_at: eligible_at,
    })
}

async fn rpc_handler(
    State(state): State<AppState>,
    Json(payload): Json<Value>,
) -> Json<Value> {
    let request = match parse_rpc_request(&payload) {
        Ok(request) => request,
        Err(err) => return rpc_error_response(Value::Null, err),
    };

    if request.method != "eth_sendRawTransaction" {
        return rpc_error_response(
            request.id,
            RpcError {
                code: -32601,
                message: "method not found".to_string(),
            },
        );
    }

    let raw_tx = match request.params.first().and_then(|value| value.as_str()) {
        Some(raw_tx) => raw_tx,
        None => {
            return rpc_error_response(
                request.id,
                RpcError {
                    code: -32602,
                    message: "expected raw transaction hex string".to_string(),
                },
            );
        }
    };

    let parsed = match parse_raw_tx(raw_tx) {
        Ok(parsed) => parsed,
        Err(err) => {
            return rpc_error_response(
                request.id,
                RpcError {
                    code: -32602,
                    message: err.to_string(),
                },
            );
        }
    };

    if state.rpcs.chain(parsed.chain_id).is_none() {
        return rpc_error_response(
            request.id,
            RpcError {
                code: -32602,
                message: format!("unsupported chainId {}", parsed.chain_id),
            },
        );
    }

    let new_tx = match prepare_new_tx_from_parsed(parsed) {
        Ok(new_tx) => new_tx,
        Err(err) => {
            let code = match err.status {
                StatusCode::BAD_REQUEST => -32602,
                _ => -32603,
            };
            return rpc_error_response(
                request.id,
                RpcError {
                    code,
                    message: err.message,
                },
            );
        }
    };

    let result = store_transactions(&state, vec![new_tx]).await;
    let record = match result {
        Ok((mut records, _)) => records
            .pop()
            .expect("store_transactions returns at least one record"),
        Err(err) => {
            return rpc_error_response(
                request.id,
                RpcError {
                    code: -32603,
                    message: err.message,
                },
            );
        }
    };

    rpc_success_response(request.id, Value::from(bytes_to_hex(&record.tx_hash)))
}

fn parse_rpc_request(payload: &Value) -> Result<RpcRequest, RpcError> {
    let obj = payload
        .as_object()
        .ok_or_else(|| RpcError::invalid_request("expected JSON object"))?;

    if let Some(version) = obj.get("jsonrpc").and_then(|value| value.as_str())
        && version != "2.0"
    {
        return Err(RpcError::invalid_request("unsupported jsonrpc version"));
    }

    let method = obj
        .get("method")
        .and_then(|value| value.as_str())
        .ok_or_else(|| RpcError::invalid_request("missing method"))?
        .to_string();

    let params = match obj.get("params") {
        Some(value) => value
            .as_array()
            .cloned()
            .ok_or_else(|| RpcError::invalid_params("params must be an array"))?,
        None => Vec::new(),
    };

    let id = obj.get("id").cloned().unwrap_or(Value::Null);

    Ok(RpcRequest { id, method, params })
}

fn rpc_success_response(id: Value, result: Value) -> Json<Value> {
    Json(serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    }))
}

fn rpc_error_response(id: Value, err: RpcError) -> Json<Value> {
    Json(serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": err.code,
            "message": err.message,
        },
    }))
}

impl RpcError {
    fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            code: -32600,
            message: message.into(),
        }
    }

    fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }
}

async fn get_transaction(
    State(state): State<AppState>,
    Path(tx_hash): Path<String>,
    Query(query): Query<ChainQuery>,
) -> Result<Json<TxInfo>, ApiError> {
    let tx_hash = parse_fixed_hex(&tx_hash, 32)?;
    let chain_id = query.chain_id.map(|id| id as i64);
    let record = db::get_tx_by_hash(&state.db, chain_id, &tx_hash)
        .await
        .map_err(|err| ApiError::internal(err.to_string()))?
        .ok_or_else(|| ApiError::not_found("transaction not found"))?;

    Ok(Json(tx_info_from(&record)?))
}

async fn list_transactions(
    State(state): State<AppState>,
    Query(query): Query<TxListQuery>,
) -> Result<Json<Vec<TxInfo>>, ApiError> {
    let sender = match query.sender {
        Some(value) => Some(parse_fixed_hex(&value, 20)?),
        None => None,
    };
    let group_id = match query.group_id {
        Some(value) => Some(parse_fixed_hex(&value, 16)?),
        None => None,
    };

    let filters = db::TxFilters {
        chain_id: query.chain_id.map(|id| id as i64),
        sender,
        group_id,
        status: query.status,
        limit: query.limit.unwrap_or(100),
    };

    let records = db::list_txs(&state.db, filters)
        .await
        .map_err(|err| ApiError::internal(err.to_string()))?;

    let mut out = Vec::with_capacity(records.len());
    for record in &records {
        out.push(tx_info_from(record)?);
    }

    Ok(Json(out))
}

async fn get_group(
    State(state): State<AppState>,
    Path((sender, group_id)): Path<(String, String)>,
    Query(query): Query<ChainQuery>,
) -> Result<Json<GroupResponse>, ApiError> {
    let sender_bytes = parse_fixed_hex(&sender, 20)?;
    let group_bytes = parse_fixed_hex(&group_id, 16)?;

    let mut records = db::get_group_txs(
        &state.db,
        &sender_bytes,
        &group_bytes,
        query.chain_id.map(|id| id as i64),
    )
    .await
    .map_err(|err| ApiError::internal(err.to_string()))?;

    if records.is_empty() {
        return Err(ApiError::not_found("group not found"));
    }

    let chain_ids: BTreeSet<i64> = records.iter().map(|record| record.chain_id).collect();
    let chain_id = match query.chain_id {
        Some(id) => id as i64,
        None => {
            if chain_ids.len() == 1 {
                *chain_ids.iter().next().unwrap()
            } else {
                return Err(ApiError::bad_request(
                    "multiple chainIds found; specify chainId",
                ));
            }
        }
    };

    records.sort_by_key(|record| record.nonce);

    let mut members = Vec::with_capacity(records.len());
    for record in &records {
        members.push(GroupMember {
            tx_hash: bytes_to_hex(&record.tx_hash),
            nonce_key: u256_bytes_to_hex(&record.nonce_key),
            nonce: record.nonce as u64,
            status: record.status.clone(),
        });
    }

    let cancel_plan = build_cancel_plan(&state, chain_id as u64, &sender_bytes, &records)
        .await
        .map_err(|err| ApiError::internal(err.to_string()))?;

    Ok(Json(GroupResponse {
        sender: bytes_to_hex(&sender_bytes),
        group_id: bytes_to_hex(&group_bytes),
        members,
        cancel_plan,
    }))
}

async fn cancel_group(
    State(state): State<AppState>,
    Path((sender, group_id)): Path<(String, String)>,
) -> Result<Json<CancelResponse>, ApiError> {
    let sender_bytes = parse_fixed_hex(&sender, 20)?;
    let group_bytes = parse_fixed_hex(&group_id, 16)?;

    let records = db::cancel_group(&state.db, &sender_bytes, &group_bytes)
        .await
        .map_err(|err| ApiError::internal(err.to_string()))?;

    if records.is_empty() {
        return Err(ApiError::not_found("group not found"));
    }

    let mut tx_hashes = Vec::with_capacity(records.len());

    let mut redis = state.redis.clone();
    for record in &records {
        let tx_hash = bytes_to_hex(&record.tx_hash);
        tx_hashes.push(tx_hash.clone());
        let ready_key = ready_key(record.chain_id as u64);
        let retry_key = retry_key(record.chain_id as u64);
        let _: () = redis
            .zrem::<_, _, ()>(ready_key, &tx_hash)
            .await
            .unwrap_or(());
        let _: () = redis
            .zrem::<_, _, ()>(retry_key, &tx_hash)
            .await
            .unwrap_or(());
    }

    Ok(Json(CancelResponse {
        canceled: records.len(),
        tx_hashes,
    }))
}

async fn store_transactions(
    state: &AppState,
    prepared: Vec<NewTx>,
) -> Result<(Vec<TxRecord>, Vec<bool>), ApiError> {
    let mut db_tx = state
        .db
        .begin()
        .await
        .map_err(|err| ApiError::internal(err.to_string()))?;

    let mut records = Vec::with_capacity(prepared.len());
    let mut already_known_flags = Vec::with_capacity(prepared.len());
    for new_tx in prepared {
        let (record, already_known) = db::insert_tx(&mut db_tx, &new_tx)
            .await
            .map_err(|err| ApiError::internal(err.to_string()))?;
        records.push(record);
        already_known_flags.push(already_known);
    }

    db_tx
        .commit()
        .await
        .map_err(|err| ApiError::internal(err.to_string()))?;

    scheduler::schedule_records(state, &records)
        .await
        .map_err(|err| ApiError::internal(err.to_string()))?;

    Ok((records, already_known_flags))
}

fn tx_info_from(record: &TxRecord) -> Result<TxInfo, ApiError> {
    Ok(TxInfo {
        chain_id: record.chain_id as u64,
        tx_hash: bytes_to_hex(&record.tx_hash),
        sender: bytes_to_hex(&record.sender),
        fee_payer: record.fee_payer.as_ref().map(|v| bytes_to_hex(v)),
        nonce_key: u256_bytes_to_hex(&record.nonce_key),
        nonce: record.nonce as u64,
        valid_after: record.valid_after,
        valid_before: record.valid_before,
        eligible_at: record.eligible_at.timestamp(),
        expires_at: record.expires_at.map(|ts| ts.timestamp()),
        status: record.status.clone(),
        group: group_info_from_record(record),
        next_action_at: record.next_action_at.map(|ts| ts.timestamp()),
        attempts: record.attempts,
        last_error: record.last_error.clone(),
        last_broadcast_at: record.last_broadcast_at.map(|ts| ts.timestamp()),
        receipt: record.receipt.clone(),
    })
}

fn group_info_from_record(record: &TxRecord) -> Option<GroupInfo> {
    let group_id = record.group_id.as_ref()?;
    let aux = record.group_aux.as_ref()?;
    let version = record.group_version? as u8;
    let flags = record.group_flags.map(|value| value as u8).unwrap_or(0);

    Some(GroupInfo {
        group_id: bytes_to_hex(group_id),
        aux: bytes_to_hex(aux),
        version,
        flags,
    })
}

async fn build_cancel_plan(
    state: &AppState,
    chain_id: u64,
    sender: &[u8],
    records: &[TxRecord],
) -> anyhow::Result<CancelPlan> {
    let mut groups: BTreeMap<Vec<u8>, Vec<u64>> = BTreeMap::new();
    for record in records {
        groups
            .entry(record.nonce_key.clone())
            .or_default()
            .push(record.nonce as u64);
    }

    let chain = state
        .rpcs
        .chain(chain_id)
        .ok_or_else(|| anyhow::anyhow!("unknown chain id"))?;

    let sender_addr = parse_address(sender)?;

    let mut by_nonce_key = Vec::new();

    for (nonce_key_bytes, mut nonces) in groups {
        nonces.sort_unstable();
        nonces.dedup();
        let nonce_key =
            u256_from_bytes(&nonce_key_bytes).map_err(|err| anyhow::anyhow!(err.message))?;
        let current_nonce = fetch_current_nonce(chain, sender_addr, nonce_key).await?;
        let max_nonce = *nonces.last().unwrap_or(&0);

        by_nonce_key.push(CancelPlanNonceKey {
            nonce_key: u256_bytes_to_hex(&nonce_key_bytes),
            nonces,
            already_invalidated: current_nonce > max_nonce,
        });
    }

    Ok(CancelPlan { by_nonce_key })
}

fn ready_key(chain_id: u64) -> String {
    format!("watchtower:ready:{chain_id}")
}

fn retry_key(chain_id: u64) -> String {
    format!("watchtower:retry:{chain_id}")
}

fn parse_fixed_hex(value: &str, len: usize) -> Result<Vec<u8>, ApiError> {
    let bytes = parse_hex(value)?;
    if bytes.len() != len {
        return Err(ApiError::bad_request(format!(
            "expected {len} bytes, got {}",
            bytes.len()
        )));
    }
    Ok(bytes)
}

fn parse_hex(value: &str) -> Result<Vec<u8>, ApiError> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    if !value.len().is_multiple_of(2) {
        return Err(ApiError::bad_request("invalid hex length"));
    }
    hex::decode(value).map_err(|err| ApiError::bad_request(err.to_string()))
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(bytes))
}

fn u256_bytes_to_hex(bytes: &[u8]) -> String {
    let value = u256_from_bytes(bytes).unwrap_or_default();
    let mut hex = format!("{:x}", value);
    if hex.is_empty() {
        hex = "0".to_string();
    }
    format!("0x{hex}")
}

fn u256_from_bytes(bytes: &[u8]) -> Result<alloy::primitives::U256, ApiError> {
    if bytes.len() > 32 {
        return Err(ApiError::bad_request("nonce_key too large"));
    }
    let mut buf = [0u8; 32];
    let offset = 32 - bytes.len();
    buf[offset..].copy_from_slice(bytes);
    Ok(alloy::primitives::U256::from_be_slice(&buf))
}

fn u256_to_bytes(value: alloy::primitives::U256) -> Vec<u8> {
    value.to_be_bytes::<32>().to_vec()
}

fn datetime_from_ts(ts: i64) -> Result<DateTime<Utc>, ApiError> {
    Utc.timestamp_opt(ts, 0)
        .single()
        .ok_or_else(|| ApiError::bad_request("invalid timestamp"))
}

fn parse_address(bytes: &[u8]) -> anyhow::Result<alloy::primitives::Address> {
    if bytes.len() != 20 {
        anyhow::bail!("invalid address length");
    }
    let mut data = [0u8; 20];
    data.copy_from_slice(bytes);
    Ok(alloy::primitives::Address::from(data))
}

async fn fetch_current_nonce(
    chain: &crate::rpc::ChainRpc,
    sender: alloy::primitives::Address,
    nonce_key: alloy::primitives::U256,
) -> anyhow::Result<u64> {
    if nonce_key.is_zero() {
        let provider = chain
            .http
            .first()
            .ok_or_else(|| anyhow::anyhow!("missing provider"))?;
        let nonce = provider.get_transaction_count(sender).await?;
        return Ok(nonce);
    }

    let call = tempo_alloy::contracts::precompiles::INonce::getNonceCall {
        account: sender,
        nonceKey: nonce_key,
    };
    let mut req = tempo_alloy::rpc::TempoTransactionRequest::default();
    req.set_kind(alloy::primitives::TxKind::Call(nonce_precompile_address()));
    req.set_call(&call);

    let provider = chain
        .http
        .first()
        .ok_or_else(|| anyhow::anyhow!("missing provider"))?;
    let output = provider
        .call(req)
        .decode_resp::<tempo_alloy::contracts::precompiles::INonce::getNonceCall>()
        .await??;
    Ok(output)
}

fn nonce_precompile_address() -> alloy::primitives::Address {
    alloy::primitives::Address::from_slice(
        &hex::decode("4e4f4e4345000000000000000000000000000000").expect("valid precompile"),
    )
}

#[cfg(test)]
mod tests {
    use super::{parse_fixed_hex, u256_bytes_to_hex, u256_from_bytes};
    use alloy::primitives::U256;

    #[test]
    fn parse_fixed_hex_enforces_length() {
        let ok = parse_fixed_hex("0x0102", 2).expect("valid");
        assert_eq!(ok, vec![0x01, 0x02]);

        let err = parse_fixed_hex("0x01", 2).unwrap_err();
        assert!(err.message.contains("expected 2 bytes"));
    }

    #[test]
    fn u256_from_bytes_handles_short() {
        let value = u256_from_bytes(&[0x01, 0x00]).expect("u256");
        assert_eq!(value, U256::from(0x0100u64));
        assert_eq!(u256_bytes_to_hex(&[0x01]), "0x1");
    }
}
