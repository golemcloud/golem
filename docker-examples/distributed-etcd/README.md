# Golem with a distributed shard manager (etcd)

Same stack as `../published-postgres`, with one difference: the shard manager stores its shard
lease state in **etcd** instead of a SQL database, which is what allows more than one shard manager
replica to run. Every other service still uses Postgres.

```sh
docker compose up
```

The Golem APIs are then on `http://localhost:9881`, as in the other example.

Distributed mode is newer than the image tag in `.env` (`GOLEM_IMAGES_VERSION`), which is shared with
`published-postgres`. Point that variable at a release that includes it, or at an image built from
this repository, before bringing the stack up.

### Checking that it runs

A shard manager running in this mode logs, in order:

```
INFO golem_shard_manager: Configured the etcd client for shard lease state persistence
     endpoints="http://etcd:2379" state_key="/golem/shard-manager/state"
INFO ...leader_election: Elected as the shard manager leader
     leader_key="/golem/shard-manager/leader/..." create_revision=2 granted_ttl=10s
INFO golem_shard_manager: Started shard manager on ports: grpc: 9002
```

and reports `shard_manager_is_leader 1` on its HTTP port's `/metrics`. On `SIGTERM` it logs
`Released the shard manager leadership` before exiting.

Both misconfigurations described below stop it at startup:

- `https://` endpoint without TLS settings → exits 1 with `Error: Invalid configuration: etcd endpoint
  https://... uses https://, which needs persistence.config.tls (at least ca_cert_file or ca_cert)`
- unbracketed endpoint list → `Failed to load config: invalid type: found string "...", expected a
  sequence for key "PERSISTENCE"`. This one exits with status **0**, as every Golem service does on a
  config it cannot load, so `restart: on-failure` leaves the container stopped and it looks like a
  clean exit; check its log.

## What changes, and why

| | `published-postgres` | here |
|---|---|---|
| `GOLEM__PERSISTENCE__TYPE` | `Postgres` | `Etcd` |
| Shard manager replicas | exactly one | any number; one is elected, the rest stand by |
| Quota state | in Postgres | in etcd, with the shard state |

The endpoint list must be bracketed — `'["http://etcd:2379"]'`. Unbracketed it is read as a single
string and fails to deserialize. The etcd here is plaintext, so the endpoint is `http://`.

### With TLS and an etcd user

For an etcd that requires TLS, client certificates or a user, the shard manager takes the
certificates as files (or as Base64 values in `__TLS__CA_CERT`, `__TLS__CLIENT_CERT` and
`__TLS__CLIENT_KEY`) and the password from a file, and every endpoint is `https://`. A user without
TLS is refused, so switch both on together. The deployment guide has the details.

```yaml
services:
  golem-shard-manager:
    environment:
      GOLEM__PERSISTENCE__TYPE: Etcd
      GOLEM__PERSISTENCE__CONFIG__ENDPOINTS: '["https://etcd:2379"]'
      GOLEM__PERSISTENCE__CONFIG__TLS__CA_CERT_FILE: /etc/etcd-tls/ca.pem
      GOLEM__PERSISTENCE__CONFIG__TLS__CLIENT_CERT_FILE: /etc/etcd-tls/client.pem
      GOLEM__PERSISTENCE__CONFIG__TLS__CLIENT_KEY_FILE: /etc/etcd-tls/client-key.pem
      GOLEM__PERSISTENCE__CONFIG__USERNAME: golem-shard-manager
      GOLEM__PERSISTENCE__CONFIG__PASSWORD_FILE: /run/secrets/etcd-password
    volumes:
      - ./etcd-tls:/etc/etcd-tls:ro
    secrets:
      - etcd-password
  etcd:
    command:
      - /usr/local/bin/etcd
      - --listen-client-urls=https://0.0.0.0:2379
      - --advertise-client-urls=https://etcd:2379
      - --cert-file=/etc/etcd-tls/server.pem
      - --key-file=/etc/etcd-tls/server-key.pem
      - --trusted-ca-file=/etc/etcd-tls/ca.pem
      - --client-cert-auth
    volumes:
      - ./etcd-tls:/etc/etcd-tls:ro

secrets:
  etcd-password:
    file: ./etcd-password.txt
```

etcd's server certificate must name `etcd` (the endpoint host), or set
`GOLEM__PERSISTENCE__CONFIG__TLS__SERVER_NAME` to a name it holds. The etcd user and its role
(`readwrite` on `/golem/`) are created with `etcdctl` before authentication is enabled.

## Limitations of this example

**One etcd node.** It is enough to show the mode, not to survive losing the container — etcd is now
the shard manager's durable state, so a real deployment runs a cluster.

**One shard manager replica.** Compose has no readiness gating, so scaling this service up here
would not behave like a real deployment. See below.

## Running more than one replica for real

A standby **does not open its gRPC port until it is elected** — that is what routes traffic to the
leader. It does bind its HTTP port while campaigning, so:

- **liveness** probe → the HTTP port (`GOLEM__HTTP_PORT`)
- **readiness** probe → the gRPC port (`GOLEM__GRPC__PORT`)

Wiring readiness to HTTP would put every standby into the service's endpoint list, and clients would
get connection-refused on every request that did not land on the leader. Clients hold a single shard
manager address and do not fail over between replicas themselves.

During a failover the gRPC port is closed on every replica. A graceful handover takes milliseconds;
an ungraceful one (kill or partition) takes between two thirds and one times `leader_lease_ttl`
(10s by default), plus the new leader's own startup, whose initial executor health check is capped
at 15s. A worker executor that starts inside that window retries its registration for
about 10s and then exits.

`/metrics` on the HTTP port distinguishes the roles: `shard_manager_is_leader` is 1 on the leader and
0 on a standby.

See `docs/src/content/next/deploy.mdx` for the full deployment guide.
