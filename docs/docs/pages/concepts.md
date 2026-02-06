# Concepts

## Delivery Semantics

- Broadcast starts at `valid_after` (or immediately if unset).
- Retry continues until:
  - the transaction is mined,
  - the validity window expires,
  - the transaction is provably invalid,
  - or the user cancels locally.

The transaction can fail to be executed for a number of reasons. A non-exhaustive list is:

- Sender out-of-funds
- Fees too low for transaction to be included
- RPC failure

The Watchtower will always retry to send the transaction unless it [becomes invalid](#validity-logic).

## Transaction Format

The service accepts the same payload as Ethereum raw transaction submission:

```
rawTx = "0x" + hex-encoded signed Tempo tx bytes
```

This is the output of:

- `viem.signTransaction()`
- Any Tempo wallet signer

The server performs:

- decoding
- signature validation
- field extraction
- scheduling

Notes:

- Any valid Tempo transaction is accepted, including ones with custom nonce keys.
- The service accepts JSON-RPC `eth_sendRawTransaction` payloads as-is.

## Grouping

### Group Identity

Users may run multiple concurrent groups (for example, multiple subscriptions). Group identity is:

```
GroupKey = (sender_address, group_id)
```

Each sender can have arbitrarily many groups.

### How Grouping Works

Grouping is based solely on the transaction nonce key. Transaction input data and memo contents are ignored.

For every accepted transaction, the group fields are derived from the nonce key:

- `group_id` = first 16 bytes of `keccak256(nonce_key_bytes)`

All transactions that share a nonce key are placed in the same group for a given sender.

Transactions are grouped only when `nonceKey` matches the explicit grouping format described below.

## Nonce Key Format

Nonce keys are 32-byte values. Transactions are grouped only when the nonce key matches this explicit format.

### Binary Layout (Big-Endian)

```
0..3   magic     = 0x4E4B4731   // "NKG1"
4      version   = 0x01
5      kind      = enum (purpose)
6..7   flags     = u16
8..15  scope_id  = u64
16..19 group_id  = u32
20..31 memo      = 12 bytes
```

### Flags Encoding (u16)

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

### Display / Interpretation Rules

- `numeric/raw`:
  - `scope_id` and `group_id` are interpreted as unsigned integers (big-endian) and displayed in decimal.
  - `memo` is rendered as hex (full 12 bytes, with 0x prefix).
- `ASCII`:
  - Bytes must be printable 7-bit ASCII (0x20..0x7E).
  - Trailing `0x00` bytes are trimmed for display.
  - If any non-printable byte is present, render as hex.

### Example

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

## State Machine

Transactions follow this lifecycle:

```
queued → broadcasting ↔ retry_scheduled → terminal
```

Terminal states:

- `executed`
- `expired`
- `invalid` (provably invalid)
- `stale_by_nonce`
- `canceled_locally`

## Validity Logic

### Time Window

- `eligibleAt = valid_after || now`
- `expiresAt = valid_before || ∞`

### Provably Invalid (Stop Early)

- malformed tx
- invalid sender signature
- invalid fee payer signature
- expired at ingest
- nonce mismatch (if transaction is replaced on-chain)

### Dynamic (Retry Until Expiry)

- insufficient balance
- fee token selection failure
- temporary RPC errors
