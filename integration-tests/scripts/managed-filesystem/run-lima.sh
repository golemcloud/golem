#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
instance=golem-managed-xfs-v1
template=${repo_root}/integration-tests/lima/managed-filesystem.yaml

validate_binary_control() {
  local name=$1
  local value=$2
  if [[ ${value} != 0 && ${value} != 1 ]]; then
    echo "${name} must be 0 or 1" >&2
    return 1
  fi
}

load_filesystem_isolation_controls() {
  filesystem_disable_root_capability_reuse=${GOLEM_FILESYSTEM_BENCHMARK_DISABLE_ROOT_CAPABILITY_REUSE:-0}
  filesystem_disable_managed_xfs_name_mode_shortcut=${GOLEM_FILESYSTEM_DISABLE_MANAGED_XFS_NAME_MODE_SHORTCUT:-0}
  filesystem_eager_append_coordination=${GOLEM_FILESYSTEM_BENCH_EAGER_APPEND_COORDINATION:-0}
  validate_binary_control GOLEM_FILESYSTEM_BENCHMARK_DISABLE_ROOT_CAPABILITY_REUSE "${filesystem_disable_root_capability_reuse}" || return
  validate_binary_control GOLEM_FILESYSTEM_DISABLE_MANAGED_XFS_NAME_MODE_SHORTCUT "${filesystem_disable_managed_xfs_name_mode_shortcut}" || return
  validate_binary_control GOLEM_FILESYSTEM_BENCH_EAGER_APPEND_COORDINATION "${filesystem_eager_append_coordination}" || return
  filesystem_isolation_environment=(
    "GOLEM_FILESYSTEM_BENCHMARK_DISABLE_ROOT_CAPABILITY_REUSE=${filesystem_disable_root_capability_reuse}"
    "GOLEM_FILESYSTEM_DISABLE_MANAGED_XFS_NAME_MODE_SHORTCUT=${filesystem_disable_managed_xfs_name_mode_shortcut}"
    "GOLEM_FILESYSTEM_BENCH_EAGER_APPEND_COORDINATION=${filesystem_eager_append_coordination}"
  )
}

assert_isolation_controls() {
  local expected=$1
  local IFS='|'
  local actual="${filesystem_isolation_environment[*]}"
  if [[ ${actual} != "${expected}" ]]; then
    echo "unexpected Lima isolation controls: ${actual}" >&2
    return 1
  fi
}

load_filesystem_isolation_controls

if [[ ${1:-} == --self-test-isolation-controls ]]; then
  for controls in 'default 0 0 0' 'single 1 0 0' 'single 0 1 0' 'single 0 0 1'; do
    read -r source root_disabled xfs_disabled eager_append <<< "${controls}"
    if [[ ${source} == default ]]; then
      unset GOLEM_FILESYSTEM_BENCHMARK_DISABLE_ROOT_CAPABILITY_REUSE
      unset GOLEM_FILESYSTEM_DISABLE_MANAGED_XFS_NAME_MODE_SHORTCUT
      unset GOLEM_FILESYSTEM_BENCH_EAGER_APPEND_COORDINATION
    else
      export GOLEM_FILESYSTEM_BENCHMARK_DISABLE_ROOT_CAPABILITY_REUSE=${root_disabled}
      export GOLEM_FILESYSTEM_DISABLE_MANAGED_XFS_NAME_MODE_SHORTCUT=${xfs_disabled}
      export GOLEM_FILESYSTEM_BENCH_EAGER_APPEND_COORDINATION=${eager_append}
    fi
    load_filesystem_isolation_controls
    assert_isolation_controls "GOLEM_FILESYSTEM_BENCHMARK_DISABLE_ROOT_CAPABILITY_REUSE=${root_disabled}|GOLEM_FILESYSTEM_DISABLE_MANAGED_XFS_NAME_MODE_SHORTCUT=${xfs_disabled}|GOLEM_FILESYSTEM_BENCH_EAGER_APPEND_COORDINATION=${eager_append}"
  done
  for name in \
    GOLEM_FILESYSTEM_BENCHMARK_DISABLE_ROOT_CAPABILITY_REUSE \
    GOLEM_FILESYSTEM_DISABLE_MANAGED_XFS_NAME_MODE_SHORTCUT \
    GOLEM_FILESYSTEM_BENCH_EAGER_APPEND_COORDINATION; do
    if (export "${name}=2"; load_filesystem_isolation_controls) 2>/dev/null; then
      echo "Lima isolation control accepted an invalid value: ${name}" >&2
      exit 1
    fi
  done
  exit 0
fi

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
    "${filesystem_isolation_environment[@]}" \
    GOLEM_FILESYSTEM_BENCH_QUICK="${GOLEM_FILESYSTEM_BENCH_QUICK:-0}" \
    GOLEM_FILESYSTEM_BENCH_SINGLE_AGENT="${GOLEM_FILESYSTEM_BENCH_SINGLE_AGENT:-0}" \
    GOLEM_FILESYSTEM_BENCH_REVERSE_MODES="${GOLEM_FILESYSTEM_BENCH_REVERSE_MODES:-0}" \
    GOLEM_FILESYSTEM_PROTOTYPE_EXECUTION="${GOLEM_FILESYSTEM_PROTOTYPE_EXECUTION:-}" \
    GOLEM_FILESYSTEM_PROTOTYPE_BLOCKING_PERMITS="${GOLEM_FILESYSTEM_PROTOTYPE_BLOCKING_PERMITS:-}" \
    GOLEM_FILESYSTEM_PROTOTYPE_MICRO_OPTIMIZATIONS="${GOLEM_FILESYSTEM_PROTOTYPE_MICRO_OPTIMIZATIONS:-0}" \
    CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" \
    "${repo_root}/integration-tests/scripts/managed-filesystem/run-loopback-xfs.sh" "$@"
done
