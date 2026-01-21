CREATE TABLE IF NOT EXISTS txs (
    id BIGSERIAL PRIMARY KEY,
    chain_id NUMERIC(20, 0) NOT NULL CONSTRAINT chk_chain_id_range CHECK (chain_id >= 0 AND chain_id <= 18446744073709551615),
    tx_hash BYTEA NOT NULL,
    raw_tx BYTEA,
    sender BYTEA NOT NULL,
    fee_payer BYTEA,
    nonce_key BYTEA NOT NULL,
    nonce NUMERIC(20, 0) NOT NULL CONSTRAINT chk_nonce_range CHECK (nonce >= 0 AND nonce <= 18446744073709551615),
    valid_after NUMERIC(20, 0) CONSTRAINT chk_valid_after_range CHECK (valid_after IS NULL OR (valid_after >= 0 AND valid_after <= 18446744073709551615)),
    valid_before NUMERIC(20, 0) CONSTRAINT chk_valid_before_range CHECK (valid_before IS NULL OR (valid_before >= 0 AND valid_before <= 18446744073709551615)),
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
