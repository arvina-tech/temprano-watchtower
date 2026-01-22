use std::collections::BTreeMap;
use std::time::Duration;

use alloy::network::TransactionBuilder;
use alloy::primitives::B256;
use alloy::providers::Provider;
use chrono::Utc;
use tokio_stream::StreamExt;
use tracing::{info, warn};

use crate::db;
use crate::models::TxRecord;
use crate::rpc::ChainRpc;
use crate::state::AppState;

pub fn start(state: AppState) {
    for chain_id in state.rpcs.chain_ids() {
        let state = state.clone();
        tokio::spawn(async move {
            run_chain_watcher(state, chain_id).await;
        });
    }
}

async fn run_chain_watcher(state: AppState, chain_id: u64) {
    let chain = match state.rpcs.chain(chain_id) {
        Some(chain) => chain.clone(),
        None => {
            warn!(%chain_id, "missing rpc chain for watcher");
            return;
        }
    };

    if state.config.watcher.use_websocket
        && let Some(ws) = chain.ws.clone()
    {
        match watch_ws(&state, chain_id, ws).await {
            Ok(()) => return,
            Err(err) => {
                warn!(%chain_id, error = %err, "ws watcher failed, falling back to polling");
            }
        }
    }

    watch_poll(&state, chain_id, &chain).await;
}

async fn watch_ws(
    state: &AppState,
    chain_id: u64,
    ws: alloy::providers::DynProvider<tempo_alloy::TempoNetwork>,
) -> anyhow::Result<()> {
    info!(%chain_id, "starting websocket watcher");
    let sub = ws.subscribe_blocks().await?;
    let mut stream = sub.into_stream();

    while let Some(_header) = stream.next().await {
        if let Err(err) = process_tick(state, chain_id).await {
            warn!(%chain_id, error = %err, "watcher tick failed");
        }
    }

    Err(anyhow::anyhow!("websocket stream ended"))
}

async fn watch_poll(state: &AppState, chain_id: u64, chain: &ChainRpc) {
    info!(%chain_id, "starting polling watcher");
    let mut interval =
        tokio::time::interval(Duration::from_millis(state.config.watcher.poll_interval_ms));

    loop {
        interval.tick().await;
        if let Err(err) = process_tick_with_chain(state, chain_id, chain).await {
            warn!(%chain_id, error = %err, "polling watcher tick failed");
        }
    }
}

async fn process_tick(state: &AppState, chain_id: u64) -> anyhow::Result<()> {
    let chain = state
        .rpcs
        .chain(chain_id)
        .ok_or_else(|| anyhow::anyhow!("missing rpc chain"))?;
    process_tick_with_chain(state, chain_id, chain).await
}

async fn process_tick_with_chain(
    state: &AppState,
    chain_id: u64,
    chain: &ChainRpc,
) -> anyhow::Result<()> {
    let records = db::list_active_txs(&state.db, chain_id).await?;
    if records.is_empty() {
        return Ok(());
    }

    let now = Utc::now();
    let mut pending = Vec::new();

    for record in records {
        if let Some(expires_at) = record.expires_at
            && expires_at <= now
        {
            db::mark_expired(&state.db, record.id).await?;
            continue;
        }

        if let Some(receipt) = fetch_receipt(chain, &record).await? {
            let receipt_json = serde_json::to_value(receipt)?;
            db::mark_executed(&state.db, record.id, receipt_json).await?;
            continue;
        }

        pending.push(record);
    }

    if pending.is_empty() {
        return Ok(());
    }

    let mut grouped: BTreeMap<(Vec<u8>, Vec<u8>), Vec<TxRecord>> = BTreeMap::new();
    for record in pending {
        grouped
            .entry((record.sender.clone(), record.nonce_key.clone()))
            .or_default()
            .push(record);
    }

    for ((sender, nonce_key_bytes), records) in grouped {
        let sender_addr = parse_address(&sender)?;
        let current_nonce = fetch_current_nonce(chain, sender_addr, &nonce_key_bytes).await?;

        if let Some(current_nonce) = current_nonce {
            for record in records {
                if current_nonce > record.nonce.to_uint() {
                    db::mark_stale_by_nonce(&state.db, record.id).await?;
                }
            }
        }
    }

    Ok(())
}

async fn fetch_receipt(
    chain: &ChainRpc,
    record: &TxRecord,
) -> anyhow::Result<Option<tempo_alloy::rpc::TempoTransactionReceipt>> {
    let provider = chain
        .http
        .first()
        .ok_or_else(|| anyhow::anyhow!("missing provider"))?;

    if record.tx_hash.len() != 32 {
        warn!(id = record.id, "invalid tx_hash length");
        return Ok(None);
    }

    let hash = B256::from_slice(&record.tx_hash);
    let receipt = provider.get_transaction_receipt(hash).await?;
    Ok(receipt)
}

async fn fetch_current_nonce(
    chain: &ChainRpc,
    sender: alloy::primitives::Address,
    nonce_key_bytes: &[u8],
) -> anyhow::Result<Option<u64>> {
    let nonce_key = u256_from_bytes(nonce_key_bytes)?;
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

fn parse_address(bytes: &[u8]) -> anyhow::Result<alloy::primitives::Address> {
    if bytes.len() != 20 {
        anyhow::bail!("invalid address length");
    }
    let mut data = [0u8; 20];
    data.copy_from_slice(bytes);
    Ok(alloy::primitives::Address::from(data))
}

fn u256_from_bytes(bytes: &[u8]) -> anyhow::Result<alloy::primitives::U256> {
    if bytes.len() > 32 {
        anyhow::bail!("nonce_key too large");
    }
    let mut buf = [0u8; 32];
    let offset = 32 - bytes.len();
    buf[offset..].copy_from_slice(bytes);
    Ok(alloy::primitives::U256::from_be_slice(&buf))
}

fn nonce_precompile_address() -> alloy::primitives::Address {
    alloy::primitives::Address::from_slice(
        &hex::decode("4e4f4e4345000000000000000000000000000000").expect("valid precompile"),
    )
}
