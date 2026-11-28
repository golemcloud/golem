# A fixed Golem cluster on one host

The per-service images with several worker executors, Caddy for TLS, and a Postgres database and
an S3 bucket outside the host. The guide is
[Fixed cluster with Docker Compose](https://learn.golem.cloud/next/deploy/vm#fixed-cluster-with-docker-compose).

First [build the images from this checkout](../README.md). Every Golem image uses the local tag `local`; the example does not pull an older published image.

```sh
cp .env.example .env   # then fill in every value
docker compose up -d
```

- Only Caddy publishes ports (80 and 443). It routes the management host by path to the registry
  service and the worker service, and every deployment domain to the worker service's custom
  request port.
- The shard manager runs as exactly one replica in Postgres mode. Two shard managers on one
  database can run the same agent twice.
- `WORKER_EXECUTOR_REPLICAS` sets the number of worker executors.
- All services share one bucket with `namespace_prefixes = true`. Do not change it after the first
  start.
- The local build embeds the built-in artifacts, so they need no GitHub download. The registry
  service keeps extracted metadata and fetched URL-backed artifacts in the `builtin_artifacts`
  volume. It reuses valid cached entries after container replacement. Source overrides can select
  URLs even in a binary with embedded artifacts.
