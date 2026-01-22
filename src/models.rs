use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use sqlx_pg_uint::PgU64;

#[derive(Debug, Clone, FromRow)]
pub struct TxRecord {
    pub id: i64,
    pub chain_id: PgU64,
    pub tx_hash: Vec<u8>,
    pub raw_tx: Option<Vec<u8>>,
    pub sender: Vec<u8>,
    pub fee_payer: Option<Vec<u8>>,
    pub nonce_key: Vec<u8>,
    pub nonce: PgU64,
    pub valid_after: Option<PgU64>,
    pub valid_before: Option<PgU64>,
    pub eligible_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub status: String,
    pub group_id: Option<Vec<u8>>,
    pub next_action_at: Option<DateTime<Utc>>,
    #[allow(dead_code)]
    pub lease_owner: Option<String>,
    #[allow(dead_code)]
    pub lease_until: Option<DateTime<Utc>>,
    pub attempts: i32,
    pub last_error: Option<String>,
    pub last_broadcast_at: Option<DateTime<Utc>>,
    pub receipt: Option<serde_json::Value>,
    #[allow(dead_code)]
    pub created_at: DateTime<Utc>,
    #[allow(dead_code)]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewTx {
    pub chain_id: PgU64,
    pub tx_hash: Vec<u8>,
    pub raw_tx: Vec<u8>,
    pub sender: Vec<u8>,
    pub fee_payer: Option<Vec<u8>>,
    pub nonce_key: Vec<u8>,
    pub nonce: PgU64,
    pub valid_after: Option<PgU64>,
    pub valid_before: Option<PgU64>,
    pub eligible_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub status: String,
    pub group_id: Option<Vec<u8>>,
    pub next_action_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxStatus {
    Queued,
    Broadcasting,
    RetryScheduled,
    Executed,
    Expired,
    Invalid,
    StaleByNonce,
    CanceledLocally,
}

impl TxStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TxStatus::Queued => "queued",
            TxStatus::Broadcasting => "broadcasting",
            TxStatus::RetryScheduled => "retry_scheduled",
            TxStatus::Executed => "executed",
            TxStatus::Expired => "expired",
            TxStatus::Invalid => "invalid",
            TxStatus::StaleByNonce => "stale_by_nonce",
            TxStatus::CanceledLocally => "canceled_locally",
        }
    }
}

impl std::fmt::Display for TxStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<&str> for TxStatus {
    type Error = ();

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "queued" => Ok(TxStatus::Queued),
            "broadcasting" => Ok(TxStatus::Broadcasting),
            "retry_scheduled" => Ok(TxStatus::RetryScheduled),
            "executed" => Ok(TxStatus::Executed),
            "expired" => Ok(TxStatus::Expired),
            "invalid" => Ok(TxStatus::Invalid),
            "stale_by_nonce" => Ok(TxStatus::StaleByNonce),
            "canceled_locally" => Ok(TxStatus::CanceledLocally),
            _ => Err(()),
        }
    }
}
