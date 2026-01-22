use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, QueryBuilder, Transaction};
use sqlx_pg_uint::PgU64;

use crate::models::{NewTx, TxRecord, TxStatus};

pub async fn connect(url: &str) -> Result<PgPool> {
    Ok(PgPool::connect(url).await?)
}

pub async fn migrate(pool: &PgPool) -> Result<()> {
    sqlx::migrate!().run(pool).await?;
    Ok(())
}

pub async fn insert_tx(
    tx: &mut Transaction<'_, Postgres>,
    new_tx: &NewTx,
) -> Result<(TxRecord, bool)> {
    let result = sqlx::query(
        r#"
        INSERT INTO txs (
            chain_id, tx_hash, raw_tx, sender, fee_payer, nonce_key, nonce,
            valid_after, valid_before, eligible_at, expires_at, status,
            group_id, next_action_at
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7,
            $8, $9, $10, $11, $12,
            $13, $14
        )
        ON CONFLICT (chain_id, tx_hash) DO NOTHING
        "#,
    )
    .bind(&new_tx.chain_id)
    .bind(&new_tx.tx_hash)
    .bind(&new_tx.raw_tx)
    .bind(&new_tx.sender)
    .bind(&new_tx.fee_payer)
    .bind(&new_tx.nonce_key)
    .bind(&new_tx.nonce)
    .bind(&new_tx.valid_after)
    .bind(&new_tx.valid_before)
    .bind(new_tx.eligible_at)
    .bind(new_tx.expires_at)
    .bind(&new_tx.status)
    .bind(&new_tx.group_id)
    .bind(new_tx.next_action_at)
    .execute(tx.as_mut())
    .await?;

    let already_known = result.rows_affected() == 0;
    let record =
        sqlx::query_as::<_, TxRecord>("SELECT * FROM txs WHERE chain_id = $1 AND tx_hash = $2")
            .bind(&new_tx.chain_id)
            .bind(&new_tx.tx_hash)
            .fetch_one(tx.as_mut())
            .await?;

    Ok((record, already_known))
}

