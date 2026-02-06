# Common Types

## Transaction Status

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

## TxInfo Object

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
