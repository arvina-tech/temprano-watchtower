# tempo-watchtower

Tempo Watchtower is a Rust service that accepts signed Tempo transactions, stores them durably, and broadcasts them throughout their validity window until mined, expired, invalid, or canceled. It groups transactions by nonce key and lets clients cancel groups locally.

## Hosted endpoint

Watchtower is available at `https://watchtower.temprano.io`. For now it is open and does not require any auth, but this could change in the future.

## API

### JSON-RPC raw transaction submit

`POST /rpc`

Accepts JSON-RPC 2.0 `eth_sendRawTransaction` requests with a single raw transaction hex string. The service extracts the `chainId` from the transaction, validates it against `rpc.chains`, and stores it for broadcasting.

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "eth_sendRawTransaction",
  "params": ["0x..."]
}
```

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": "0x..."
}
```

Errors use standard JSON-RPC codes: `-32600` invalid request, `-32601` method not found, `-32602` invalid params (including unsupported chain IDs or invalid raw tx), `-32603` internal error.

### Submit transactions (batch)

`POST /v1/transactions`

```json
{
  "chainId": 42431,
  "transactions": ["0x..."]
}
```

Returns an array of results with per-tx status, scheduling data, and optional group info.

### Get transaction

`GET /v1/transactions/{txHash}`

Optional query: `chainId`.

### Cancel transaction (stale by nonce)

`DELETE /v1/transactions/{txHash}`

Optional query: `chainId`.

Marks the transaction as `stale_by_nonce` when the current nonce is higher than the transaction nonce.
Returns `400` if the nonce has not been invalidated or the transaction is already terminal.

### List transactions

`GET /v1/transactions?sender=0x...&groupId=0x...&status=queued&status=retry_scheduled&chainId=42431&limit=100`

Use `ungrouped=true` to return only transactions without a `groupId` (cannot be combined with `groupId`).

### List groups

`GET /v1/senders/{sender}/groups?chainId=42431&limit=100&active=true`

### Get group + cancel plan

`GET /v1/senders/{sender}/groups/{groupId}`

Optional query: `chainId`.

### Cancel group (local)

`POST /v1/senders/{sender}/groups/{groupId}/cancel`

Marks the group as `canceled_locally`, clears `raw_tx`, and removes scheduled retries. This does not affect on-chain state.
Requires header `Authorization: Signature <hex>`: a 65-byte hex secp256k1 signature of `keccak256(groupId)` signed by the group owner (sender).

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

The end-to-end test uses the real Postgres and Redis instances configured in `config.toml`.
