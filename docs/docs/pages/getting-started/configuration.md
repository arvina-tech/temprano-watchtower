# Configuration

The service reads `config.toml` by default. You can override the path with the `CONFIG_PATH` environment variable. The config file supports environment variable interpolation (for example `${DB_HOST}`).

## Sample `config.toml`

```toml
[server]
bind = "0.0.0.0:8080"

[database]
url = "postgres://${DB_USER}@${DB_HOST}:${DB_PORT}/${DB_NAME}"

[redis]
url = "redis://${REDIS_HOST}:${REDIS_PORT}/${REDIS_DB}"

[rpc]
# Per-chain RPC endpoints. Keys are chain IDs.
[rpc.chains]
"42431" = ["${RPC_URL}"]

[scheduler]
poll_interval_ms = 200
lease_ttl_seconds = 30
max_concurrency = 50
retry_min_ms = 250
retry_max_ms = 900000
expiry_soon_window_seconds = 3600
expiry_soon_retry_max_ms = 5000

[broadcaster]
fanout = 2
timeout_ms = 2000

[watcher]
poll_interval_ms = 1500
use_websocket = true

[api]
max_body_bytes = 1048576
```

## `server`

- `bind`: Address and port to listen on (for example `0.0.0.0:8080`).

## `database`

- `url`: PostgreSQL connection string. Environment variables may be interpolated.

## `redis`

- `url`: Redis connection string. Environment variables may be interpolated.

## `rpc`

- `chains`: Map of chain IDs to one or more RPC URLs for each chain. Chain IDs are string keys in the TOML file, and each value is an array of URLs. These endpoints are used by the broadcaster and watcher.

## `scheduler`

- `poll_interval_ms`: How often the scheduler scans for due work.
- `lease_ttl_seconds`: Duration of DB-backed leases used for multi-replica scheduling.
- `max_concurrency`: Maximum number of concurrent scheduler tasks.
- `retry_min_ms`: Minimum delay between retry attempts near eligibility.
- `retry_max_ms`: Maximum backoff delay between retry attempts.
- `expiry_soon_window_seconds`: Window before expiry during which retry cadence is adjusted.
- `expiry_soon_retry_max_ms`: Maximum retry delay when a transaction is nearing expiry.

## `broadcaster`

- `fanout`: Number of RPC endpoints to broadcast to in parallel.
- `timeout_ms`: Per-endpoint broadcast timeout in milliseconds.

## `watcher`

- `poll_interval_ms`: How often the watcher polls for updates when websocket subscriptions are unavailable or disabled.
- `use_websocket`: Whether to use websocket subscriptions when supported by the RPC endpoint.

## `api`

- `max_body_bytes`: Maximum request body size accepted by the API.
