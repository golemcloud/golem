# One all-in-one Golem server on one host

The `golemservices/golem` image and Caddy for TLS. The guide is
[All-in-one image with Docker Compose](https://learn.golem.cloud/next/deploy/vm#all-in-one-image-with-docker-compose).

```sh
cp .env.example .env                                  # then fill in every value
GOLEM_S3_BUCKET=my-bucket ./generate-env.sh > golem.env
echo "AWS_ACCESS_KEY_ID=..." >> golem.env
echo "AWS_SECRET_ACCESS_KEY=..." >> golem.env
docker compose up -d
```

- `generate-env.sh` prints new secrets and the S3 configuration of every service. With
  `GOLEM_POSTGRES_HOST` and `GOLEM_POSTGRES_PASSWORD`, it also prints the Postgres configuration,
  and the state moves from SQLite to Postgres. Keep `golem.env` secret.
- The server runs as uid and gid 65534. The `golem_data` named volume needs nothing; a bind mount
  must belong to that user (`sudo chown -R 65534:65534 <dir>`).
- Only Caddy publishes ports (80 and 443). The server serves the management host, the HTTP APIs
  and MCP on one port, 9881, and routes by host.
- Run exactly one server: two servers on one database can run the same agent twice.
- Litestream (`GOLEM_LITESTREAM=true`) is backup and restore for one server, not high availability.
  It replicates the SQLite databases asynchronously, about once a second, so the newest writes can be
  lost with the disk, and at startup it restores only the databases that are missing. Use it only
  where the disk is ephemeral or belongs to one host. See
  [Litestream limits](https://learn.golem.cloud/next/deploy/server#litestream-limits).
