# Health

`GET /health`

Returns service status, version, and runtime/config details. If Redis or Postgres are unavailable, the endpoint returns HTTP 503 and `status: "degraded"`.

## Response

```json
{
  "status": "ok",
  "service": "tempo-watchtower",
  "version": "0.2.6",
  "build": {
    "gitSha": "abc123",
    "buildTimestamp": "2026-02-03T12:34:56Z"
  },
  "now": 1738612345,
  "startedAt": 1738610000,
  "uptimeSeconds": 2345,
  "chains": [42431],
  "rpcEndpoints": 2,
  "scheduler": {
    "pollIntervalMs": 1000,
    "leaseTtlSeconds": 30,
    "maxConcurrency": 10,
    "retryMinMs": 100,
    "retryMaxMs": 1000,
    "expirySoonWindowSeconds": 3600,
    "expirySoonRetryMaxMs": 5000
  },
  "watcher": {
    "pollIntervalMs": 1000,
    "useWebsocket": false
  },
  "broadcaster": {
    "fanout": 2,
    "timeoutMs": 500
  },
  "api": {
    "maxBodyBytes": 1048576
  },
  "dependencies": {
    "database": { "ok": true },
    "redis": { "ok": true }
  }
}
```

`build.gitSha` and `build.buildTimestamp` are omitted when not provided at build time.
