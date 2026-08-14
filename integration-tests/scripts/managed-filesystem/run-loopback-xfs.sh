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
target_dir=${CARGO_MAKE_CRATE_TARGET_DIRECTORY:-${CARGO_TARGET_DIR:-${repo_root}/target}}
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
      --preserve-env=GOLEM_MANAGED_XFS_TEST_ROOT,CARGO_TARGET_DIR,CARGO_BUILD_JOBS \
      env PATH="${user_home}/.cargo/bin:/usr/local/bin:/usr/bin:/bin" "$@"
  else
    "$@"
  fi
}

build_test_binaries() {
  local manifest=$1
  run_test timeout --kill-after=30s 20m cargo test \
    -p golem-worker-executor \
    --features managed-xfs-tests \
    --lib \
    --test integration \
    --no-run \
    --message-format=json \
    | python3 -c '
import json
import sys

targets = {"golem_worker_executor", "integration"}
executables = {}
for line in sys.stdin:
    message = json.loads(line)
    name = message.get("target", {}).get("name")
    if message.get("reason") == "compiler-artifact" and name in targets:
        executable = message.get("executable")
        if executable is not None:
            executables[name] = executable
missing = targets - executables.keys()
if missing:
    raise SystemExit(f"cargo did not emit test executables for: {sorted(missing)}")
for name in sorted(executables):
    print(f"{name}\t{executables[name]}")
' > "${manifest}"
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
        "${capable_binary}" "${filter}" --exact --nocapture --report-time
    )
  else
    timeout --kill-after=30s 5m \
      "${capable_binary}" "${filter}" --exact --nocapture --report-time
  fi
}

if [[ $# -gt 0 ]]; then
  run_test "$@"
  exit
fi

if [[ ! -f test-components/it_initial_file_system_release.wasm ]]; then
  run_test cargo build -p golem-cli
  (
    cd test-components/initial-file-system
    run_test "${target_dir}/debug/golem-cli" --preset release build --yes --skip-check
    run_test "${target_dir}/debug/golem-cli" --preset release exec copy
  )
fi

test_binary_manifest=${work_dir}/test-binaries
build_test_binaries "${test_binary_manifest}"
lib_test_binary=
integration_test_binary=
while IFS=$'\t' read -r name executable; do
  case "${name}" in
    golem_worker_executor) lib_test_binary=${executable} ;;
    integration) integration_test_binary=${executable} ;;
  esac
done < "${test_binary_manifest}"

run_privileged_test \
  lib \
  "${lib_test_binary}" \
  services::agent_filesystem::tests::managed_xfs_owns_observes_and_cleans_project_filesystem

run_privileged_test \
  integration \
  "${integration_test_binary}" \
  wasi::p2_p3_filesystem_parity_on_managed_xfs
