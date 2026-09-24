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

### What has actually been verified

The compose file parses (`docker compose config`), and the shard manager's half of it was checked
directly: an etcd container started with the flags below, and a shard manager built from this
repository run against it with this file's environment variables. It elected a leader and served:

```
INFO golem_shard_manager: Configured the etcd client for shard lease state persistence
     endpoints="http://127.0.0.1:12379" state_key="/golem/shard-manager/state"
INFO ...leader_election: Elected as the shard manager leader
     leader_key="/golem/shard-manager/leader/..." create_revision=2 granted_ttl=10s
INFO golem_shard_manager: Started shard manager on ports: grpc: 19002
```

with `shard_manager_is_leader 1` on the HTTP port's `/metrics`, and on `SIGTERM` it logged
`Released the shard manager leadership` before exiting. (The ports differ from this file's only
because that run was on the host.) Both misconfigurations this file warns about stop the shard
manager at startup:

- `https://` endpoint → exits 1 with `Error: Internal error: etcd endpoint https://... must start
  with http:// (TLS is not supported)`
- unbracketed endpoint list → `Failed to load config: invalid type: found string "...", expected a
  sequence for key "PERSISTENCE"`. This one exits with status **0**, as every Golem service does on a
  config it cannot load, so `restart: on-failure` leaves the container stopped and it looks like a
  clean exit; check its log.

The rest of the stack — a worker executor registering, quota degrading — has **not** been run,
because the published images predate this mode.

## What changes, and why

| | `published-postgres` | here |
|---|---|---|
| `GOLEM__PERSISTENCE__TYPE` | `Postgres` | `Etcd` |
| Shard manager replicas | exactly one | any number; one is elected, the rest stand by |
| Quota service | available | **unavailable** |

The endpoint list must be bracketed — `'["http://etcd:2379"]'`. Unbracketed it is read as a single
string and fails to deserialize. Only `http://` endpoints are accepted: TLS is not configurable and
the shard manager refuses to start on `https://`.

## Limitations of this example

**One etcd node.** It is enough to show the mode, not to survive losing the container — etcd is now
the shard manager's durable state, so a real deployment runs a cluster.

**One shard manager replica.** Compose has no readiness gating, so scaling this service up here
would not behave like a real deployment. See below.

**Quota is not enforced in this mode.** In distributed mode the quota repository is wired to an
unavailable implementation; quota state has not moved into etcd. Use local (SQL) mode if you need it.

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
