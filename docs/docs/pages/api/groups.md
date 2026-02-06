# Groups

## List Groups

`GET /v1/groups`

Query parameters:

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `sender` | `string` | No | Filter by sender address (hex, 20 bytes) |
| `chainId` | `number` | No | Filter by chain ID |
| `limit` | `number` | No | Max results to return (default: 100, max: 500) |
| `active` | `boolean` | No | Return only active (non-terminal) groups |

Response example:

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

### Response Fields

| Field | Type | Description |
|-------|------|-------------|
| `chainId` | `number` | Chain ID |
| `groupId` | `string` | Group ID (hex) |
| `nonceKey` | `string` | Nonce key (hex U256) |
| `nonceKeyInfo` | `object` | Decoded nonce key components |
| `startAt` | `number` | Unix timestamp of first transaction eligibility |
| `endAt` | `number` | Unix timestamp of last transaction expiration |
| `nextPaymentAt` | `number?` | Unix timestamp of next eligible transaction |

`endAt` is the largest `eligibleAt` for the group. `nextPaymentAt` is the earliest `eligibleAt` for non-terminal transactions in the group. `active=true` returns groups whose `endAt` is in the future.

## Get Group

`GET /v1/senders/{sender}/groups/{groupId}`

Get detailed information about a transaction group including member transactions and cancel plan.

### Path Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `sender` | `string` | Sender address (hex, 20 bytes) |
| `groupId` | `string` | Group ID (hex, 16 bytes) |

### Query Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `chainId` | `number` | No | Required if group exists on multiple chains |

### Response

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

### Response Fields

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

To cancel the group, the user must invalidate all nonces for the group's nonce key. All group members share the same nonce key. Transactions are grouped only when `nonceKey` matches the explicit grouping format.

## Cancel Group (Local)

`POST /v1/senders/{sender}/groups/{groupId}/cancel`

Cancel a group locally. This marks all group transactions as `canceled_locally`, clears stored `raw_tx` data, and removes scheduled retries. This does not affect on-chain state — transactions that have already been broadcast may still be mined.

The specification also notes that local cancel:

- Removes the group from the scheduler.
- Removes the signed transactions from the database while keeping the remaining metadata.

### Headers

| Header | Required | Description |
|--------|----------|-------------|
| `Authorization` | Yes | `Signature <hex>` — Tempo primitive signature bytes over `keccak256(groupId)` signed by the sender. Accepts legacy 65-byte secp256k1 signatures or P256/WebAuthn signatures with a 1-byte type prefix (`0x01`/`0x02`) per the Tempo signature spec. |

### Response

```json
{
  "canceled": 3,
  "txHashes": ["0x...", "0x...", "0x..."]
}
```

### Errors

| Status | Description |
|--------|-------------|
| `401` | Missing or invalid authorization signature |
| `404` | Group not found |
