# Temprano Watchtower

Tempo Watchtower is a Rust service that accepts signed Tempo transactions, stores them durably, and broadcasts them throughout their validity window.

## What It Does

- Accepts raw signed Tempo transactions (same format as `eth_sendRawTransaction`).
- Stores transactions durably for guaranteed delivery.
- Broadcasts as soon as transactions are valid and retries throughout their validity window.
- Groups transactions by nonce key and allows local group cancellation.
- Exposes JSON-RPC and REST APIs for ingestion and querying.

## Hosted Endpoint

Watchtower is available at `https://watchtower.temprano.io`.

## Where To Go Next

- Start with the [Getting Started section](./getting-started/installation.md) for installation and configuration.
- Use the [API Reference](./api/index.md) for request and response formats.
- Read [Concepts](./concepts.md) and [System Design](./system-design.md) for guarantees, grouping logic, and internal behavior.