pub async fn get_group_nonce_key(
    tx: &mut Transaction<'_, Postgres>,
    chain_id: u64,
    sender: &[u8],
    group_id: &[u8],
) -> Result<Option<Vec<u8>>> {
    let chain_id = PgU64::from(chain_id);
    let nonce_key = sqlx::query_scalar::<_, Vec<u8>>(
        r#"
        SELECT nonce_key
        FROM txs
        WHERE chain_id = $1 AND sender = $2 AND group_id = $3
        LIMIT 1
        "#,
    )
    .bind(chain_id)
    .bind(sender)
    .bind(group_id)
    .fetch_optional(tx.as_mut())
    .await?;

    Ok(nonce_key)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GroupNonceWindow {
    pub nonce: PgU64,
    pub valid_before: Option<PgU64>,
}

pub async fn get_group_nonce_windows(
    tx: &mut Transaction<'_, Postgres>,
    chain_id: u64,
    sender: &[u8],
    group_id: &[u8],
) -> Result<Vec<GroupNonceWindow>> {
    let chain_id = PgU64::from(chain_id);
    let rows = sqlx::query_as::<_, GroupNonceWindow>(
        r#"
        SELECT nonce, valid_before
        FROM txs
        WHERE chain_id = $1 AND sender = $2 AND group_id = $3
        "#,
    )
    .bind(chain_id)
    .bind(sender)
    .bind(group_id)
    .fetch_all(tx.as_mut())
    .await?;

    Ok(rows)
}

pub async fn get_tx_by_hash(
    pool: &PgPool,
    chain_id: Option<u64>,
    tx_hash: &[u8],
) -> Result<Option<TxRecord>> {
    let record = if let Some(chain_id) = chain_id {
        let chain_id = PgU64::from(chain_id);
        sqlx::query_as::<_, TxRecord>("SELECT * FROM txs WHERE chain_id = $1 AND tx_hash = $2")
            .bind(chain_id)
            .bind(tx_hash)
            .fetch_optional(pool)
            .await?
    } else {
        sqlx::query_as::<_, TxRecord>(
            "SELECT * FROM txs WHERE tx_hash = $1 ORDER BY created_at DESC LIMIT 1",
        )
        .bind(tx_hash)
        .fetch_optional(pool)
        .await?
    };

    Ok(record)
}

#[derive(Default, Debug, Clone)]
pub struct TxFilters {
    pub chain_id: Option<u64>,
    pub sender: Option<Vec<u8>>,
    pub group_id: Option<Vec<u8>>,
    pub statuses: Vec<TxStatus>,
    pub limit: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SenderGroupRecord {
    pub chain_id: PgU64,
    pub group_id: Vec<u8>,
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
}

pub async fn list_txs(pool: &PgPool, filters: TxFilters) -> Result<Vec<TxRecord>> {
    let mut qb = QueryBuilder::<Postgres>::new("SELECT * FROM txs WHERE 1=1");

    if let Some(chain_id) = filters.chain_id {
        let chain_id = PgU64::from(chain_id);
        qb.push(" AND chain_id = ").push_bind(chain_id);
    }
    if let Some(sender) = filters.sender {
        qb.push(" AND sender = ").push_bind(sender);
    }
    if let Some(group_id) = filters.group_id {
        qb.push(" AND group_id = ").push_bind(group_id);
    }
    if !filters.statuses.is_empty() {
        qb.push(" AND status IN (");
        let mut separated = qb.separated(", ");
        for status in &filters.statuses {
            separated.push_bind(status.as_str());
        }
        qb.push(")");
    }

    let limit = filters.limit.clamp(1, 500);
    qb.push(" ORDER BY created_at DESC LIMIT ").push_bind(limit);

    let records = qb.build_query_as::<TxRecord>().fetch_all(pool).await?;
    Ok(records)
}

pub async fn list_sender_groups(
    pool: &PgPool,
    sender: &[u8],
    chain_id: Option<u64>,
    limit: i64,
    active_only: bool,
) -> Result<Vec<SenderGroupRecord>> {
    let mut qb = QueryBuilder::<Postgres>::new(
        "SELECT \
        chain_id, \
        group_id, \
        MIN(eligible_at) AS start_at, \
        MAX(eligible_at) AS end_at \
        FROM txs WHERE sender = ",
    );
    qb.push_bind(sender);
    qb.push(" AND group_id IS NOT NULL");
    if let Some(chain_id) = chain_id {
        let chain_id = PgU64::from(chain_id);
        qb.push(" AND chain_id = ").push_bind(chain_id);
    }

    let limit = limit.clamp(1, 500);
    qb.push(" GROUP BY chain_id, group_id");
    if active_only {
        qb.push(" HAVING MAX(eligible_at) > NOW()");
    }
    qb.push(" ORDER BY chain_id, group_id LIMIT ")
        .push_bind(limit);

    let rows = qb.build_query_as::<SenderGroupRecord>().fetch_all(pool).await?;
    Ok(rows)
}

pub async fn list_active_txs(pool: &PgPool, chain_id: u64) -> Result<Vec<TxRecord>> {
    let chain_id = PgU64::from(chain_id);
    let rows = sqlx::query_as::<_, TxRecord>(
        r#"
        SELECT *
        FROM txs
        WHERE chain_id = $1
          AND status IN ($2, $3, $4)
        ORDER BY next_action_at ASC NULLS LAST, created_at ASC
        "#,
    )
    .bind(chain_id)
    .bind(TxStatus::Queued.as_str())
    .bind(TxStatus::Broadcasting.as_str())
    .bind(TxStatus::RetryScheduled.as_str())
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

pub async fn get_group_txs(
    pool: &PgPool,
    sender: &[u8],
    group_id: &[u8],
    chain_id: Option<u64>,
) -> Result<Vec<TxRecord>> {
    let mut qb = QueryBuilder::<Postgres>::new("SELECT * FROM txs WHERE sender = ");
    qb.push_bind(sender);
    qb.push(" AND group_id = ").push_bind(group_id);
    if let Some(chain_id) = chain_id {
        let chain_id = PgU64::from(chain_id);
        qb.push(" AND chain_id = ").push_bind(chain_id);
    }
    qb.push(" ORDER BY nonce ASC");

    let rows = qb.build_query_as::<TxRecord>().fetch_all(pool).await?;
    Ok(rows)
}

pub async fn cancel_group(pool: &PgPool, sender: &[u8], group_id: &[u8]) -> Result<Vec<TxRecord>> {
    let rows = sqlx::query_as::<_, TxRecord>(
        r#"
        UPDATE txs
        SET status = $1,
            raw_tx = NULL,
            next_action_at = NULL,
            lease_owner = NULL,
            lease_until = NULL,
            updated_at = NOW()
        WHERE sender = $2 AND group_id = $3
        RETURNING *
        "#,
    )
    .bind(TxStatus::CanceledLocally.as_str())
    .bind(sender)
    .bind(group_id)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

pub async fn lease_due_txs(
    pool: &PgPool,
    chain_id: u64,
    now: DateTime<Utc>,
    lease_owner: &str,
    lease_until: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<TxRecord>> {
    let chain_id = PgU64::from(chain_id);
    let rows = sqlx::query_as::<_, TxRecord>(
        r#"
        WITH due AS (
            SELECT id
            FROM txs
            WHERE chain_id = $1
              AND status IN ($2, $3, $4)
              AND next_action_at <= $5
              AND (lease_until IS NULL OR lease_until < $5)
            ORDER BY next_action_at ASC
            LIMIT $6
            FOR UPDATE SKIP LOCKED
        )
        UPDATE txs
        SET status = $7,
            lease_owner = $8,
            lease_until = $9,
            updated_at = NOW()
        WHERE id IN (SELECT id FROM due)
        RETURNING *
        "#,
    )
    .bind(chain_id)
    .bind(TxStatus::Queued.as_str())
    .bind(TxStatus::RetryScheduled.as_str())
    .bind(TxStatus::Broadcasting.as_str())
    .bind(now)
    .bind(limit)
    .bind(TxStatus::Broadcasting.as_str())
    .bind(lease_owner)
    .bind(lease_until)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

pub async fn lease_tx_by_hash(
    pool: &PgPool,
    chain_id: u64,
    tx_hash: &[u8],
    now: DateTime<Utc>,
    lease_owner: &str,
    lease_until: DateTime<Utc>,
) -> Result<Option<TxRecord>> {
    let chain_id = PgU64::from(chain_id);
    let row = sqlx::query_as::<_, TxRecord>(
        r#"
        UPDATE txs
        SET status = $1,
            lease_owner = $2,
            lease_until = $3,
            updated_at = NOW()
        WHERE chain_id = $4
          AND tx_hash = $5
          AND status IN ($6, $7, $8)
          AND next_action_at <= $9
          AND (lease_until IS NULL OR lease_until < $9)
        RETURNING *
        "#,
    )
    .bind(TxStatus::Broadcasting.as_str())
    .bind(lease_owner)
    .bind(lease_until)
    .bind(chain_id)
    .bind(tx_hash)
    .bind(TxStatus::Queued.as_str())
    .bind(TxStatus::RetryScheduled.as_str())
    .bind(TxStatus::Broadcasting.as_str())
    .bind(now)
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

pub async fn reschedule_tx(
    pool: &PgPool,
    id: i64,
    status: &str,
    next_action_at: DateTime<Utc>,
    attempts: i32,
    last_error: Option<&str>,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE txs
        SET status = $1,
            next_action_at = $2,
            attempts = $3,
            last_error = $4,
            last_broadcast_at = NOW(),
            lease_owner = NULL,
            lease_until = NULL,
            updated_at = NOW()
        WHERE id = $5
        "#,
    )
    .bind(status)
    .bind(next_action_at)
    .bind(attempts)
    .bind(last_error)
    .bind(id)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn reschedule_tx_if_leased(
    pool: &PgPool,
    id: i64,
    lease_owner: &str,
    status: &str,
    next_action_at: DateTime<Utc>,
    attempts: i32,
    last_error: Option<&str>,
) -> Result<bool> {
    let result = sqlx::query(
        r#"
        UPDATE txs
        SET status = $1,
            next_action_at = $2,
            attempts = $3,
            last_error = $4,
            last_broadcast_at = NOW(),
            lease_owner = NULL,
            lease_until = NULL,
            updated_at = NOW()
        WHERE id = $5
          AND status = $6
          AND lease_owner = $7
        "#,
    )
    .bind(status)
    .bind(next_action_at)
    .bind(attempts)
    .bind(last_error)
    .bind(id)
    .bind(TxStatus::Broadcasting.as_str())
    .bind(lease_owner)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn mark_broadcasted_if_leased(
    pool: &PgPool,
    id: i64,
    lease_owner: &str,
    attempts: i32,
    last_error: Option<&str>,
) -> Result<bool> {
    let result = sqlx::query(
        r#"
        UPDATE txs
        SET status = $1,
            next_action_at = NULL,
            attempts = $2,
            last_error = $3,
            last_broadcast_at = NOW(),
            lease_owner = NULL,
            lease_until = NULL,
            updated_at = NOW()
        WHERE id = $4
          AND status = $5
          AND lease_owner = $6
        "#,
    )
    .bind(TxStatus::Broadcasting.as_str())
    .bind(attempts)
    .bind(last_error)
    .bind(id)
    .bind(TxStatus::Broadcasting.as_str())
    .bind(lease_owner)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn mark_terminal(
    pool: &PgPool,
    id: i64,
    status: &str,
    last_error: Option<&str>,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE txs
        SET status = $1,
            last_error = $2,
            next_action_at = NULL,
            lease_owner = NULL,
            lease_until = NULL,
            updated_at = NOW()
        WHERE id = $3
        "#,
    )
    .bind(status)
    .bind(last_error)
    .bind(id)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn mark_terminal_if_leased(
    pool: &PgPool,
    id: i64,
    lease_owner: &str,
    status: &str,
    last_error: Option<&str>,
) -> Result<bool> {
    let result = sqlx::query(
        r#"
        UPDATE txs
        SET status = $1,
            last_error = $2,
            next_action_at = NULL,
            lease_owner = NULL,
            lease_until = NULL,
            updated_at = NOW()
        WHERE id = $3
          AND status = $4
          AND lease_owner = $5
        "#,
    )
    .bind(status)
    .bind(last_error)
    .bind(id)
    .bind(TxStatus::Broadcasting.as_str())
    .bind(lease_owner)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn mark_executed(pool: &PgPool, id: i64, receipt: serde_json::Value) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE txs
        SET status = $1,
            receipt = $2,
            next_action_at = NULL,
            lease_owner = NULL,
            lease_until = NULL,
            updated_at = NOW()
        WHERE id = $3
        "#,
    )
    .bind(TxStatus::Executed.as_str())
    .bind(receipt)
    .bind(id)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn mark_expired(pool: &PgPool, id: i64) -> Result<()> {
    mark_terminal(pool, id, TxStatus::Expired.as_str(), None).await
}

pub async fn mark_invalid(pool: &PgPool, id: i64, reason: &str) -> Result<()> {
    mark_terminal(pool, id, TxStatus::Invalid.as_str(), Some(reason)).await
}

pub async fn mark_stale_by_nonce(pool: &PgPool, id: i64) -> Result<()> {
    mark_terminal(pool, id, TxStatus::StaleByNonce.as_str(), None).await
}

pub async fn recover_stuck_broadcasts(pool: &PgPool) -> Result<Vec<TxRecord>> {
    let rows = sqlx::query_as::<_, TxRecord>(
        r#"
        UPDATE txs
        SET status = $1,
            next_action_at = NOW(),
            lease_owner = NULL,
            lease_until = NULL,
            updated_at = NOW()
        WHERE status = $2
          AND next_action_at IS NULL
        RETURNING *
        "#,
    )
    .bind(TxStatus::RetryScheduled.as_str())
    .bind(TxStatus::Broadcasting.as_str())
    .fetch_all(pool)
    .await?;

    Ok(rows)
}
