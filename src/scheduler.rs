use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use tokio::sync::Semaphore;
use tracing::{error, warn};
use uuid::Uuid;

use crate::broadcaster::{self, BroadcastOutcome};
use crate::db;
use crate::models::{TxRecord, TxStatus};
use crate::state::AppState;

pub fn start(state: AppState) {
    for chain_id in state.rpcs.chain_ids() {
        let state = state.clone();
        tokio::spawn(async move {
            run_chain_scheduler(state, chain_id).await;
        });
    }
}

async fn run_chain_scheduler(state: AppState, chain_id: u64) {
    let config = state.config.clone();
    let mut interval =
        tokio::time::interval(Duration::from_millis(config.scheduler.poll_interval_ms));
    let lease_owner = format!("scheduler:{}:{}", chain_id, Uuid::new_v4());
    let semaphore = Arc::new(Semaphore::new(config.scheduler.max_concurrency));

    loop {
        interval.tick().await;

        let available = semaphore.available_permits();
        if available == 0 {
            continue;
        }

        let now = Utc::now();
        let lease_until = now + chrono::Duration::seconds(config.scheduler.lease_ttl_seconds);

        let mut redis = state.redis.clone();
        let mut leased = Vec::new();

        match fetch_due_from_redis(&mut redis, chain_id, now, available).await {
            Ok(due) => {
                for hash in due {
                    if let Ok(tx_hash) = parse_hex_hash(&hash) {
                        match db::lease_tx_by_hash(
                            &state.db,
                            chain_id as i64,
                            &tx_hash,
                            now,
                            &lease_owner,
                            lease_until,
                        )
                        .await
                        {
                            Ok(Some(record)) => leased.push(record),
                            Ok(None) => {}
                            Err(err) => {
                                warn!(error = %err, "failed to lease tx");
                            }
                        }
                    }

                    let ready_key = ready_key(chain_id);
                    let retry_key = retry_key(chain_id);
                    let _: () = redis.zrem(ready_key, &hash).await.unwrap_or(());
                    let _: () = redis.zrem(retry_key, &hash).await.unwrap_or(());
                }
            }
            Err(err) => {
                warn!(error = %err, "failed to fetch due txs from redis");
            }
        }

        let remaining = available.saturating_sub(leased.len());
        if remaining > 0 {
            match db::lease_due_txs(
                &state.db,
                chain_id as i64,
                now,
                &lease_owner,
                lease_until,
                remaining as i64,
            )
            .await
            {
                Ok(mut records) => leased.append(&mut records),
                Err(err) => warn!(error = %err, "failed to lease due txs from db"),
            }
        }

        for record in leased {
            let state = state.clone();
            let semaphore = semaphore.clone();
            let permit = match semaphore.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(err) => {
                    warn!(error = %err, "failed to acquire scheduler permit");
                    continue;
                }
            };

            tokio::spawn(async move {
                let _permit = permit;
                if let Err(err) = handle_broadcast(state, chain_id, record).await {
                    error!(error = %err, "broadcast attempt failed");
                }
            });
        }
    }
}

async fn handle_broadcast(state: AppState, chain_id: u64, record: TxRecord) -> anyhow::Result<()> {
    let now = Utc::now();
    if let Some(expires_at) = record.expires_at
        && expires_at <= now
    {
        db::mark_expired(&state.db, record.id).await?;
        return Ok(());
    }

    let raw_tx = match record.raw_tx.as_ref() {
        Some(raw) => raw,
        None => {
            db::mark_invalid(&state.db, record.id, "missing raw_tx").await?;
            return Ok(());
        }
    };

    let chain = state
        .rpcs
        .chain(chain_id)
        .ok_or_else(|| anyhow::anyhow!("missing rpc chain"))?;

    let outcome = broadcaster::broadcast_raw_tx(
        chain,
        raw_tx,
        state.config.broadcaster.fanout,
        Duration::from_millis(state.config.broadcaster.timeout_ms),
        record.attempts,
    )
    .await;

    let attempts = record.attempts.saturating_add(1);

    match outcome {
        BroadcastOutcome::Accepted { error } => {
            let next_action_at =
                schedule_next_attempt(now, record.expires_at, attempts as u64, &state);
            db::reschedule_tx(
                &state.db,
                record.id,
                TxStatus::RetryScheduled.as_str(),
                next_action_at,
                attempts,
                error.as_deref(),
            )
            .await?;
            update_retry_schedule(&state, chain_id, &record.tx_hash, next_action_at).await?;
        }
        BroadcastOutcome::Retry { error } => {
            let next_action_at =
                schedule_next_attempt(now, record.expires_at, attempts as u64, &state);
            db::reschedule_tx(
                &state.db,
                record.id,
                TxStatus::RetryScheduled.as_str(),
                next_action_at,
                attempts,
                Some(&error),
            )
            .await?;
            update_retry_schedule(&state, chain_id, &record.tx_hash, next_action_at).await?;
        }
        BroadcastOutcome::Invalid { error } => {
            db::mark_invalid(&state.db, record.id, &error).await?;
        }
    }

    Ok(())
}

