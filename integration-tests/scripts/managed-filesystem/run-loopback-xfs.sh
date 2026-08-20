#!/usr/bin/env bash
set -euo pipefail

if [[ ${EUID} -ne 0 ]]; then
  echo "run-loopback-xfs.sh must run as root inside a privileged container or VM" >&2
  exit 1
fi

for command in flock losetup mkfs.xfs mount python3 timeout truncate; do
  if ! command -v "${command}" >/dev/null 2>&1; then
    echo "missing required command: ${command}" >&2
    exit 1
  fi
done

exec 9>/tmp/golem-managed-xfs.lock
if ! flock --wait 10 9; then
  echo "another managed XFS test run is still active" >&2
  exit 1
fi

repo_root=${GOLEM_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)}
target_dir=${GOLEM_MANAGED_XFS_TARGET_DIR:-${CARGO_TARGET_DIR:-/var/tmp/golem-managed-xfs-target}}
cli_target_dir=${GOLEM_MANAGED_XFS_CLI_TARGET_DIR:-${repo_root}/target/managed-xfs-cli-linux}
clean_build=${GOLEM_MANAGED_XFS_CLEAN:-0}
if [[ ${clean_build} != 0 && ${clean_build} != 1 ]]; then
  echo "GOLEM_MANAGED_XFS_CLEAN must be 0 or 1" >&2
  exit 1
fi
minimum_free_gib=${GOLEM_MANAGED_XFS_MIN_FREE_GIB:-15}
if [[ ! ${minimum_free_gib} =~ ^[0-9]+$ ]]; then
  echo "GOLEM_MANAGED_XFS_MIN_FREE_GIB must be a non-negative integer" >&2
  exit 1
fi
validate_cache_only=${GOLEM_MANAGED_XFS_VALIDATE_CACHE_ONLY:-0}
if [[ ${validate_cache_only} != 0 && ${validate_cache_only} != 1 ]]; then
  echo "GOLEM_MANAGED_XFS_VALIDATE_CACHE_ONLY must be 0 or 1" >&2
  exit 1
fi
reuse_test_binaries=${GOLEM_MANAGED_XFS_REUSE_TEST_BINARIES:-0}
if [[ ${reuse_test_binaries} != 0 && ${reuse_test_binaries} != 1 ]]; then
  echo "GOLEM_MANAGED_XFS_REUSE_TEST_BINARIES must be 0 or 1" >&2
  exit 1
fi
cargo_test_r=${GOLEM_MANAGED_XFS_CARGO_TEST_R:-cargo-test-r}
export CARGO_TARGET_DIR=${target_dir}
export CARGO_INCREMENTAL=0
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_PROFILE_DEV_INCREMENTAL=false
export CARGO_PROFILE_TEST_DEBUG=0
export CARGO_PROFILE_TEST_INCREMENTAL=false

run_test() {
  if [[ -n ${SUDO_USER:-} ]]; then
    user_home=$(eval echo "~${SUDO_USER}")
    sudo --user "${SUDO_USER}" --set-home \
      --preserve-env=GOLEM_MANAGED_XFS_TEST_ROOT,GOLEM_MANAGED_XFS_TARGET_DIR,GOLEM_MANAGED_XFS_CLEAN,GOLEM_MANAGED_XFS_MIN_FREE_GIB,CARGO_TARGET_DIR,CARGO_BUILD_JOBS,CARGO_INCREMENTAL,CARGO_PROFILE_DEV_DEBUG,CARGO_PROFILE_DEV_INCREMENTAL,CARGO_PROFILE_TEST_DEBUG,CARGO_PROFILE_TEST_INCREMENTAL \
      env PATH="${user_home}/.cargo/bin:/usr/local/bin:/usr/bin:/bin" "$@"
  else
    "$@"
  fi
}

run_vm_cargo() {
  if [[ -n ${SUDO_USER:-} ]]; then
    user_home=$(eval echo "~${SUDO_USER}")
    sudo --user "${SUDO_USER}" --set-home \
      --preserve-env=GOLEM_MANAGED_XFS_TEST_ROOT,GOLEM_MANAGED_XFS_TARGET_DIR,GOLEM_MANAGED_XFS_CLEAN,GOLEM_MANAGED_XFS_MIN_FREE_GIB,CARGO_TARGET_DIR,CARGO_BUILD_JOBS,CARGO_INCREMENTAL,CARGO_PROFILE_DEV_DEBUG,CARGO_PROFILE_DEV_INCREMENTAL,CARGO_PROFILE_TEST_DEBUG,CARGO_PROFILE_TEST_INCREMENTAL \
      env PATH="${user_home}/.cargo/bin:/usr/local/bin:/usr/bin:/bin" "$@"
  else
    "$@"
  fi
}

