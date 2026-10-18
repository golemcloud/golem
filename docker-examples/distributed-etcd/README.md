# Golem with a distributed shard manager (etcd)

Same stack as `../published-postgres`, with one difference: the shard manager stores its shard
lease state in **etcd** instead of a SQL database, which is what allows more than one shard manager
replica to run. This example runs three (`golem-shard-manager-1` to `-3`). Every other service still
uses Postgres.

```sh
docker compose up
```

The Golem APIs are then on `http://localhost:9881`, as in the other example.

Distributed mode is newer than the image tag in `.env` (`GOLEM_IMAGES_VERSION`), which is shared with
`published-postgres`. Point that variable at a release that includes it, or at an image built from
this repository, before bringing the stack up.

### Checking that it runs

The replica that wins the election logs, in order:

```
INFO golem_shard_manager: Configured the etcd client for shard lease state persistence
     endpoints="http://etcd:2379" state_key="/golem/shard-manager/state"
INFO ...leader_election: Elected as the shard manager leader
     leader_key="/golem/shard-manager/leader/..." create_revision=2 granted_ttl=10s
INFO golem_shard_manager: Started shard manager on ports: grpc: 9002
```

and reports `shard_manager_is_serving 1` on its HTTP port's `/metrics`. The other two log
`Campaigning for shard manager leadership` and then `Still standing by; leadership is held
elsewhere` with the leader's address. On `SIGTERM` the leader logs `Released the shard manager
leadership` before exiting, and a standby takes over within milliseconds: `docker compose stop` the
leader and watch another replica log `Elected as the shard manager leader`.

The misconfigurations described below stop a replica at startup:

- `https://` endpoint → exits 1 with `Error: Internal error: etcd endpoint https://... must start
  with http:// (TLS is not supported)`
- no `GOLEM__GRPC__ADVERTISED_HOST` → exits 1 with `grpc.advertised_host must be set in
  distributed mode`
- unbracketed endpoint list → `Failed to load config: invalid type: found string "...", expected a
  sequence for key "PERSISTENCE"`. This one exits with status **0**, as every Golem service does on a
  config it cannot load, so `restart: on-failure` leaves the container stopped and it looks like a
  clean exit; check its log.

## What changes, and why

| | `published-postgres` | here |
|---|---|---|
| `GOLEM__PERSISTENCE__TYPE` | `Postgres` | `Etcd` |
| Shard manager replicas | exactly one | any number (three here); one is elected and serves, the rest stand by |
| `GOLEM__GRPC__ADVERTISED_HOST` | not used | each replica's own service name |
| Quota state | in Postgres | in etcd, with the shard state |

The endpoint list must be bracketed — `'["http://etcd:2379"]'`. Unbracketed it is read as a single
string and fails to deserialize. Only `http://` endpoints are accepted: TLS is not configurable and
the shard manager refuses to start on `https://`.

## Limitations of this example

**One etcd node.** It is enough to show the mode, not to survive losing the container — etcd is now
the shard manager's durable state, so a real deployment runs a cluster.

## How the replicas share one address

**Every replica serves its gRPC port.** The executors and the worker service reach any replica
through the network alias `golem-shard-manager`. A standby does not run a call: it answers with the
leader's address, and the client connects to the leader at that address and keeps using it. Each
replica therefore sets `GOLEM__GRPC__ADVERTISED_HOST` to the host that the other containers reach it
at, here its own service name. On Kubernetes that is the pod IP; see the deployment guide for a full
example.

Use two probes on the HTTP port (`GOLEM__HTTP_PORT`). Readiness is `/readyz`: it answers 200 while the replica
serves, or, for a standby or a just-elected leader still starting, while etcd has answered it within one lease TTL. A replica is also not ready until it has reached its own advertised address, so a wrong `GOLEM__GRPC__ADVERTISED_HOST` keeps it unready, with a warning that names the address. Liveness is `/healthcheck`.
The shard manager image has neither `curl` nor `wget`, so this example defines no container healthcheck.

Each replica's `GOLEM__GRPC__ADVERTISED_HOST` is its own service name. It must stay unique: a replica that
finds another live replica advertising its address refuses to start.

During a failover no replica serves. A graceful handover takes milliseconds for the election. After a kill or
a partition, a standby is elected within `leader_lease_ttl` (10s by default). The new leader then runs its
startup: it serves almost at once when every stored executor lease is still valid, and otherwise checks the
executors for at most `initial_health_check_timeout` (3s by default). The worker executors and the worker
service wait a failover out by themselves, for about 15s, so no client setting needs tuning for it.

`/metrics` on the HTTP port distinguishes the roles: `shard_manager_is_serving` is 1 on the replica
that takes calls, and `shard_manager_is_leader` is 1 on the replica that holds the leadership lease.
They differ only while a new leader starts.

See `docs/src/content/next/deploy.mdx` for the full deployment guide.
