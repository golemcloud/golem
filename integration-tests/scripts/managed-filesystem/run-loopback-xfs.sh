#!/usr/bin/env bash
set -euo pipefail

if [[ ${EUID} -ne 0 ]]; then
  echo "run-loopback-xfs.sh must run as root inside a privileged container or VM" >&2
  exit 1
fi

for command in flock losetup mkfs.xfs mount timeout truncate; do
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
target_dir=${GOLEM_MANAGED_XFS_TARGET_DIR:-/var/tmp/golem-managed-xfs-target}
export CARGO_TARGET_DIR=${target_dir}
export CARGO_INCREMENTAL=0
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_PROFILE_TEST_DEBUG=0
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

run_test() {
  if [[ -n ${SUDO_USER:-} ]]; then
    user_home=$(eval echo "~${SUDO_USER}")
    sudo --user "${SUDO_USER}" \
      --preserve-env=GOLEM_MANAGED_XFS_TEST_ROOT,CARGO_TARGET_DIR,CARGO_BUILD_JOBS,CARGO_INCREMENTAL,CARGO_PROFILE_DEV_DEBUG,CARGO_PROFILE_TEST_DEBUG \
      env PATH="${user_home}/.cargo/bin:/usr/local/bin:/usr/bin:/bin" "$@"
  else
    "$@"
  fi
}

run_vm_cargo() {
  if [[ -n ${SUDO_USER:-} ]]; then
    user_home=$(eval echo "~${SUDO_USER}")
    env HOME="${user_home}" PATH="${user_home}/.cargo/bin:/usr/local/bin:/usr/bin:/bin" "$@"
  else
    "$@"
  fi
}

# Never mix Linux test artifacts with a macOS host target, and bound the disk
# consumed by repeated privileged runs.
run_vm_cargo cargo clean --target-dir "${target_dir}"

build_test_binaries() {
  echo "Building managed XFS test binaries with ${CARGO_BUILD_JOBS:-default} Cargo jobs" >&2
  local build_command=(
    timeout --kill-after=30s 20m cargo test
    -p golem-worker-executor
    --lib
    --test integration
    --no-run
  )
  if ! run_vm_cargo "${build_command[@]}"; then
    echo "Initial clean build failed; retrying once with completed dependencies" >&2
    run_vm_cargo "${build_command[@]}"
  fi

  lib_test_binary=
  integration_test_binary=
  local executable
  for executable in "${target_dir}"/debug/deps/golem_worker_executor-*; do
    [[ -f ${executable} && -x ${executable} ]] && lib_test_binary=${executable}
  done
  for executable in "${target_dir}"/debug/deps/integration-*; do
    [[ -f ${executable} && -x ${executable} ]] && integration_test_binary=${executable}
  done
  if [[ -z ${lib_test_binary} || -z ${integration_test_binary} ]]; then
    echo "cargo did not emit both managed XFS test executables" >&2
    return 1
  fi
}

run_privileged_test() {
  local target=$1
  local selected=$2
  local filter=$3

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
  cli_target_dir=${repo_root}/target/managed-xfs-cli-linux
  run_test cargo clean --target-dir "${cli_target_dir}"
  run_test env CARGO_TARGET_DIR="${cli_target_dir}" cargo build -p golem-cli
  (
    cd test-components/initial-file-system
    run_test env CARGO_TARGET_DIR="${cli_target_dir}" \
      "${cli_target_dir}/debug/golem-cli" --preset release build --yes --skip-check
    run_test env CARGO_TARGET_DIR="${cli_target_dir}" \
      "${cli_target_dir}/debug/golem-cli" --preset release exec copy
  )
fi

build_test_binaries

run_privileged_test \
  lib \
  "${lib_test_binary}" \
  services::agent_filesystem::tests::managed_xfs_owns_observes_and_cleans_project_filesystem

run_privileged_test \
  integration \
  "${integration_test_binary}" \
  wasi::initial_file_p2_p3_parity_on_managed_xfs

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
