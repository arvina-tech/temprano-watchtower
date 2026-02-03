# tempo-watchtower

Tempo Watchtower is a Rust service that accepts signed Tempo transactions, stores them durably, and broadcasts them throughout their validity window until mined, expired, invalid, or canceled. It groups transactions by nonce key and lets clients cancel groups locally.

## Hosted endpoint

Watchtower is available at `https://watchtower.temprano.io`. For now it is open and does not require any auth, but this could change in the future.

## API

### Common Types

#### Transaction Status

Transactions can have the following statuses:

| Status | Description |
|--------|-------------|
| `queued` | Transaction is waiting to be broadcast |
| `broadcasting` | Transaction is currently being broadcast |
| `retry_scheduled` | Broadcast failed, retry is scheduled |
| `executed` | Transaction was mined successfully |
| `expired` | Transaction's validity window expired |
| `invalid` | Transaction was rejected as invalid |
| `stale_by_nonce` | Nonce was consumed by another transaction |
| `canceled_locally` | Group was canceled via the API |

#### TxInfo Object

All transaction endpoints return or include `TxInfo` objects:

| Field | Type | Description |
|-------|------|-------------|
| `chainId` | `number` | Chain ID |
| `txHash` | `string` | Transaction hash (hex) |
| `type` | `number?` | Transaction type (EIP-2718) |
| `sender` | `string` | Sender address (hex) |
| `feePayer` | `string?` | Fee payer address if different from sender (hex) |
| `nonceKey` | `string` | Nonce key (hex U256) |
| `nonce` | `number` | Transaction nonce |
| `groupId` | `string?` | Group ID if part of a group (hex) |
| `validAfter` | `number?` | Unix timestamp when tx becomes valid |
| `validBefore` | `number?` | Unix timestamp when tx expires |
| `eligibleAt` | `number` | Unix timestamp when broadcasting begins |
| `expiresAt` | `number?` | Unix timestamp when tx expires |
| `status` | `string` | Current transaction status |
| `nextActionAt` | `number?` | Unix timestamp of next scheduled action |
| `attempts` | `number` | Number of broadcast attempts |
| `lastError` | `string?` | Last broadcast error message |
| `lastBroadcastAt` | `number?` | Unix timestamp of last broadcast |
| `receipt` | `object?` | Transaction receipt if executed |
| `gas` | `number?` | Gas limit |
| `gasPrice` | `string?` | Gas price (for legacy txs) |
| `maxFeePerGas` | `string?` | Max fee per gas |
| `maxPriorityFeePerGas` | `string?` | Max priority fee per gas |
| `input` | `string?` | Transaction input data (hex) |
| `calls` | `array?` | Decoded calls for batch transactions |

If raw transaction data is not stored (for example after canceling a group locally), fields derived from the raw transaction (`type`, `gas`, `gasPrice`, `maxFeePerGas`, `maxPriorityFeePerGas`, `input`, `calls`) are omitted.

---

### JSON-RPC: Submit Raw Transaction

**`POST /rpc`**

Accepts JSON-RPC 2.0 `eth_sendRawTransaction` requests. The service extracts the `chainId` from the transaction, validates it against configured chains, and stores it for broadcasting.

#### Request

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "eth_sendRawTransaction",
  "params": ["0x...signed_tx_hex..."]
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `jsonrpc` | `string` | Yes | Must be `"2.0"` |
| `id` | `any` | No | Request identifier |
| `method` | `string` | Yes | Must be `"eth_sendRawTransaction"` |
| `params` | `array` | Yes | Array with single hex-encoded signed transaction |

#### Response (Success)

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": "0x...tx_hash..."
}
```

#### Response (Error)

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": {
    "code": -32602,
    "message": "unsupported chainId 1"
  }
}
```

| Code | Meaning |
|------|---------|
| `-32600` | Invalid request (malformed JSON-RPC) |
| `-32601` | Method not found |
| `-32602` | Invalid params (bad tx, unsupported chain, expired, etc.) |
| `-32603` | Internal error |

---

### Submit Transactions (Batch)

**`POST /v1/transactions`**

Submit one or more signed transactions for broadcasting.

#### Request

