# Golem with a distributed shard manager (etcd)

Same stack as `../published-postgres`, with one difference: the shard manager stores its shard
lease state in **etcd** instead of a SQL database, which is what allows more than one shard manager
replica to run. This example runs three (`golem-shard-manager-1` to `-3`). Every other service still
uses Postgres.

```sh
docker compose up
```

The Golem APIs are then on `http://localhost:9881`, as in the other example. The router sends
requests for `localhost` (`GOLEM_ROUTER_MANAGEMENT_HOST`) to the Golem APIs, and requests for every
other host to the HTTP API and MCP deployments.

Distributed mode and the `golem-router` image are newer than the image tag in `.env`
(`GOLEM_IMAGES_VERSION`), which is shared with `published-postgres`. Point that variable at a release
that includes them, or at images built from this repository, before bringing the stack up.

### Checking that it runs

The replica that wins the election logs, in order:

```
INFO golem_shard_manager: Configured the etcd client for shard lease state persistence
     endpoints="http://etcd:2379" state_key="/golem/shard-manager/state"
INFO ...leader_election: Elected as the shard manager leader
     leader_key="/golem/shard-manager/leader/..." create_revision=2 granted_ttl=10s
INFO golem_shard_manager: Started shard manager
     grpc_port=9002
```

and reports `shard_manager_is_serving 1` on its HTTP port's `/metrics`. The other two log
`Campaigning for shard manager leadership` and then `Still standing by; leadership is held
elsewhere` with the leader's address. On `SIGTERM` the leader logs `Released the shard manager
leadership` before exiting, and a standby takes over within milliseconds: `docker compose stop` the
leader and watch another replica log `Elected as the shard manager leader`.

The misconfigurations described below stop a replica at startup:

- `https://` endpoint without TLS settings → exits 1 with `Error: Invalid configuration: etcd endpoint
  https://... uses https://, which needs persistence.config.tls (at least ca_cert_file or ca_cert)`
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
string and fails to deserialize. The etcd here is plaintext, so the endpoint is `http://`.

### With TLS and an etcd user

`compose.tls.yaml` switches this example's etcd to TLS with client certificates and an etcd user,
and points the shard manager at it:

```sh
docker compose -f compose.yaml -f compose.tls.yaml up
```

It needs these files beside the compose files:

- `etcd-tls/ca.pem`: the CA that signed the two certificates below.
- `etcd-tls/server.pem` and `etcd-tls/server-key.pem`: etcd's certificate and key. The certificate
  must name `etcd`, the endpoint host, or set `GOLEM__PERSISTENCE__CONFIG__TLS__SERVER_NAME` to a
  name it holds.
- `etcd-tls/client.pem` and `etcd-tls/client-key.pem`: the shard manager's client certificate and
  its unencrypted key.
- `etcd-password.txt`: the etcd user's password; one trailing line ending is removed.

The shard manager's image runs as user 65534, so the client key and the password file must be
readable by that user (for this local example, `chmod 644`). Instead of files, the certificates can
also be given as values in `__TLS__CA_CERT`, `__TLS__CLIENT_CERT` and `__TLS__CLIENT_KEY`: the PEM
in standard Base64 on one line, as `base64 -w0` writes it. A user without TLS is refused, so TLS and
the user are switched on together.

Create the etcd user and its role once, before the shard manager starts for the first time:

```sh
etcdctl() {
  docker compose -f compose.yaml -f compose.tls.yaml exec -T etcd /usr/local/bin/etcdctl \
    --endpoints=https://etcd:2379 --cacert=/etc/etcd-tls/ca.pem \
    --cert=/etc/etcd-tls/client.pem --key=/etc/etcd-tls/client-key.pem "$@"
}
docker compose -f compose.yaml -f compose.tls.yaml up -d --wait etcd
etcdctl role add golem-shard-manager
etcdctl role grant-permission golem-shard-manager --prefix=true readwrite /golem/
etcdctl user add "golem-shard-manager:$(cat etcd-password.txt)" --interactive=false
etcdctl user grant-role golem-shard-manager golem-shard-manager
etcdctl user add "root:<a root password>" --interactive=false
etcdctl user grant-role root root
etcdctl auth enable
```

The client certificate's common name is `golem-shard-manager`, the same as the etcd user, so etcd's
health check above still passes once authentication is on: etcd takes the user from the certificate
when no password is sent. The docs' Shard Manager page (`docs/src/content/next/operate/shard-manager.mdx`)
has the details of every setting.

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
serves, or, for a standby or a just-elected leader still starting, while etcd has answered it within one lease TTL. A replica is also not ready until it has reached its own advertised address, so a wrong `GOLEM__GRPC__ADVERTISED_HOST` keeps it unready, with a warning that names the address; such a replica does not join the election either, so it neither queues nor leads. Liveness is `/healthcheck`.
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
