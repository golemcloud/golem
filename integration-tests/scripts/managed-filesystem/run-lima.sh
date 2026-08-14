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
    CARGO_TARGET_DIR="${guest_home}/.cache/golem-managed-xfs-target" \
    CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}" \
    "${repo_root}/integration-tests/scripts/managed-filesystem/run-loopback-xfs.sh" "$@"
done
