#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
instance=golem-managed-xfs-v1
template=${repo_root}/integration-tests/lima/managed-filesystem.yaml

if ! command -v limactl >/dev/null 2>&1; then
  echo "missing required command: limactl" >&2
  exit 1
fi

exists=false
while read -r name; do
  if [[ ${name} == "${instance}" ]]; then
    exists=true
  fi
done < <(limactl list --format '{{.Name}}')

if [[ ${exists} == false ]]; then
  limactl start --name "${instance}" --mount-only "${repo_root}:w" "${template}"
else
  limactl start "${instance}"
fi

guest_home=$(limactl shell "${instance}" -- printenv HOME)
runs=${GOLEM_MANAGED_XFS_RUNS:-1}
if [[ ! ${runs} =~ ^[1-9][0-9]*$ ]]; then
  echo "GOLEM_MANAGED_XFS_RUNS must be a positive integer" >&2
  exit 1
fi

for ((run = 1; run <= runs; run++)); do
  echo "Managed XFS Lima run ${run}/${runs}"
  limactl shell "${instance}" -- \
    sudo GOLEM_REPO_ROOT="${repo_root}" \
    GOLEM_MANAGED_XFS_TARGET_DIR="${GOLEM_MANAGED_XFS_TARGET_DIR:-${guest_home}/.cache/golem-managed-xfs-target}" \
    GOLEM_MANAGED_XFS_CLI_TARGET_DIR="${guest_home}/.cache/golem-managed-xfs-cli-target" \
    GOLEM_MANAGED_XFS_CLEAN="${GOLEM_MANAGED_XFS_CLEAN:-0}" \
    GOLEM_MANAGED_XFS_MIN_FREE_GIB="${GOLEM_MANAGED_XFS_MIN_FREE_GIB:-15}" \
    GOLEM_MANAGED_XFS_VALIDATE_CACHE_ONLY="${GOLEM_MANAGED_XFS_VALIDATE_CACHE_ONLY:-0}" \
    GOLEM_MANAGED_XFS_REUSE_TEST_BINARIES="${GOLEM_MANAGED_XFS_REUSE_TEST_BINARIES:-0}" \
    GOLEM_MANAGED_XFS_CARGO_TEST_R="${GOLEM_MANAGED_XFS_CARGO_TEST_R:-cargo-test-r}" \
    CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" \
    "${repo_root}/integration-tests/scripts/managed-filesystem/run-loopback-xfs.sh" "$@"
done
