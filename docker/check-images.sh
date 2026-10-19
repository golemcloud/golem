#!/usr/bin/env bash
# Builds the linux/amd64 images of the Golem services, of the golem executable, of xfs-quotad and of
# the router without pushing them, and checks the contract of each image:
#
# - The image runs as uid and gid 65534, and that user can write the directories of the image
#   where the default configuration keeps data.
# - No file in the image has a setuid or setgid bit.
# - The golem image starts with GOLEM_LITESTREAM=true on a new volume.
# - Envoy accepts the configuration that the router image writes, and the router sends each request
#   to its service.
# - The binary of xfs-quotad has the file capability cap_sys_admin+ep.
# - xfs-quotad starts only when the container adds SYS_ADMIN and allows privilege escalation.
#
# Run it from the root of the repository, after `cargo build --release -p golem-xfs-quotad
# --target x86_64-unknown-linux-gnu`. The checks cover the images and not the services, so a
# service binary that is not built is replaced by a placeholder. CONTAINER_ENGINE selects the
# engine (default: docker). The startup checks expect the behaviour of Docker: a process that does
# not run as root gets an added capability only through a file capability, and no_new_privs makes
# Linux ignore that file capability. Rootless Podman gives an added capability to the process
# directly, so its startup check with no_new_privs fails.
set -euo pipefail

engine=${CONTAINER_ENGINE:-docker}
release=target/x86_64-unknown-linux-gnu/release
tag_prefix=golem-image-check

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

if [ ! -x "$release/golem-xfs-quotad" ]; then
    fail "$release/golem-xfs-quotad is missing; build it first"
fi

# <directory of the Dockerfile> <binary> <directories that uid 65534 must write>
services=(
    "golem-worker-executor worker-executor /home/golem /data /data/blob_storage"
    "golem-registry-service golem-registry-service /home/golem /data /data/blob_storage /app/builtin-artifacts"
    "golem-shard-manager golem-shard-manager /home/golem /data /data/blob_storage"
    "golem-worker-service golem-worker-service /home/golem /data /data/blob_storage"
    "golem-component-compilation-service golem-component-compilation-service /home/golem /data /data/blob_storage"
    "cli/golem golem /home/golem /var/lib/golem"
    "golem-xfs-quotad golem-xfs-quotad /run/golem-quota"
    "golem-router - /home/golem"
)

for service in "${services[@]}"; do
    read -r directory binary _ <<<"$service"
    if [ "$binary" != - ] && [ ! -e "$release/$binary" ]; then
        printf '#!/bin/sh\nexit 0\n' >"$release/$binary"
        chmod +x "$release/$binary"
    fi
done

for service in "${services[@]}"; do
    read -r directory binary writable <<<"$service"
    image="$tag_prefix/$directory"
    echo "== $image"
    "$engine" build --quiet --platform linux/amd64 -f "$directory/docker/Dockerfile" -t "$image" . >/dev/null

    user=$("$engine" image inspect --format '{{.Config.User}}' "$image")
    [ "$user" = "65534:65534" ] || fail "$image runs as '$user', not 65534:65534"

    # shellcheck disable=SC2086
    "$engine" run --rm --entrypoint /bin/sh "$image" -c '
        [ "$(id -u):$(id -g)" = "65534:65534" ] || { echo "runs as $(id -u):$(id -g)"; exit 1; }
        for directory in "$@"; do
            [ -d "$directory" ] && [ -w "$directory" ] || { echo "$directory is not writable"; exit 1; }
        done' sh $writable || fail "$image: a directory of the image is not writable by uid 65534"

    privileged=$("$engine" run --rm --user 0 --entrypoint /usr/bin/find "$image" / -xdev -perm /6000 -type f)
    [ -z "$privileged" ] || fail "$image has setuid or setgid files: $privileged"
done

