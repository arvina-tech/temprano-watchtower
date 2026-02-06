# JSON-RPC

## Submit Raw Transaction

`POST /rpc`

Accepts JSON-RPC 2.0 `eth_sendRawTransaction` requests. The service extracts the `chainId` from the transaction, validates it against configured chains, and stores it for broadcasting.

### Request

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

### Response (Success)

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": "0x...tx_hash..."
}
```

### Response (Error)

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