clean_cargo_cache_if_needed() {
  local cache_dir=$1
  local cache_name=$2
  local runner=$3
  local available_kib=
  local minimum_free_kib=$((minimum_free_gib * 1024 * 1024))
  local write_probe=${cache_dir}/.golem-managed-xfs-write-probe.$$

  if ! "${runner}" mkdir -p "${cache_dir}" ||
    ! "${runner}" test -w "${cache_dir}" ||
    ! "${runner}" touch "${write_probe}" ||
    ! "${runner}" rm "${write_probe}"; then
    echo "${cache_name} is not writable by its build user: ${cache_dir}" >&2
    echo "Remove or repair that cache directory as its owner, then rerun this script." >&2
    exit 1
  fi
  while read -r _ _ _ available _; do
    if [[ ${available} =~ ^[0-9]+$ ]]; then
      available_kib=${available}
    fi
  done < <(df -Pk "${cache_dir}")
  if [[ -z ${available_kib} ]]; then
    echo "failed to determine free space for ${cache_dir}" >&2
    exit 1
  fi
  if [[ ${clean_build} == 1 || ${available_kib} -lt ${minimum_free_kib} ]]; then
    if [[ ${clean_build} == 0 ]]; then
      echo "Cleaning ${cache_name}: less than ${minimum_free_gib} GiB is available" >&2
    fi
    "${runner}" cargo clean --target-dir "${cache_dir}"
  fi
}

clean_cargo_cache_if_needed "${target_dir}" "managed XFS Cargo cache" run_vm_cargo

if [[ ${validate_cache_only} == 1 ]]; then
  clean_cargo_cache_if_needed "${cli_target_dir}" "managed XFS CLI Cargo cache" run_test
  run_vm_cargo test -w "${target_dir}"
  run_test test -w "${cli_target_dir}"
  exit
fi

work_dir=$(mktemp -d /tmp/golem-managed-xfs.XXXXXX)
chmod 0755 "${work_dir}"
image=${work_dir}/filesystem.img
mount_point=${work_dir}/mount
loop_device=

cleanup() {
  set +e
  sync
  if mountpoint -q "${mount_point}"; then
    umount "${mount_point}"
  fi
  if [[ -n ${loop_device} ]]; then
    losetup --detach "${loop_device}"
  fi
  rm -rf "${work_dir}"
}
trap cleanup EXIT INT TERM

truncate --size 1G "${image}"
loop_device=$(losetup --find --show "${image}")
mkfs.xfs -f -m reflink=1 -n ftype=1 "${loop_device}"
mkdir -p "${mount_point}"
mount -t xfs -o prjquota "${loop_device}" "${mount_point}"
mkdir -p "${mount_point}/agents"
chmod 0777 "${mount_point}/agents"
if [[ -n ${SUDO_UID:-} && -n ${SUDO_GID:-} ]]; then
  chown "${SUDO_UID}:${SUDO_GID}" "${mount_point}/agents"
fi

export GOLEM_MANAGED_XFS_TEST_ROOT=${mount_point}/agents
cd "${repo_root}"

build_test_binaries() {
  echo "Building managed XFS test binaries with ${CARGO_BUILD_JOBS:-default} Cargo jobs" >&2
  local build_messages=${work_dir}/cargo-test-artifacts.jsonl
  local selected_executables=${work_dir}/cargo-test-executables.tsv
  local lib_decoy=${target_dir}/debug/deps/golem_worker_executor-stale-decoy
  local integration_decoy=${target_dir}/debug/deps/integration-stale-decoy
  run_vm_cargo mkdir -p "${target_dir}/debug/deps"
  run_vm_cargo touch -t 203801010000 "${lib_decoy}" "${integration_decoy}"
  run_vm_cargo chmod 0755 "${lib_decoy}" "${integration_decoy}"
  local build_command=(
    timeout --kill-after=30s 20m cargo test
    -p golem-worker-executor
    --lib
    --test integration
    --no-run
    --message-format=json-render-diagnostics
  )
  if ! run_vm_cargo "${build_command[@]}" > "${build_messages}"; then
    echo "Initial clean build failed; retrying once with completed dependencies" >&2
    if ! run_vm_cargo "${build_command[@]}" > "${build_messages}"; then
      run_vm_cargo rm -f "${lib_decoy}" "${integration_decoy}"
      return 1
    fi
  fi

  lib_test_binary=
  integration_test_binary=
  if ! run_vm_cargo python3 \
    "${repo_root}/integration-tests/scripts/managed-filesystem/select-cargo-test-executables.py" \
    "${build_messages}" "${repo_root}" > "${selected_executables}"; then
    run_vm_cargo rm -f "${lib_decoy}" "${integration_decoy}"
    return 1
  fi
  local target executable
  while IFS=$'\t' read -r target executable; do
    case ${target} in
      lib) lib_test_binary=${executable} ;;
      integration) integration_test_binary=${executable} ;;
      *)
        echo "unexpected Cargo test target selected: ${target}" >&2
        run_vm_cargo rm -f "${lib_decoy}" "${integration_decoy}"
        return 1
        ;;
    esac
  done < "${selected_executables}"
  run_vm_cargo rm -f "${lib_decoy}" "${integration_decoy}"
  if [[ -z ${lib_test_binary} || -z ${integration_test_binary} ]]; then
    echo "cargo did not emit both managed XFS test executables" >&2
    return 1
  fi
  if [[ ${lib_test_binary} == "${lib_decoy}" || ${integration_test_binary} == "${integration_decoy}" ]]; then
    echo "Cargo artifact selection accepted a stale decoy executable" >&2
    return 1
  fi
}

