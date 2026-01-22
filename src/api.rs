use std::collections::{BTreeMap, BTreeSet};

use alloy::network::TransactionBuilder;
use alloy::primitives::{Signature, keccak256};
use alloy::providers::Provider;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, TimeZone, Utc};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx_pg_uint::{OptionPgUint, PgU64};
use tracing::{error, info};

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
        .route("/v1/senders/:sender/groups", get(list_groups))
        .route("/v1/senders/:sender/groups/:group_id", get(get_group))
        .route(
            "/v1/senders/:sender/groups/:group_id/cancel",
            post(cancel_group),
        )
        .with_state(state)
}

const GROUP_SIGNATURE_HEADER: &str = "authorization";

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

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
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
    group_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    eligible_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    already_known: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    group_id: Option<String>,
    valid_after: Option<u64>,
    valid_before: Option<u64>,
    eligible_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<i64>,
    status: String,
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
    #[serde(default, deserialize_with = "crate::serde_helpers::deserialize_string_or_vec")]
    status: Vec<String>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChainQuery {
    chain_id: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GroupListQuery {
    chain_id: Option<u64>,
    limit: Option<i64>,
    active: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GroupSummary {
    chain_id: u64,
    group_id: String,
    start_at: i64,
    end_at: i64,
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
            nonce: Some(record.nonce.to_uint()),
            group_id: record.group_id.as_ref().map(|value| bytes_to_hex(value)),
            eligible_at: Some(record.eligible_at.timestamp()),
            expires_at: record.expires_at.map(|ts| ts.timestamp()),
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
    let valid_after = parsed.valid_after;
    let valid_before = parsed.valid_before;
    let now_ts = u64::try_from(now.timestamp())
        .map_err(|_| ApiError::internal("system clock before unix epoch"))?;
    let valid_after_pg = valid_after.map(PgU64::from);
    let valid_before_pg = valid_before.map(PgU64::from);

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
        Some(ts) if ts > now_ts => datetime_from_ts(ts)?,
        _ => now,
    };

    let nonce_key_bytes = u256_to_bytes(parsed.nonce_key);
    if is_random_nonce_key(&nonce_key_bytes) && valid_after.is_some() {
        return Err(ApiError::bad_request(
            "random nonce key requires valid_after to be unset",
        ));
    }

    let group_id = group_id_from_nonce_key(&nonce_key_bytes);

    Ok(NewTx {
        chain_id: PgU64::from(parsed.chain_id),
        tx_hash: parsed.tx_hash.as_slice().to_vec(),
        raw_tx: parsed.raw_tx.clone(),
        sender: parsed.sender.as_slice().to_vec(),
        fee_payer: parsed.fee_payer.map(|addr| addr.as_slice().to_vec()),
        nonce_key: nonce_key_bytes,
        nonce: PgU64::from(parsed.nonce),
        valid_after: valid_after_pg,
        valid_before: valid_before_pg,
        eligible_at,
        expires_at,
        status: TxStatus::Queued.as_str().to_string(),
        group_id: Some(group_id),
        next_action_at: eligible_at,
    })
}

async fn rpc_handler(State(state): State<AppState>, Json(payload): Json<Value>) -> Json<Value> {
    let request = match parse_rpc_request(&payload) {
        Ok(request) => request,
        Err(err) => return rpc_error_response(Value::Null, err),
    };

    if request.method != "eth_sendRawTransaction" {
        return rpc_error_response(
            request.id,
            RpcError {
                code: -32601,
                message: format!("method not found: {}", request.method),
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
    let chain_id = query.chain_id;
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

    let statuses = query
        .status
        .iter()
        .map(|status| {
            TxStatus::try_from(status.as_str())
                .map_err(|_| ApiError::bad_request(format!("invalid status: {status}")))
        })
        .collect::<Result<Vec<TxStatus>, ApiError>>()?;

    let filters = db::TxFilters {
        chain_id: query.chain_id,
        sender,
        group_id,
        statuses,
        limit: query.limit.unwrap_or(100).min(500),
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

async fn list_groups(
    State(state): State<AppState>,
    Path(sender): Path<String>,
    Query(query): Query<GroupListQuery>,
) -> Result<Json<Vec<GroupSummary>>, ApiError> {
    let sender_bytes = parse_fixed_hex(&sender, 20)?;

    let limit = query.limit.unwrap_or(100).min(500);
    let active_only = query.active.unwrap_or(false);
    let records =
        db::list_sender_groups(&state.db, &sender_bytes, query.chain_id, limit, active_only)
            .await
            .map_err(|err| ApiError::internal(err.to_string()))?;

    let mut out = Vec::with_capacity(records.len());
    for record in &records {
        out.push(GroupSummary {
            chain_id: record.chain_id.to_uint(),
            group_id: bytes_to_hex(&record.group_id),
            start_at: record.start_at.timestamp(),
            end_at: record.end_at.timestamp(),
        });
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

    let mut records = db::get_group_txs(&state.db, &sender_bytes, &group_bytes, query.chain_id)
        .await
        .map_err(|err| ApiError::internal(err.to_string()))?;

    if records.is_empty() {
        return Err(ApiError::not_found("group not found"));
    }

    let chain_ids: BTreeSet<u64> = records
        .iter()
        .map(|record| record.chain_id.to_uint())
        .collect();
    let chain_id = match query.chain_id {
        Some(id) => id,
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

    records.sort_by_key(|record| record.nonce.to_uint());

    let mut members = Vec::with_capacity(records.len());
    for record in &records {
        members.push(GroupMember {
            tx_hash: bytes_to_hex(&record.tx_hash),
            nonce_key: u256_bytes_to_hex(&record.nonce_key),
            nonce: record.nonce.to_uint(),
            status: record.status.clone(),
        });
    }

    let cancel_plan = build_cancel_plan(&state, chain_id, &sender_bytes, &records)
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
    headers: HeaderMap,
    Path((sender, group_id)): Path<(String, String)>,
) -> Result<Json<CancelResponse>, ApiError> {
    let sender_bytes = parse_fixed_hex(&sender, 20)?;
    let group_bytes = parse_fixed_hex(&group_id, 16)?;
    verify_group_signature(&headers, &sender_bytes, &group_bytes)?;

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
        let chain_id = record.chain_id.to_uint();
        let ready_key = ready_key(chain_id);
        let retry_key = retry_key(chain_id);
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

    let mut group_nonce_keys: BTreeMap<(u64, Vec<u8>, Vec<u8>), Vec<u8>> = BTreeMap::new();
    let mut group_windows: BTreeMap<(u64, Vec<u8>, Vec<u8>), Vec<(u64, Option<u64>)>> =
        BTreeMap::new();
    for new_tx in &prepared {
        let Some(group_id) = new_tx.group_id.as_ref() else {
            continue;
        };
        let key = (
            new_tx.chain_id.to_uint(),
            new_tx.sender.clone(),
            group_id.clone(),
        );
        if let Some(existing) = group_nonce_keys.get(&key) {
            if existing != &new_tx.nonce_key {
                return Err(ApiError::bad_request(
                    "group transactions must share the same nonce_key",
                ));
            }
        } else {
            group_nonce_keys.insert(key.clone(), new_tx.nonce_key.clone());
        }
        group_windows
            .entry(key)
            .or_default()
            .push((
                new_tx.nonce.to_uint(),
                new_tx.valid_before.as_ref().map(|value| value.to_uint()),
            ));
    }

    for ((chain_id, sender, group_id), nonce_key) in &group_nonce_keys {
        let existing = db::get_group_nonce_key(&mut db_tx, *chain_id, sender, group_id)
            .await
            .map_err(|err| ApiError::internal(err.to_string()))?;
        if let Some(existing) = existing {
            if existing != *nonce_key {
                return Err(ApiError::bad_request(
                    "group transactions must share the same nonce_key",
                ));
            }
        }
    }

    for ((chain_id, sender, group_id), mut windows) in group_windows {
        let existing = db::get_group_nonce_windows(&mut db_tx, chain_id, &sender, &group_id)
            .await
            .map_err(|err| ApiError::internal(err.to_string()))?;
        for row in existing {
            windows.push((
                row.nonce.to_uint(),
                row.valid_before.map(|value| value.to_uint()),
            ));
        }
        validate_nonce_valid_before_order(&windows)?;
    }

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

    for (record, already_known) in records.iter().zip(already_known_flags.iter()) {
        if *already_known {
            continue;
        }
        if record.status.as_str() != TxStatus::Queued.as_str() {
            continue;
        }
        info!(
            chain_id = %record.chain_id.to_uint(),
            tx_hash = %bytes_to_hex(&record.tx_hash),
            eligible_at = record.eligible_at.timestamp(),
            expires_at = ?record.expires_at.map(|ts| ts.timestamp()),
            "transaction queued",
        );
    }

    Ok((records, already_known_flags))
}

fn validate_nonce_valid_before_order(pairs: &[(u64, Option<u64>)]) -> Result<(), ApiError> {
    let mut ordered: Vec<(u64, u64)> = pairs
        .iter()
        .filter_map(|(nonce, valid_before)| valid_before.map(|value| (*nonce, value)))
        .collect();
    if ordered.len() <= 1 {
        return Ok(());
    }
    ordered.sort_by_key(|(nonce, _)| *nonce);
    let mut prev = ordered[0].1;
    for (_, valid_before) in ordered.into_iter().skip(1) {
        if valid_before < prev {
            return Err(ApiError::bad_request(
                "group valid_before order must match nonce order",
            ));
        }
        prev = valid_before;
    }
    Ok(())
}

fn tx_info_from(record: &TxRecord) -> Result<TxInfo, ApiError> {
    Ok(TxInfo {
        chain_id: record.chain_id.to_uint(),
        tx_hash: bytes_to_hex(&record.tx_hash),
        sender: bytes_to_hex(&record.sender),
        fee_payer: record.fee_payer.as_ref().map(|v| bytes_to_hex(v)),
        nonce_key: u256_bytes_to_hex(&record.nonce_key),
        nonce: record.nonce.to_uint(),
        group_id: record.group_id.as_ref().map(|value| bytes_to_hex(value)),
        valid_after: record.valid_after.to_option_uint(),
        valid_before: record.valid_before.to_option_uint(),
        eligible_at: record.eligible_at.timestamp(),
        expires_at: record.expires_at.map(|ts| ts.timestamp()),
        status: record.status.clone(),
        next_action_at: record.next_action_at.map(|ts| ts.timestamp()),
        attempts: record.attempts,
        last_error: record.last_error.clone(),
        last_broadcast_at: record.last_broadcast_at.map(|ts| ts.timestamp()),
        receipt: record.receipt.clone(),
    })
}

async fn build_cancel_plan(
    state: &AppState,
    chain_id: u64,
    sender: &[u8],
    records: &[TxRecord],
) -> anyhow::Result<CancelPlan> {
    let mut nonce_key_bytes = None;
    let mut nonces = Vec::with_capacity(records.len());
    for record in records {
        if let Some(existing) = nonce_key_bytes.as_ref() {
            if existing != &record.nonce_key {
                anyhow::bail!("group has multiple nonce keys");
            }
        } else {
            nonce_key_bytes = Some(record.nonce_key.clone());
        }
        nonces.push(record.nonce.to_uint());
    }

    nonces.sort_unstable();
    nonces.dedup();

    let nonce_key_bytes = nonce_key_bytes.ok_or_else(|| anyhow::anyhow!("missing nonce key"))?;

    let chain = state
        .rpcs
        .chain(chain_id)
        .ok_or_else(|| anyhow::anyhow!("unknown chain id"))?;

    let sender_addr = parse_address(sender)?;

    let current_nonce = fetch_current_nonce(chain, sender_addr, &nonce_key_bytes).await?;
    let max_nonce = *nonces.last().unwrap_or(&0);

    Ok(CancelPlan {
        nonce_key: u256_bytes_to_hex(&nonce_key_bytes),
        nonces,
        already_invalidated: current_nonce
            .map(|nonce| nonce > max_nonce)
            .unwrap_or(false),
    })
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

fn verify_group_signature(
    headers: &HeaderMap,
    sender_bytes: &[u8],
    group_bytes: &[u8],
) -> Result<(), ApiError> {
    let signature_value = headers
        .get(GROUP_SIGNATURE_HEADER)
        .ok_or_else(|| ApiError::unauthorized("missing authorization header"))?;
    let signature_str = signature_value
        .to_str()
        .map_err(|_| ApiError::unauthorized("invalid authorization header"))?;
    let mut parts = signature_str.split_whitespace();
    let scheme = parts
        .next()
        .ok_or_else(|| ApiError::unauthorized("invalid authorization header"))?;
    let signature_hex = parts
        .next()
        .ok_or_else(|| ApiError::unauthorized("invalid authorization header"))?;
    if scheme != "Signature" || parts.next().is_some() {
        return Err(ApiError::unauthorized("invalid authorization header"));
    }
    let signature_bytes = parse_fixed_hex(signature_hex, 65)
        .map_err(|_| ApiError::unauthorized("invalid signature"))?;
    let signature = Signature::from_raw(&signature_bytes)
        .map_err(|_| ApiError::unauthorized("invalid signature"))?;
    let group_hash = keccak256(group_bytes);
    let recovered = signature
        .recover_address_from_prehash(&group_hash)
        .map_err(|_| ApiError::unauthorized("invalid signature"))?;
    let sender_addr =
        parse_address(sender_bytes).map_err(|err| ApiError::bad_request(err.to_string()))?;
    if recovered != sender_addr {
        return Err(ApiError::unauthorized("signature does not match sender"));
    }
    Ok(())
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(bytes))
}

fn u256_bytes_to_hex(bytes: &[u8]) -> String {
    if is_random_nonce_key(bytes) {
        return "random".to_string();
    }
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

fn datetime_from_ts(ts: u64) -> Result<DateTime<Utc>, ApiError> {
    let ts = i64::try_from(ts).map_err(|_| ApiError::bad_request("timestamp out of range"))?;
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
    nonce_key_bytes: &[u8],
) -> anyhow::Result<Option<u64>> {
    if is_random_nonce_key(nonce_key_bytes) {
        return Ok(None);
    }

    let nonce_key =
        u256_from_bytes(nonce_key_bytes).map_err(|err| anyhow::anyhow!(err.message))?;
    if nonce_key.is_zero() {
        let provider = chain
            .http
            .first()
            .ok_or_else(|| anyhow::anyhow!("missing provider"))?;
        let nonce = provider.get_transaction_count(sender).await?;
        return Ok(Some(nonce));
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
    Ok(Some(output))
}

fn is_random_nonce_key(bytes: &[u8]) -> bool {
    let mut offset = 0;
    while offset < bytes.len() && bytes[offset] == 0 {
        offset += 1;
    }
    bytes.get(offset..) == Some(b"random")
}

fn group_id_from_nonce_key(nonce_key_bytes: &[u8]) -> Vec<u8> {
    let hash = keccak256(nonce_key_bytes);
    let mut group_id = vec![0u8; 16];
    group_id.copy_from_slice(&hash[..16]);
    group_id
}

fn nonce_precompile_address() -> alloy::primitives::Address {
    alloy::primitives::Address::from_slice(
        &hex::decode("4e4f4e4345000000000000000000000000000000").expect("valid precompile"),
    )
}

#[cfg(test)]
mod tests {
    use super::{parse_fixed_hex, u256_bytes_to_hex, u256_from_bytes, validate_nonce_valid_before_order};
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

    #[test]
    fn validate_nonce_valid_before_order_accepts_monotonic() {
        let ok = validate_nonce_valid_before_order(&[
            (1, Some(10)),
            (2, Some(10)),
            (3, Some(12)),
        ]);
        assert!(ok.is_ok());
    }

    #[test]
    fn validate_nonce_valid_before_order_rejects_decreasing() {
        let err = validate_nonce_valid_before_order(&[(1, Some(10)), (2, Some(9))])
            .expect_err("expected error");
        assert!(err.message.contains("valid_before order"));
    }
}
