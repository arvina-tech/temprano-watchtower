CREATE TABLE IF NOT EXISTS txs (
    id BIGSERIAL PRIMARY KEY,
    chain_id BIGINT NOT NULL,
    tx_hash BYTEA NOT NULL,
    raw_tx BYTEA,
    sender BYTEA NOT NULL,
    fee_payer BYTEA,
    nonce_key BYTEA NOT NULL,
    nonce BIGINT NOT NULL,
    valid_after BIGINT,
    valid_before BIGINT,
    eligible_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ,
    status TEXT NOT NULL,
    group_id BYTEA,
    group_aux BYTEA,
    group_version SMALLINT,
    group_flags SMALLINT DEFAULT 0,
    next_action_at TIMESTAMPTZ,
    lease_owner TEXT,
    lease_until TIMESTAMPTZ,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    last_broadcast_at TIMESTAMPTZ,
    receipt JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS txs_chain_hash_idx ON txs (chain_id, tx_hash);
CREATE INDEX IF NOT EXISTS txs_sender_group_idx ON txs (sender, group_id);
CREATE INDEX IF NOT EXISTS txs_status_next_idx ON txs (status, next_action_at);
CREATE INDEX IF NOT EXISTS txs_chain_status_idx ON txs (chain_id, status);