fn schedule_next_attempt(
    now: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    attempts: u64,
    state: &AppState,
) -> DateTime<Utc> {
    let delay_ms = retry_backoff_ms(
        attempts,
        state.config.scheduler.retry_min_ms,
        state.config.scheduler.retry_max_ms,
    );
    let mut next_action_at = now + chrono::Duration::milliseconds(delay_ms as i64);
    if let Some(expires_at) = expires_at
        && next_action_at > expires_at
    {
        next_action_at = expires_at;
    }
    next_action_at
}

fn retry_backoff_ms(attempts: u64, min_ms: u64, max_ms: u64) -> u64 {
    let shift = attempts.saturating_sub(1).min(10);
    let delay = min_ms.saturating_mul(1u64 << shift);
    delay.clamp(min_ms, max_ms)
}

async fn update_retry_schedule(
    state: &AppState,
    chain_id: u64,
    tx_hash: &[u8],
    next_action_at: DateTime<Utc>,
) -> anyhow::Result<()> {
    let mut redis = state.redis.clone();
    let tx_hash = bytes_to_hex(tx_hash);
    let ready_key = ready_key(chain_id);
    let retry_key = retry_key(chain_id);

    let _: () = redis.zrem(ready_key, &tx_hash).await.unwrap_or(());
    let _: () = redis.zrem(retry_key.clone(), &tx_hash).await.unwrap_or(());
    let _: () = redis
        .zadd(retry_key, tx_hash, next_action_at.timestamp())
        .await?;
    Ok(())
}

async fn fetch_due_from_redis(
    redis: &mut redis::aio::ConnectionManager,
    chain_id: u64,
    now: DateTime<Utc>,
    limit: usize,
) -> redis::RedisResult<Vec<String>> {
    let ready_key = ready_key(chain_id);
    let retry_key = retry_key(chain_id);
    let max_score = now.timestamp();
    let mut out: Vec<String> = Vec::new();

    if limit == 0 {
        return Ok(out);
    }

    let mut ready: Vec<String> = redis
        .zrangebyscore_limit(ready_key, 0, max_score, 0, limit as isize)
        .await?;
    out.append(&mut ready);

    if out.len() < limit {
        let mut retry: Vec<String> = redis
            .zrangebyscore_limit(retry_key, 0, max_score, 0, (limit - out.len()) as isize)
            .await?;
        out.append(&mut retry);
    }

    Ok(out)
}

fn parse_hex_hash(value: &str) -> Result<Vec<u8>, hex::FromHexError> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    hex::decode(value)
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(bytes))
}

fn ready_key(chain_id: u64) -> String {
    format!("watchtower:ready:{chain_id}")
}

fn retry_key(chain_id: u64) -> String {
    format!("watchtower:retry:{chain_id}")
}

#[cfg(test)]
mod tests {
    use super::retry_backoff_ms;

    #[test]
    fn retry_backoff_respects_bounds() {
        assert_eq!(retry_backoff_ms(0, 250, 5000), 250);
        assert_eq!(retry_backoff_ms(1, 250, 5000), 250);
        assert_eq!(retry_backoff_ms(2, 250, 5000), 500);
        assert_eq!(retry_backoff_ms(3, 250, 5000), 1000);
        assert_eq!(retry_backoff_ms(10, 250, 5000), 5000);
        assert_eq!(retry_backoff_ms(20, 250, 5000), 5000);
    }
}
