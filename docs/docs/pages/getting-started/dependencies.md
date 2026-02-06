# Dependencies

This page describes the external services and tooling needed to run Tempo Watchtower in development or production.

## PostgreSQL

PostgreSQL stores transactions, scheduler state, and metadata.
You must create a database for Tempo Watchtower and provide the credentials [when configuring](./configuration).

## Redis

Redis is used to improve scheduling performance. The database remains the source of truth; Redis can be rebuilt from Postgres.

## Reverse Proxy (optional)

A reverse proxy is recommended when exposing the service to the Internet. It can terminate TLS, apply rate limits, and manage access controls in front of the Watchtower API.
We provide a [sample configuration for nginx](https://github.com/arvina-tech/tempo-watchtower/blob/main/deployment/nginx/tempo-watchtower.conf) as part of this repo.

## Supervisor (optional)

A process supervisor is recommended for production deployments to keep the service running and to manage restarts on failure.
We provide a [sample configuration for supervisor](https://github.com/arvina-tech/tempo-watchtower/blob/main/deployment/tempo-watchtower.conf) as part of this repo.
