# Transactions

## Submit Transactions (Batch)

`POST /v1/transactions`

Submit one or more signed transactions for broadcasting.

### Request

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

### Response

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

### Behavior

- Hash-based idempotency: `(chainId, txHash)` is unique, and resubmission returns the existing record.
- Static validation performed at ingest: decoding, signature verification, and not already expired.
- Dynamic validity (nonce, balance) is handled by the scheduler.

## Get Transaction

`GET /v1/transactions/{txHash}`

Retrieve a single transaction by hash.

### Path Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `txHash` | `string` | Transaction hash (hex, 32 bytes) |

### Query Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `chainId` | `number` | No | Filter by chain ID (required if tx exists on multiple chains) |

### Response

Returns a `TxInfo` object.

### Errors

| Status | Description |
|--------|-------------|
| `400` | Invalid transaction hash format |
| `404` | Transaction not found |

## Cancel Transaction (Mark Stale by Nonce)

`DELETE /v1/transactions/{txHash}`

Mark a transaction as `stale_by_nonce` when its nonce has been consumed by another transaction on-chain.

### Query Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `chainId` | `number` | No | Filter by chain ID |

### Response

Returns the updated `TxInfo` object with status `stale_by_nonce`.

### Errors

| Status | Description |
|--------|-------------|
| `400` | Transaction nonce has not been invalidated on-chain |
| `400` | Transaction is already in a terminal state |
| `404` | Transaction not found |

### Behavior

- Fetches the current nonce for the transaction's nonce key.
- If the current nonce is higher than the transaction nonce, marks the transaction as `stale_by_nonce`.
- Otherwise returns `400`.

## List Transactions

`GET /v1/transactions`

Query transactions with optional filters.

### Query Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `chainId` | `number` | No | Filter by chain ID |
| `sender` | `string` | No | Filter by sender address (hex, 20 bytes) |
| `groupId` | `string` | No | Filter by group ID (hex, 16 bytes) |
| `ungrouped` | `boolean` | No | Return only transactions without a group (cannot combine with `groupId`) |
| `status` | `string` | No | Filter by status (can be repeated for multiple statuses) |
| `limit` | `number` | No | Max results to return (default: 100, max: 500) |

### Example

```
GET /v1/transactions?sender=0x1234...&status=queued&status=retry_scheduled&chainId=42431&limit=50
```

### Response

Returns an array of `TxInfo` objects.
