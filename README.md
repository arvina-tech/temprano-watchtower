# Temprano Watchtower

Temprano Watchtower is a Rust service that accepts signed Tempo transactions, stores them durably, and broadcasts them throughout their validity window until mined, expired, invalid, or canceled. It groups transactions by nonce key and lets clients cancel groups locally.

Check out the [documentation](https://docs.watchtower.temprano.io) for more information.

## Hosted endpoint

Temprano Watchtower is available at `https://watchtower.temprano.io`. For now it is open and does not require any auth, but this could change in the future.

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
