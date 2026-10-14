# A fixed Golem cluster on one host

The per-service images with several worker executors, Caddy for TLS, and a Postgres database and
an S3 bucket outside the host. The guide is
[Fixed cluster with Docker Compose](https://learn.golem.cloud/next/deploy/vm#fixed-cluster-with-docker-compose).

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