```json
{
  "chainId": 42431,
  "transactions": ["0x...signed_tx_1...", "0x...signed_tx_2..."]
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `chainId` | `number` | Yes | Target chain ID |
| `transactions` | `string[]` | Yes | Array of hex-encoded signed transactions |

#### Response

```json
{
  "results": [
    {
      "ok": true,
      "txHash": "0x...",
      "sender": "0x...",
      "nonceKey": "0x...",
      "nonce": 5,
      "groupId": "0x...",
      "eligibleAt": 1700000000,
      "expiresAt": 1700003600,
      "status": "queued",
      "alreadyKnown": false
    }
  ]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `results` | `array` | Array of results, one per submitted transaction |
| `results[].ok` | `boolean` | Whether submission succeeded |
| `results[].txHash` | `string?` | Transaction hash (hex) |
| `results[].sender` | `string?` | Sender address (hex) |
| `results[].nonceKey` | `string?` | Nonce key (hex U256) |
| `results[].nonce` | `number?` | Transaction nonce |
| `results[].groupId` | `string?` | Group ID if using group nonce key (hex) |
| `results[].eligibleAt` | `number?` | Unix timestamp when broadcasting begins |
| `results[].expiresAt` | `number?` | Unix timestamp when tx expires |
| `results[].status` | `string?` | Initial transaction status |
| `results[].alreadyKnown` | `boolean?` | True if tx was already in the system |
| `results[].error` | `string?` | Error message if `ok` is false |

---

### Get Transaction

**`GET /v1/transactions/{txHash}`**

Retrieve a single transaction by hash.

#### Path Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `txHash` | `string` | Transaction hash (hex, 32 bytes) |

#### Query Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `chainId` | `number` | No | Filter by chain ID (required if tx exists on multiple chains) |

#### Response

Returns a [`TxInfo`](#txinfo-object) object.

#### Errors

| Status | Description |
|--------|-------------|
| `400` | Invalid transaction hash format |
| `404` | Transaction not found |

---

### Cancel Transaction (Mark Stale by Nonce)

**`DELETE /v1/transactions/{txHash}`**

Mark a transaction as `stale_by_nonce` when its nonce has been consumed by another transaction on-chain.

#### Path Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `txHash` | `string` | Transaction hash (hex, 32 bytes) |

#### Query Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `chainId` | `number` | No | Filter by chain ID |

#### Response

Returns the updated [`TxInfo`](#txinfo-object) object with status `stale_by_nonce`.

#### Errors

| Status | Description |
|--------|-------------|
| `400` | Transaction nonce has not been invalidated on-chain |
| `400` | Transaction is already in a terminal state |
| `404` | Transaction not found |

---

### List Transactions

**`GET /v1/transactions`**

Query transactions with optional filters.

#### Query Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `chainId` | `number` | No | Filter by chain ID |
| `sender` | `string` | No | Filter by sender address (hex, 20 bytes) |
| `groupId` | `string` | No | Filter by group ID (hex, 16 bytes) |
| `ungrouped` | `boolean` | No | Return only transactions without a group (cannot combine with `groupId`) |
| `status` | `string` | No | Filter by status (can be repeated for multiple statuses) |
| `limit` | `number` | No | Max results to return (default: 100, max: 500) |

#### Example

```
GET /v1/transactions?sender=0x1234...&status=queued&status=retry_scheduled&chainId=42431&limit=50
```

#### Response

Returns an array of [`TxInfo`](#txinfo-object) objects.

---

### List Groups

**`GET /v1/groups`**

List transaction groups with optional filters.

#### Query Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `sender` | `string` | No | Filter by sender address (hex, 20 bytes) |
| `chainId` | `number` | No | Filter by chain ID |
| `limit` | `number` | No | Max results to return (default: 100, max: 500) |
| `active` | `boolean` | No | Return only active (non-terminal) groups |

#### Response

```json
[
  {
    "chainId": 42431,
    "groupId": "0x...",
    "nonceKey": "0x...",
    "nonceKeyInfo": {
      "kind": "0x01",
      "scope": { "encoding": "hex", "value": "0x..." },
      "group": { "encoding": "utf8", "value": "my-group" },
      "memo": { "encoding": "hex", "value": "0x..." }
    },
    "startAt": 1700000000,
    "endAt": 1700086400,
    "nextPaymentAt": 1700043200
  }
]
```

| Field | Type | Description |
|-------|------|-------------|
| `chainId` | `number` | Chain ID |
| `groupId` | `string` | Group ID (hex) |
| `nonceKey` | `string` | Nonce key (hex U256) |
| `nonceKeyInfo` | `object` | Decoded nonce key components |
| `startAt` | `number` | Unix timestamp of first transaction eligibility |
| `endAt` | `number` | Unix timestamp of last transaction expiration |
| `nextPaymentAt` | `number?` | Unix timestamp of next eligible transaction |

---

### Get Group

**`GET /v1/senders/{sender}/groups/{groupId}`**

Get detailed information about a transaction group including member transactions and cancel plan.

#### Path Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `sender` | `string` | Sender address (hex, 20 bytes) |
| `groupId` | `string` | Group ID (hex, 16 bytes) |

#### Query Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `chainId` | `number` | No | Required if group exists on multiple chains |

#### Response

```json
{
  "sender": "0x...",
  "groupId": "0x...",
  "nonceKey": "0x...",
  "nonceKeyInfo": {
    "kind": "0x01",
    "scope": { "encoding": "hex", "value": "0x..." },
    "group": { "encoding": "utf8", "value": "my-group" },
    "memo": { "encoding": "hex", "value": "0x..." }
  },
  "members": [
    {
      "txHash": "0x...",
      "nonceKey": "0x...",
      "nonce": 0,
      "status": "executed"
    },
    {
      "txHash": "0x...",
      "nonceKey": "0x...",
      "nonce": 1,
      "status": "queued"
    }
  ],
  "cancelPlan": {
    "nonceKey": "0x...",
    "nonces": [0, 1, 2],
    "alreadyInvalidated": false
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `sender` | `string` | Sender address (hex) |
| `groupId` | `string` | Group ID (hex) |
| `nonceKey` | `string` | Nonce key shared by all members (hex U256) |
| `nonceKeyInfo` | `object` | Decoded nonce key components |
| `members` | `array` | List of group member transactions |
| `members[].txHash` | `string` | Transaction hash (hex) |
| `members[].nonceKey` | `string` | Nonce key (hex U256) |
| `members[].nonce` | `number` | Transaction nonce |
| `members[].status` | `string` | Transaction status |
| `cancelPlan` | `object` | Information for canceling the group on-chain |
| `cancelPlan.nonceKey` | `string` | Nonce key to use for cancellation |
| `cancelPlan.nonces` | `number[]` | Nonces that need to be invalidated |
| `cancelPlan.alreadyInvalidated` | `boolean` | True if all nonces are already invalid |

---

### Cancel Group (Local)

**`POST /v1/senders/{sender}/groups/{groupId}/cancel`**

Cancel a group locally. This marks all group transactions as `canceled_locally`, clears stored `raw_tx` data, and removes scheduled retries. **This does not affect on-chain state** — transactions that have already been broadcast may still be mined.

#### Path Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `sender` | `string` | Sender address (hex, 20 bytes) |
| `groupId` | `string` | Group ID (hex, 16 bytes) |

#### Headers

| Header | Required | Description |
|--------|----------|-------------|
| `Authorization` | Yes | `Signature <hex>` — Tempo primitive signature bytes over `keccak256(groupId)` signed by the sender. Accepts legacy 65-byte secp256k1 signatures or P256/WebAuthn signatures with a 1-byte type prefix (`0x01`/`0x02`) per the Tempo signature spec. |

#### Response

```json
{
  "canceled": 3,
  "txHashes": ["0x...", "0x...", "0x..."]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `canceled` | `number` | Number of transactions canceled |
| `txHashes` | `string[]` | Hashes of canceled transactions (hex) |

#### Errors

| Status | Description |
|--------|-------------|
| `401` | Missing or invalid authorization signature |
| `404` | Group not found |

## Development

### Requirements

- Rust toolchain
- Postgres
- Redis

### Configuration

The service reads `config.toml` by default. You can override the path with `CONFIG_PATH`. The config file supports environment variable interpolation (for example `${DB_HOST}`).

Key config sections:
- `server.bind`: Address to listen on.
- `database.url`: Postgres connection string.
- `redis.url`: Redis connection string.
- `rpc.chains`: Map of chain IDs to one or more RPC URLs.
- `scheduler`, `broadcaster`, `watcher`, `api`: Runtime tuning knobs.

### Running

```bash
cargo run
```

On startup the service runs database migrations automatically.

### Git hooks

To enforce that release tags match the `Cargo.toml` package version (ignoring a leading `v`), this repo includes a `reference-transaction` hook. Enable it with:

```bash
git config core.hooksPath .githooks
```

## Notes

- Any valid Tempo transaction is accepted, including ones with custom nonce keys.
- The watcher uses websocket subscriptions when available and falls back to polling.
- Redis is used as a scheduling accelerator; the database remains the source of truth.

### Tests

```bash
cargo test
```

The end-to-end test uses Postgres and Redis from environment variables (loaded via `.env` if present).
Set `DB_NAME_TEST` to point tests at a separate database (falls back to `DB_NAME` if unset), or use
`TEST_DATABASE_URL` for a full connection string. `REDIS_DB` controls the Redis database index.
