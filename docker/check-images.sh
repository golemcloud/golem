#!/usr/bin/env bash
# Builds the linux/amd64 images of the Golem services, of the golem executable and of xfs-quotad
# without pushing them, and checks the contract of each image:
#
# - The image runs as uid and gid 65534, and that user can write the directories of the image
#   where the default configuration keeps data.
# - No file in the image has a setuid or setgid bit.
# - The golem image starts with GOLEM_LITESTREAM=true on a new volume.
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
)

for service in "${services[@]}"; do
    read -r directory binary _ <<<"$service"
    if [ ! -e "$release/$binary" ]; then
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
