# Tempo Watchtower — Full Software Specification

## 1) Purpose

A Rust monolith service that:

* Accepts **raw signed Tempo transactions** (same format as `eth_sendRawTransaction`, e.g. output of `viem.signTransaction`)
* Stores them durably
* Broadcasts them to Tempo nodes **as soon as they are valid**
* Keeps retrying **throughout their validity window** (guaranteed delivery mode)
* Supports **grouping of transactions via the nonce key**
* Allows users to cancel an entire group by invalidating the relevant nonces

---

## 2) Core Guarantees

### 2.1 Delivery Semantics

Default mode is **guaranteed delivery**.

For every accepted transaction:

* Broadcast starts at `valid_after` (or immediately if unset)
* Retry continues until:

  * transaction is mined
  * validity window expires
  * transaction is provably invalid
  * user cancels locally


---

## 3) Accepted Transaction Format

The service accepts exactly the same payload as Ethereum raw transaction submission:

```
rawTx = "0x" + hex-encoded signed Tempo tx bytes
```

This is the output of:

* `viem.signTransaction()`
* Any Tempo wallet signer

The server performs:

* decoding
* signature validation
* field extraction
* scheduling

---

## 4) Grouping Design

Users may run **multiple concurrent groups** (e.g. multiple subscriptions).

Group identity:

```
GroupKey = (sender_address, group_id)
```

Each sender can have arbitrarily many groups.

Grouping is based solely on the transaction nonce key. Transaction input data and memo contents are ignored.

---

## 5) Nonce Key Grouping

For every accepted transaction, the group fields are derived from the nonce key:

* `group_id` = first 16 bytes of `keccak256(nonce_key_bytes)`

All transactions that share a nonce key are placed in the same group for a given sender.

---

## 6) HTTP API (Normal REST)

Base path: `/v1`

---

### 6.0 Health

`GET /health`

Returns service status, version, and runtime/config details. If Redis or Postgres are unavailable, the endpoint returns HTTP 503 and `status: "degraded"`.

Example response:

```json
{
  "status": "ok",
  "service": "tempo-watchtower",
  "version": "0.2.6",
  "build": {
    "gitSha": "abc123",
    "buildTimestamp": "2026-02-03T12:34:56Z"
  },
  "now": 1738612345,
  "startedAt": 1738610000,
  "uptimeSeconds": 2345,
  "chains": [42431],
  "rpcEndpoints": 2,
  "scheduler": {
    "pollIntervalMs": 1000,
    "leaseTtlSeconds": 30,
    "maxConcurrency": 10,
    "retryMinMs": 100,
    "retryMaxMs": 1000,
    "expirySoonWindowSeconds": 3600,
    "expirySoonRetryMaxMs": 5000
  },
  "watcher": {
    "pollIntervalMs": 1000,
    "useWebsocket": false
  },
  "broadcaster": {
    "fanout": 2,
    "timeoutMs": 500
  },
  "api": {
    "maxBodyBytes": 1048576
  },
  "dependencies": {
    "database": { "ok": true },
    "redis": { "ok": true }
  }
}
```

`build.gitSha` and `build.buildTimestamp` are omitted when not provided at build time.

### 6.1 Submit Transactions (Batch)

`POST /v1/transactions`

#### Request

```json
{
  "chainId": 123,
  "transactions": [
    "0x...",
    "0x...",
    "0x..."
  ]
}
```

#### Response

```json
{
  "results": [
    {
      "ok": true,
      "txHash": "0x...",
      "sender": "0x...",
      "nonceKey": "0x1",
      "nonce": 100,
      "groupId": "0x00112233445566778899aabbccddeeff",
      "eligibleAt": 1730000000,
      "expiresAt": 1730003600,
      "status": "queued",
      "alreadyKnown": false
    }
  ]
}
```

#### Behavior

* Hash-based idempotency:

  * `(chainId, txHash)` is unique
  * resubmission returns existing record
* Static validation performed at ingest:

  * decoding
  * signature verification
  * not already expired
* Dynamic validity (nonce, balance) handled by scheduler

---

### 6.2 Get Transaction

`GET /v1/transactions/{txHash}`

Returns:

* status
* scheduling info
* last broadcast error
* receipt if mined
* attempts summary

---

### 6.3 Cancel Transaction (stale by nonce)

`DELETE /v1/transactions/{txHash}`

Optional query: `chainId`.

Behavior:

* Fetches the current nonce for the transaction's nonce key.
* If the current nonce is higher than the transaction nonce, marks the transaction as `stale_by_nonce`.
* Otherwise returns `400`.

---

### 6.4 List Transactions

`GET /v1/transactions?sender=0x...&groupId=0x...&status=queued&status=retry_scheduled`

Notes:
* `status` may be provided multiple times to match any of the values.
* `ungrouped=true` returns only transactions without a `groupId` (nonce keys that do not match the explicit grouping format).
* `ungrouped=true` cannot be combined with `groupId`.

---

### 6.5 List Groups

`GET /v1/senders/{sender}/groups?chainId=42431&limit=100&active=true`

```json
[
  {
    "chainId": 42431,
    "groupId": "0x00112233445566778899aabbccddeeff",
    "startAt": 1730000000,
    "endAt": 1730003600,
    "nextPaymentAt": 1730000000
  }
]
```

Notes:
* `endAt` is the largest `eligibleAt` for the group.
* `nextPaymentAt` is the earliest `eligibleAt` for non-terminal transactions in the group.
* `active=true` returns groups whose `endAt` is in the future.

---

### 6.6 Get Group + Cancel Plan

`GET /v1/senders/{sender}/groups/{groupId}`

