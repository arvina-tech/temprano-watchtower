# Tempo Watchtower — Full Software Specification

## 1) Purpose

A Rust monolith service that:

* Accepts **raw signed Tempo transactions** (same format as `eth_sendRawTransaction`, e.g. output of `viem.signTransaction`)
* Stores them durably
* Broadcasts them to Tempo nodes **as soon as they are valid**
* Keeps retrying **throughout their validity window** (guaranteed delivery mode)
* Supports **grouping of transactions via a structured TIP-20 memo**
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

---

## 5) Group Memo Format (TIP-20 transferWithMemo)

Grouping is encoded inside the 32-byte TIP-20 `memo` field using a structured container.

### 5.1 Memo Layout (bytes32)

```
[0..3]   MAGIC   = 0x54574752   // "TWGR"
[4]      VERSION = 0x01
[5]      FLAGS   bitfield
[6..7]   TYPE    = 0x0001       // group container
[8..23]  GROUP_ID (16 bytes)
[24..31] AUX      (8 bytes)
```

### 5.2 Semantics

* **MAGIC** identifies this memo as a Watchtower Group Container
* **VERSION** is the version of the memo format
* **FLAGS** is a bitfield that controls the meaning of the memo
* **GROUP_ID** is a client-generated 128-bit random identifier
* **AUX** allows coexistence with other memo usage:

Flags defines how the group id and aux fields are encoded.
This is not used by this software but can be helpful for other software that wants to use the same memo format.
The first two bits are used to define how the group id is encoded. The next two bits are used to define how the aux field is encoded. The remaining bits are reserved.

The possible values are:
- 00: value is an ASCII-ish / bytes for UI
- 01: value contains first 8 (for aux) or 16 (for group id) bytes of a keccak256 hash of an external “human memo” stored elsewhere
- 10: value is an app-defined tag
- 11: value is non of the above

### 5.3 Parsing Rules

When ingesting a transaction:

* All `transferWithMemo` calls are inspected
* If a matching group container is found → tx is grouped
* If no group container → ungrouped transaction

---

## 6) HTTP API (Normal REST)

Base path: `/v1`

---

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
      "eligibleAt": 1730000000,
      "expiresAt": 1730003600,
      "group": {
        "groupId": "0x00112233445566778899aabbccddeeff",
        "aux": "0x0000000000000000",
        "version": 1
      },
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

### 6.3 List Transactions

`GET /v1/transactions?sender=0x...&groupId=0x...&status=queued`

---

### 6.4 List Groups

`GET /v1/senders/{sender}/groups?chainId=42431&limit=100&active=true`

```json
[
  {
    "chainId": 42431,
    "groupId": "0x00112233445566778899aabbccddeeff",
    "aux": "0x0000000000000000",
    "version": 1,
    "flags": 0,
    "startAt": 1730000000,
    "endAt": 1730003600
  }
]
```

Notes:
* `endAt` is the largest `eligibleAt` for the group.
* `active=true` returns groups whose `endAt` is in the future.

---

### 6.5 Get Group + Cancel Plan

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
    "byNonceKey": [
      {
        "nonceKey": "0x1",
        "nonces": [100,101],
        "alreadyInvalidated": false
      }
    ]
  }
}
```

Meaning:

* To cancel the group, user must invalidate all nonces for each nonce_key.

---

### 6.6 Stop Group (local cancel)

`POST /v1/senders/{sender}/groups/{groupId}/cancel`

* Marks all group members as `canceled_locally`
* Removes them from scheduler
* Removes the signed transactions from the database (keep the rest of the data)
* Does not affect chain state
* Requires header `Authorization: Signature <hex>`: 65-byte hex secp256k1 signature of `keccak256(groupId)` signed by the group owner (sender)

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
* `group_aux` (8 bytes nullable)
* `group_version`
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
* Explicit structured memo grouping
* Deterministic group cancellation via nonce invalidation
* Durable scheduling with Redis acceleration
* Rust monolith deployable as a single service