run_privileged_test() {
  local target=$1
  local selected=$2
  local filter=$3

  if [[ ${reuse_test_binaries} == 1 ]]; then
    local target_args=()
    if [[ ${target} == integration ]]; then
      target_args=(--test integration)
    else
      target_args=(--lib)
    fi
    timeout --kill-after=30s 5m \
      "${cargo_test_r}" run --package golem-worker-executor "${target_args[@]}" \
      "${filter}" -- --exact --include-ignored --nocapture --report-time
    return
  fi

  local capable_binary=${work_dir}/${target}-capable
  cp "${selected}" "${capable_binary}"
  chmod 0755 "${capable_binary}"
  if [[ ${target} == integration ]]; then
    (
      cd "${repo_root}/golem-worker-executor"
      timeout --kill-after=30s 5m \
        "${capable_binary}" "${filter}" --exact --include-ignored --nocapture --report-time
    )
  else
    timeout --kill-after=30s 5m \
      "${capable_binary}" "${filter}" --exact --include-ignored --nocapture --report-time
  fi
}

if [[ $# -gt 0 ]]; then
  run_test "$@"
  exit
fi

initial_file_wasm=test-components/it_initial_file_system_release.wasm
rebuild_initial_file_wasm=false
if [[ ! -f ${initial_file_wasm} ]]; then
  rebuild_initial_file_wasm=true
else
  shopt -s globstar nullglob
  for source in Cargo.toml \
    Cargo.lock \
    test-components/initial-file-system/Cargo.toml \
    test-components/initial-file-system/Cargo.lock \
    test-components/initial-file-system/golem.yaml \
    test-components/golem-test-components-common.yaml \
    sdks/rust/golem-rust/Cargo.toml \
    sdks/rust/golem-rust/src/**/*.rs \
    sdks/rust/golem-rust/wit/**/*.wit \
    sdks/rust/golem-rust-macro/Cargo.toml \
    sdks/rust/golem-rust-macro/src/**/*.rs \
    test-components/initial-file-system/src/**/*.rs; do
    if [[ ${source} -nt ${initial_file_wasm} ]]; then
      rebuild_initial_file_wasm=true
      break
    fi
  done
fi
if [[ ${rebuild_initial_file_wasm} == true ]]; then
  clean_cargo_cache_if_needed "${cli_target_dir}" "managed XFS CLI Cargo cache" run_test
  run_test env CARGO_TARGET_DIR="${cli_target_dir}" cargo build -p golem-cli
  (
    cd test-components/initial-file-system
    run_test env CARGO_TARGET_DIR="${cli_target_dir}" \
      "${cli_target_dir}/debug/golem-cli" --preset release build --yes --skip-check
    run_test env CARGO_TARGET_DIR="${cli_target_dir}" \
      "${cli_target_dir}/debug/golem-cli" --preset release exec copy
  )
fi

lib_test_binary=
integration_test_binary=
if [[ ${reuse_test_binaries} == 0 ]]; then
  build_test_binaries
fi

run_privileged_test \
  lib \
  "${lib_test_binary}" \
  services::agent_filesystem::tests::managed_xfs_owns_observes_and_cleans_project_filesystem

run_privileged_test \
  lib \
  "${lib_test_binary}" \
  services::agent_filesystem::tests::managed_xfs_allocated_bytes_flow_through_resource_billing

run_privileged_test \
  integration \
  "${integration_test_binary}" \
  wasi::initial_file_p2_p3_parity_on_managed_xfs

run_privileged_test \
  integration \
  "${integration_test_binary}" \
  wasi::managed_xfs_resource_billing_survives_idle_and_replay

run_privileged_test \
  integration \
  "${integration_test_binary}" \
  wasi::filesystem_full_replay_survives_managed_xfs_lifecycle_transitions

run_privileged_test \
  integration \
  "${integration_test_binary}" \
  wasi::filesystem_mutation_histories_reconstruct_on_managed_xfs

run_privileged_test \
  integration \
  "${integration_test_binary}" \
  wasi::filesystem_reconstruction_stops_at_exact_revert_target_on_managed_xfs

run_privileged_test \
  integration \
  "${integration_test_binary}" \
  wasi::filesystem_reconstruction_uses_updated_initial_files_on_managed_xfs

run_privileged_test \
  integration \
  "${integration_test_binary}" \
  wasi::p2_p3_mid_effect_enospc_reconstructs_on_unmanaged_filesystem

run_privileged_test \
  integration \
  "${integration_test_binary}" \
  wasi::p2_p3_quota_exhaustion_on_managed_xfs

run_privileged_test \
  integration \
  "${integration_test_binary}" \
  wasi::p2_p3_object_quota_on_managed_xfs

run_privileged_test \
  integration \
  "${integration_test_binary}" \
  wasi::filesystem_downgrade_blocks_guest_until_limit_recovers
