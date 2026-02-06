# System Design

## Scheduler

### Behavior

- Due transactions are pulled by `next_action_at`.
- Database-backed leasing is used for multi-replica safety.
- Redis ZSET is used as an accelerator only.
- Guaranteed retry continues until expiry.

### Retry Strategy

- Near eligibility: 250–500ms attempts.
- Backoff up to 5s max.
- Endpoint rotation.
- Health scoring per RPC endpoint.

## Broadcaster

- Fan-out to multiple Tempo RPC endpoints.
- Tracks:
  - accepted
  - rejected (with reason)
  - timeout
- Stops only on terminal conditions.

## Chain Watcher

Tracks:

- `(sender, nonce_key) → current_nonce`
- receipts for known transactions

Transitions:

- mined → terminal
- nonce advanced past tx → `stale_by_nonce`

Notes:

- The watcher uses websocket subscriptions when available and falls back to polling.

## Storage Model

### `txs` Table

Fields:

- `chain_id`
- `tx_hash` (unique)
- `raw_tx`
- `sender`
- `fee_payer`
- `nonce_key`
- `nonce`
- `valid_after`
- `valid_before`
- `eligible_at`
- `expires_at`
- `status`
- `group_id` (16 bytes nullable)
- `next_action_at`
- leasing fields
- timestamps

Indexes:

- `(chain_id, tx_hash)` unique
- `(sender, group_id)`
- `(status, next_action_at)`

## Redis Acceleration

Keys:

- `watchtower:ready:{chain}` → ZSET(tx_hash, eligibleAt)
- `watchtower:retry:{chain}` → ZSET(tx_hash, nextRetryAt)
- Optional inflight/lease keys

Redis is rebuildable from the database.

Redis is used as a scheduling accelerator; the database remains the source of truth.

## Observability

Metrics:

- ingest rate
- queue depth
- retry counts
- success/failure rates
- time-to-mined

Tracing:

- full transaction lifecycle

Logs:

- structured per txHash

## Security

- request size limits
- strict decoding
- rate limiting
- no signing
- no private key handling