# The entrypoint lists the databases of the Litestream configuration, restores each of them from
# the empty replica, and runs `golem --version` under `litestream replicate`, all as uid 65534.
golem="$tag_prefix/cli/golem"
echo "== $golem: startup with Litestream on a new volume"
volume=$("$engine" volume create)
output=$("$engine" run --rm -v "$volume:/var/lib/golem" -e GOLEM_LITESTREAM=true \
    -e GOLEM_LITESTREAM_REPLICA_URL=file:///var/lib/golem/replica "$golem" --version 2>&1) ||
    fail "$golem with GOLEM_LITESTREAM=true: $output"
"$engine" volume rm "$volume" >/dev/null
databases=$(grep -c '^  - path:' cli/golem/docker/litestream.yml)
restores=$(grep -c "no matching backups found" <<<"$output" || true)
[ "$restores" = "$databases" ] ||
    fail "$golem with GOLEM_LITESTREAM=true restored $restores databases, not $databases: $output"

# The entrypoint writes the Envoy configuration from the variables, and Envoy validates it.
router="$tag_prefix/golem-router"
validate_router() {
    "$engine" run --rm --cap-drop ALL --security-opt no-new-privileges \
        -e GOLEM_ROUTER_MANAGEMENT_HOST=golem.example -e GOLEM_ROUTER_REGISTRY_SERVICE_HOST=registry \
        -e GOLEM_ROUTER_WORKER_SERVICE_HOST=worker "$@" "$router" --mode validate 2>&1
}
echo "== $router: configuration"
output=$(validate_router) || fail "$router: $output"
output=$(validate_router -e GOLEM_ROUTER_STATS_PORT=19001 -e GOLEM_ROUTER_CONCURRENCY=4) ||
    fail "$router with GOLEM_ROUTER_STATS_PORT and GOLEM_ROUTER_CONCURRENCY: $output"
if output=$(validate_router -e GOLEM_ROUTER_CONCURRENCY=0); then
    fail "$router accepted GOLEM_ROUTER_CONCURRENCY=0: $output"
fi
grep -q "golem-router: GOLEM_ROUTER_CONCURRENCY has the value '0'" <<<"$output" ||
    fail "$router with GOLEM_ROUTER_CONCURRENCY=0: $output"

# The router sends each host to its service and forwards the original authority. A management host
# with a trailing dot or a port is still the management host; every other host reaches the custom
# request port. The services are Envoy listeners that answer with their name, and with the
# authority, the protocol and the X-Forwarded-Proto of the request. They accept WebSocket upgrades
# and answer them with their name in a header, because Envoy sends no body after an upgrade.
echo "== $router: routing by host"
network=$("$engine" network create "$tag_prefix-router")
cleanup_router() {
    "$engine" rm -f "$tag_prefix-router" "$tag_prefix-router-trusted" "$tag_prefix-services" >/dev/null 2>&1 || true
    "$engine" network rm "$network" >/dev/null 2>&1 || true
}
trap cleanup_router EXIT
listener() {
    cat <<EOF
  - address: {socket_address: {address: 0.0.0.0, port_value: $1}}
    filter_chains:
    - filters:
      - name: envoy.filters.network.http_connection_manager
        typed_config:
          "@type": type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager
          stat_prefix: s
          codec_type: AUTO
          upgrade_configs:
          - upgrade_type: websocket
          route_config:
            virtual_hosts:
            - name: s
              domains: ["*"]
              response_headers_to_add:
              - header: {key: x-authority, value: "%REQ(:AUTHORITY)%"}
              - header: {key: x-protocol, value: "%PROTOCOL%"}
              - header: {key: x-received-proto, value: "%REQ(X-FORWARDED-PROTO)%"}
              - header: {key: x-service, value: $2}
              routes:
              - match: {prefix: /}
                direct_response: {status: 200, body: {inline_string: $2}}
          http_filters:
          - name: envoy.filters.http.router
            typed_config: {"@type": type.googleapis.com/envoy.extensions.filters.http.router.v3.Router}
EOF
}
stand_ins=$(printf 'static_resources:\n  listeners:\n%s\n%s\n%s\n' "$(listener 8081 registry)" \
    "$(listener 9005 worker)" "$(listener 9006 custom)")
"$engine" run -d --name "$tag_prefix-services" --network "$network" --network-alias registry \
    --network-alias worker --entrypoint envoy "$router" --disable-hot-restart --config-yaml "$stand_ins" >/dev/null
# The first router has the metrics port; the second trusts one proxy in front of it and has none.
start_router() {
    name=$1
    shift
    "$engine" run -d --name "$name" --network "$network" -p 127.0.0.1::8080 -p 127.0.0.1::19001 \
        -e GOLEM_ROUTER_MANAGEMENT_HOST=management.test -e GOLEM_ROUTER_REGISTRY_SERVICE_HOST=registry \
        -e GOLEM_ROUTER_WORKER_SERVICE_HOST=worker -e GOLEM_ROUTER_DNS_REFRESH_RATE=1s "$@" "$router" >/dev/null
}
start_router "$tag_prefix-router" -e GOLEM_ROUTER_STATS_PORT=19001
start_router "$tag_prefix-router-trusted" -e GOLEM_ROUTER_TRUSTED_HOPS=1
address=$("$engine" port "$tag_prefix-router" 8080/tcp | head -n 1)
stats_address=$("$engine" port "$tag_prefix-router" 19001/tcp | head -n 1)
trusted_address=$("$engine" port "$tag_prefix-router-trusted" 8080/tcp | head -n 1)
trusted_stats_address=$("$engine" port "$tag_prefix-router-trusted" 19001/tcp | head -n 1)
# Prints the authority that the service received and the name of the service.
served_by() {
    curl -s -D - --http1.1 -H "Host: $1" "http://$address$2" |
        tr -d '\r' | sed -n -e 's/^x-authority: //p' -e '$p'
}
for _ in $(seq 1 30); do
    [ "$(served_by app.test / | tail -n 1)" = custom ] &&
        [ "$(curl -s --http1.1 -H 'Host: app.test' "http://$trusted_address/")" = custom ] && break
    sleep 1
done
# <host> <path> <service>
cases=(
    "management.test /v1/agents/test worker"
    "management.test:443 /v1/agents/test worker"
    "management.test. /v1/agents/test worker"
    "management.test.:443 /v1/agents/test worker"
    "Management.Test. /v1/agents/test worker"
    "management.test. /v1/components registry"
    "app.test /v1/agents/test custom"
    "app.test.:443 /v1/agents/test custom"
    "management.test.example /v1/agents/test custom"
)
for case in "${cases[@]}"; do
    read -r host path expected <<<"$case"
    result=$(served_by "$host" "$path" | paste -sd ' ')
    [ "$result" = "$host $expected" ] ||
        fail "$router sent $path for host $host to '$result', not to '$host $expected'"
done

# Prints the status, the protocol that the service received and the name of the service, for a
# WebSocket upgrade request.
upgrade_served_by() {
    curl -s -D - -o /dev/null --http1.1 -H "Host: $1" -H 'Connection: Upgrade' -H 'Upgrade: websocket' \
        -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' \
        "http://$address$2" | tr -d '\r' |
        sed -n -e 's/^HTTP\/[0-9.]* \([0-9]*\).*/\1/p' -e 's/^x-protocol: //p' -e 's/^x-service: //p'
}
# The WebSockets of the management API reach the worker service over HTTP/1.1. HTTP API
# deployments serve no WebSockets, so the router refuses an upgrade for them.
# <host> <path> <result>
cases=(
    "management.test /v1/agents/invoke-agent-session 200 HTTP/1.1 worker"
    "management.test /v1/components/c/workers/w/connect 200 HTTP/1.1 worker"
    "app.test /ws 403"
)
for case in "${cases[@]}"; do
    read -r host path expected <<<"$case"
    result=$(upgrade_served_by "$host" "$path" | paste -sd ' ')
    [ "$result" = "$expected" ] ||
        fail "$router answered the upgrade of $path for host $host with '$result', not '$expected'"
done

# The metrics of the services stay inside the deployment, also with an encoded path.
for path in /metrics /%6detrics; do
    status=$(curl -s -o /dev/null -w '%{http_code}' --http1.1 -H 'Host: management.test' "http://$address$path")
    [ "$status" = 404 ] || fail "$router answered $path on the management host with $status, not 404"
done

# Without trusted proxies, the router replaces the X-Forwarded-Proto of the client by its own
# scheme. With one trusted proxy, it keeps it.
received_proto() {
    curl -s -D - -o /dev/null --http1.1 -H 'Host: app.test' -H 'X-Forwarded-Proto: https' "http://$1/" |
        tr -d '\r' | sed -n 's/^x-received-proto: //p'
}
proto=$(received_proto "$address")
[ "$proto" = http ] || fail "$router without trusted hops sent X-Forwarded-Proto '$proto', not 'http'"
proto=$(received_proto "$trusted_address")
[ "$proto" = https ] || fail "$router with GOLEM_ROUTER_TRUSTED_HOPS=1 sent X-Forwarded-Proto '$proto', not 'https'"

# The metrics port serves only the Prometheus metrics of Envoy, and only when it is set.
metrics=$(curl -s "http://$stats_address/stats/prometheus")
grep -q '^envoy_' <<<"$metrics" || fail "$router answered /stats/prometheus without Envoy metrics"
for path in /stats /config_dump /quitquitquit /; do
    status=$(curl -s -o /dev/null -w '%{http_code}' "http://$stats_address$path")
    [ "$status" = 404 ] || fail "$router answered $path on the metrics port with $status, not 404"
done
status=$(curl -s -o /dev/null -w '%{http_code}' "http://$trusted_stats_address/stats/prometheus" || true)
[ "$status" = 000 ] || fail "$router without GOLEM_ROUTER_STATS_PORT answered on port 19001 with $status"
cleanup_router
trap - EXIT

quotad="$tag_prefix/golem-xfs-quotad"
echo "== $quotad: file capability"
archive=$(mktemp -d)
trap 'rm -rf "$archive"' EXIT
"$engine" save -o "$archive/image.tar" "$quotad"
python3 - "$archive/image.tar" app/golem-xfs-quotad <<'EOF'
import io
import struct
import sys
import tarfile

# The security.capability xattr of the path in the layers of an image archive, in VFS_CAP_REVISION_2
# form: the magic with the effective flag, then the permitted and inheritable words of each half.
CAP_SYS_ADMIN = 21
archive, path = sys.argv[1], sys.argv[2]
found = None
with tarfile.open(archive) as outer:
    for member in outer.getmembers():
        if not member.isfile():
            continue
        try:
            layer = tarfile.open(fileobj=io.BytesIO(outer.extractfile(member).read()))
        except tarfile.TarError:
            continue
        for entry in layer.getmembers():
            if entry.name.lstrip("./") == path:
                value = entry.pax_headers.get("SCHILY.xattr.security.capability")
                found = value.encode("latin-1") if isinstance(value, str) else value
if not found:
    sys.exit(f"{path} has no file capability")
magic, permitted, inheritable, permitted_high, inheritable_high = struct.unpack("<5I", found[:20])
expected = (0x02000001, 1 << CAP_SYS_ADMIN, 0, 0, 0)
if (magic, permitted, inheritable, permitted_high, inheritable_high) != expected:
    sys.exit(f"{path} has the file capability {found.hex()}, not cap_sys_admin+ep")
EOF

# xfs-quotad checks CAP_SYS_ADMIN before it opens its root. The root here is not on XFS, so a
# process that holds the capability stops at the root check, and one that does not stops before.
run_quotad() {
    "$engine" run --rm --cap-drop ALL "$@" "$quotad" \
        --root /tmp --listen /run/golem-quota/xfs-quotad.sock 2>&1 || true
}

# The startup checks below show the file capability only when the engine does not give an added
# capability to a process of uid 65534 directly, for example in its ambient set. A binary without a
# file capability must then run with no effective capability.
echo "== $quotad: no effective capability without the file capability"
effective=$("$engine" run --rm --cap-drop ALL --cap-add SYS_ADMIN --entrypoint /usr/bin/grep "$quotad" \
    '^CapEff:' /proc/self/status)
[ "$effective" = "$(printf 'CapEff:\t0000000000000000')" ] ||
    fail "$engine gives an added capability to uid 65534 directly ($effective), so the startup checks cannot show the file capability"

echo "== $quotad: startup without SYS_ADMIN"
output=$(run_quotad)
grep -q "Operation not permitted" <<<"$output" || fail "without SYS_ADMIN: $output"

echo "== $quotad: startup with SYS_ADMIN and no_new_privs"
output=$(run_quotad --cap-add SYS_ADMIN --security-opt no-new-privileges)
grep -q "xfs-quotad needs CAP_SYS_ADMIN" <<<"$output" || fail "with no_new_privs: $output"

echo "== $quotad: startup with SYS_ADMIN"
output=$(run_quotad --cap-add SYS_ADMIN)
grep -q "is not on an XFS filesystem" <<<"$output" || fail "with SYS_ADMIN: $output"

echo "All image checks passed"