```json
{
  "sender": "0x...",
  "groupId": "0x00112233445566778899aabbccddeeff",
  "members": [
    { "txHash":"0x..", "nonceKey":"0x1", "nonce":100, "status":"queued" },
    { "txHash":"0x..", "nonceKey":"0x1", "nonce":101, "status":"broadcasting" }
  ],
  "cancelPlan": {
    "nonceKey": "0x1",
    "nonces": [100,101],
    "alreadyInvalidated": false
  }
}
```

Meaning:

* To cancel the group, user must invalidate all nonces for the group's nonce_key.
* All group members must share the same nonce_key.
* Transactions are grouped only when `nonceKey` matches the explicit grouping format

---

### 6.6 Nonce Key Format (Explicit Grouping)

Nonce keys are 32-byte values. Transactions are grouped only when the nonce key matches this format.

#### Binary layout (big-endian)

```
0..3   magic     = 0x4E4B4731   // "NKG1"
4      version   = 0x01
5      kind      = enum (purpose)
6..7   flags     = u16
8..15  scope_id  = u64
16..19 group_id  = u32
20..31 memo      = 12 bytes
```

#### Flags encoding (u16)

```
bits 0..1   scope_id encoding
bits 2..3   group_id encoding
bits 4..5   memo encoding
bits 6..15  reserved (must be 0)
```

Encoding values (same for scope_id, group_id, memo):

```
00 = numeric/raw
01 = ASCII
10 = reserved (undefined)
11 = reserved (undefined)
```

#### Display / interpretation rules

* `numeric/raw`:
  * `scope_id` and `group_id` are interpreted as unsigned integers (big-endian) and displayed in decimal.
  * `memo` is rendered as hex (full 12 bytes, with 0x prefix).
* `ASCII`:
  * Bytes must be printable 7-bit ASCII (0x20..0x7E).
  * Trailing `0x00` bytes are trimmed for display.
  * If any non-printable byte is present, render as hex.

#### Example

```
magic/version/kind/flags = 4e4b4731 01 02 0001
scope_id (ASCII) = 504159524f4c4c00  // "PAYROLL\0"
group_id (numeric) = 00000f42
memo (ASCII) = 4a414e2d323032360000000000  // "JAN-2026"
```

Display:

```
kind=0x02, scope=PAYROLL, group=3906, memo=JAN-2026
```

---

### 6.7 Stop Group (local cancel)

`POST /v1/senders/{sender}/groups/{groupId}/cancel`

* Marks all group members as `canceled_locally`
* Removes them from scheduler
* Removes the signed transactions from the database (keep the rest of the data)
* Does not affect chain state
* Requires header `Authorization: Signature <hex>`: Tempo primitive signature bytes over `keccak256(groupId)` signed by the group owner (sender). Accepts legacy 65-byte secp256k1 signatures or P256/WebAuthn signatures with a 1-byte type prefix (`0x01`/`0x02`) per the Tempo signature spec.

---

## 7) Transaction State Machine

```
queued → broadcasting ↔ retry_scheduled → terminal
```

Terminal states:

* `executed`
* `expired`
* `invalid` (provably invalid)
* `stale_by_nonce`
* `canceled_locally`

---

## 8) Validity Logic

### 8.1 Time window

* `eligibleAt = valid_after || now`
* `expiresAt = valid_before || ∞`

### 8.2 Provably Invalid (stop early)

* malformed tx
* invalid sender signature
* invalid fee payer signature
* expired at ingest

### 8.3 Dynamic (retry until expiry)

* nonce mismatch
* insufficient balance
* fee token selection failure
* temporary RPC errors

---

## 9) Scheduler

### Behavior

* Due txs pulled by `next_action_at`
* DB-backed leasing (multi-replica safe)
* Redis ZSET used as accelerator only
* Guaranteed retry until expiry

### Retry Strategy

* Near eligibility: 250–500ms attempts
* Backoff up to 5s max
* Endpoint rotation
* Health scoring per RPC endpoint

---

## 10) Broadcaster

* Fan-out to multiple Tempo RPC endpoints
* Tracks:
  * accepted
  * rejected (with reason)
  * timeout
* Stops only on terminal conditions

---

## 11) Chain Watcher

Tracks:

* `(sender, nonce_key) → current_nonce`
* receipts for known txs

Transitions:

* mined → terminal
* nonce advanced past tx → `stale_by_nonce`

---

## 12) Storage Model

### `txs`

* `chain_id`
* `tx_hash` (unique)
* `raw_tx`
* `sender`
* `fee_payer`
* `nonce_key`
* `nonce`
* `valid_after`
* `valid_before`
* `eligible_at`
* `expires_at`
* `status`
* `group_id` (16 bytes nullable)
* `next_action_at`
* leasing fields
* timestamps

Indexes:

* `(chain_id, tx_hash)` unique
* `(sender, group_id)`
* `(status, next_action_at)`

---

## 13) Redis

* `watchtower:ready:{chain}` → ZSET(tx_hash, eligibleAt)
* `watchtower:retry:{chain}` → ZSET(tx_hash, nextRetryAt)
* Optional inflight/lease keys

Redis is rebuildable from DB.

---

## 14) Configuration

* `CHAIN_ID`
* RPC endpoints
* max concurrency
* retry schedule
* retention window
* API rate limits

---

## 15) Observability

Metrics:

* ingest rate
* queue depth
* retry counts
* success/failure rates
* time-to-mined

Tracing:

* full tx lifecycle

Logs:

* structured per txHash

---

## 16) Security

* request size limits
* strict decoding
* rate limiting
* no signing
* no private key handling

---

## Result

* Ethereum-style raw tx ingest
* Hash-based idempotency
* Guaranteed broadcast across validity window
* Multi-group per sender
* Grouping derived solely from nonce key
* Deterministic group cancellation via nonce invalidation
* Durable scheduling with Redis acceleration
* Rust monolith deployable as a single service
