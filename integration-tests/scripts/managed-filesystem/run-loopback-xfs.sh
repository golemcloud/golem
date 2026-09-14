#!/usr/bin/env bash
set -euo pipefail

require_exactly_one_passed_test() {
  local result_file=$1
  local filter=$2
  local result_count=0
  local exact_pass_count=0
  local line

  while IFS= read -r line; do
    case ${line} in
      "test result:"*)
        result_count=$((result_count + 1))
        case ${line} in
          "test result: ok; 1 passed;"*) exact_pass_count=$((exact_pass_count + 1)) ;;
        esac
        ;;
    esac
  done < "${result_file}"

  if [[ ${result_count} -ne 1 || ${exact_pass_count} -ne 1 ]]; then
    echo "managed XFS selector '${filter}' must report exactly one passing test; result summaries=${result_count}, exact one-pass summaries=${exact_pass_count}" >&2
    return 1
  fi
}

verify_context_switch_reduction() {
  local perf_file=$1
  local baseline=$2
  local minimum_reduction_percent=$3
  local context_switches=
  local value unit event rest

  while IFS=, read -r value unit event rest; do
    if [[ ${event} == context-switches ]]; then
      value=${value//[[:space:]]/}
      if [[ ! ${value} =~ ^[0-9]+$ ]]; then
        echo "invalid context-switch count in ${perf_file}: ${value}" >&2
        return 1
      fi
      context_switches=${value}
    fi
  done < "${perf_file}"
  if [[ -z ${context_switches} ]]; then
    echo "missing context-switch count in ${perf_file}" >&2
    return 1
  fi
  if ((context_switches * 100 > baseline * (100 - minimum_reduction_percent))); then
    echo "context switches ${context_switches} did not fall by ${minimum_reduction_percent}% from ${baseline}" >&2
    return 1
  fi
}

run_with_clean_filesystem_benchmark_environment() {
  env \
    -u GOLEM_FILESYSTEM_BENCH_QUICK \
    -u GOLEM_FILESYSTEM_BENCH_SINGLE_AGENT \
    -u GOLEM_FILESYSTEM_PROTOTYPE_EXECUTION \
    -u GOLEM_FILESYSTEM_PROTOTYPE_BLOCKING_PERMITS \
    -u GOLEM_FILESYSTEM_PROTOTYPE_MICRO_OPTIMIZATIONS \
    "$@"
}

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

run_filesystem_workload_command() {
  run_with_clean_filesystem_benchmark_environment \
    "${filesystem_isolation_environment[@]}" \
    "$@"
}

select_filesystem_image_size() {
  local filesystem_benchmark=$1
  local filesystem_workload_benchmark=$2
  local filesystem_native_workload_baseline=$3
  if [[ ${filesystem_benchmark} == true || ${filesystem_workload_benchmark} == true || ${filesystem_native_workload_baseline} == true ]]; then
    printf '%s' 4G
  else
    printf '%s' 1G
  fi
}

if [[ ${1:-} == --self-test-result-guard ]]; then
  guard_fixture_dir=$(mktemp -d /tmp/golem-managed-xfs-result-guard.XXXXXX)
  trap 'rm -rf "${guard_fixture_dir}"' EXIT
  printf '%s\n' 'test result: ok; 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out' > "${guard_fixture_dir}/one"
  printf '%s\n' 'test result: ok; 0 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out' > "${guard_fixture_dir}/zero"
  printf '%s\n' 'test result: ok; 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out' > "${guard_fixture_dir}/multiple"
  require_exactly_one_passed_test "${guard_fixture_dir}/one" self-test-one
  if require_exactly_one_passed_test "${guard_fixture_dir}/zero" self-test-zero 2>/dev/null; then
    echo "managed XFS result guard accepted a zero-test result" >&2
    exit 1
  fi
  if require_exactly_one_passed_test "${guard_fixture_dir}/multiple" self-test-multiple 2>/dev/null; then
    echo "managed XFS result guard accepted a multiple-test result" >&2
    exit 1
  fi
  exit 0
fi

if [[ ${1:-} == --self-test-context-switch-gate ]]; then
  gate_fixture_dir=$(mktemp -d /tmp/golem-filesystem-context-switch-gate.XXXXXX)
  trap 'rm -rf "${gate_fixture_dir}"' EXIT
  printf '%s\n' '27729,,context-switches,1.00,100.00' > "${gate_fixture_dir}/pass"
  printf '%s\n' '27730,,context-switches,1.00,100.00' > "${gate_fixture_dir}/fail"
  verify_context_switch_reduction "${gate_fixture_dir}/pass" 554590 95
  if verify_context_switch_reduction "${gate_fixture_dir}/fail" 554590 95 2>/dev/null; then
    echo "context-switch gate accepted a reduction below 95%" >&2
    exit 1
  fi
  exit 0
fi

if [[ ${1:-} == --self-test-image-size ]]; then
  if [[ $(select_filesystem_image_size false false false) != 1G ]]; then
    echo "ordinary managed XFS suite did not select a 1 GiB image" >&2
    exit 1
  fi
  for modes in 'true false false' 'false true false' 'false false true'; do
    read -r benchmark workload native <<< "${modes}"
    if [[ $(select_filesystem_image_size "${benchmark}" "${workload}" "${native}") != 4G ]]; then
      echo "filesystem benchmark mode did not select a 4 GiB image: ${modes}" >&2
      exit 1
    fi
  done
  exit 0
fi

if [[ ${1:-} == --self-test-workload-environment ]]; then
  export GOLEM_FILESYSTEM_BENCH_QUICK=inherited
  export GOLEM_FILESYSTEM_BENCH_SINGLE_AGENT=inherited
  export GOLEM_FILESYSTEM_PROTOTYPE_EXECUTION=inherited
  export GOLEM_FILESYSTEM_PROTOTYPE_BLOCKING_PERMITS=inherited
  export GOLEM_FILESYSTEM_PROTOTYPE_MICRO_OPTIMIZATIONS=inherited
  environment_probe='printf "%s|%s|%s|%s|%s|%s|%s|%s" "${GOLEM_FILESYSTEM_BENCH_QUICK-unset}" "${GOLEM_FILESYSTEM_BENCH_SINGLE_AGENT-unset}" "${GOLEM_FILESYSTEM_PROTOTYPE_EXECUTION-unset}" "${GOLEM_FILESYSTEM_PROTOTYPE_BLOCKING_PERMITS-unset}" "${GOLEM_FILESYSTEM_PROTOTYPE_MICRO_OPTIMIZATIONS-unset}" "${GOLEM_FILESYSTEM_BENCHMARK_DISABLE_ROOT_CAPABILITY_REUSE-unset}" "${GOLEM_FILESYSTEM_DISABLE_MANAGED_XFS_NAME_MODE_SHORTCUT-unset}" "${GOLEM_FILESYSTEM_BENCH_EAGER_APPEND_COORDINATION-unset}"'
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
    expected_controls="${root_disabled}|${xfs_disabled}|${eager_append}"
    clean=$(run_filesystem_workload_command bash -c "${environment_probe}")
    single=$(run_filesystem_workload_command \
      GOLEM_FILESYSTEM_BENCH_SINGLE_AGENT=1 bash -c "${environment_probe}")
    quick=$(run_filesystem_workload_command \
      GOLEM_FILESYSTEM_BENCH_QUICK=1 bash -c "${environment_probe}")
    if [[ ${clean} != "unset|unset|unset|unset|unset|${expected_controls}" ]]; then
      echo "full workload environment was not sanitized and isolated exactly: ${clean}" >&2
      exit 1
    fi
    if [[ ${single} != "unset|1|unset|unset|unset|${expected_controls}" ]]; then
      echo "single-agent workload environment was not sanitized and isolated exactly: ${single}" >&2
      exit 1
    fi
    if [[ ${quick} != "1|unset|unset|unset|unset|${expected_controls}" ]]; then
      echo "quick diagnostic environment was not sanitized and isolated exactly: ${quick}" >&2
      exit 1
    fi
  done
  for name in \
    GOLEM_FILESYSTEM_BENCHMARK_DISABLE_ROOT_CAPABILITY_REUSE \
    GOLEM_FILESYSTEM_DISABLE_MANAGED_XFS_NAME_MODE_SHORTCUT \
    GOLEM_FILESYSTEM_BENCH_EAGER_APPEND_COORDINATION; do
    if (export "${name}=2"; load_filesystem_isolation_controls) 2>/dev/null; then
      echo "workload isolation control accepted an invalid value: ${name}" >&2
      exit 1
    fi
  done
  exit 0
fi

load_filesystem_isolation_controls

if [[ ${EUID} -ne 0 ]]; then
  echo "run-loopback-xfs.sh must run as root inside a privileged container or VM" >&2
  exit 1
fi

filesystem_benchmark=false
filesystem_workload_benchmark=false
filesystem_native_workload_baseline=false
case ${1:-} in
  --filesystem-benchmark)
    filesystem_benchmark=true
    shift
    ;;
  --filesystem-workload-benchmark)
    filesystem_workload_benchmark=true
    shift
    ;;
  --filesystem-native-workload-baseline)
    filesystem_native_workload_baseline=true
    shift
    ;;
esac
if [[ ${filesystem_benchmark} == true || ${filesystem_workload_benchmark} == true || ${filesystem_native_workload_baseline} == true ]] && [[ $# -ne 0 ]]; then
  echo "filesystem benchmark modes do not accept additional arguments" >&2
  exit 1
fi
filesystem_image_size=$(select_filesystem_image_size \
  "${filesystem_benchmark}" \
  "${filesystem_workload_benchmark}" \
  "${filesystem_native_workload_baseline}")

for command in flock losetup mkfs.xfs mount python3 tee timeout truncate; do
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
reverse_benchmark_modes=${GOLEM_FILESYSTEM_BENCH_REVERSE_MODES:-0}
if [[ ${reverse_benchmark_modes} != 0 && ${reverse_benchmark_modes} != 1 ]]; then
  echo "GOLEM_FILESYSTEM_BENCH_REVERSE_MODES must be 0 or 1" >&2
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

truncate --size "${filesystem_image_size}" "${image}"
loop_device=$(losetup --find --show "${image}")
mkfs.xfs -f -m reflink=1 -n ftype=1 "${loop_device}"
mkdir -p "${mount_point}"
mount -t xfs -o prjquota "${loop_device}" "${mount_point}"
chmod 0777 "${mount_point}"
if [[ -n ${SUDO_UID:-} && -n ${SUDO_GID:-} ]]; then
  chown "${SUDO_UID}:${SUDO_GID}" "${mount_point}"
fi

export GOLEM_MANAGED_XFS_TEST_ROOT=${mount_point}
cd "${repo_root}"

build_test_binaries() {
  local cargo_profile_args=()
  local cargo_profile_dir=debug
  if [[ ${filesystem_benchmark} == true || ${filesystem_workload_benchmark} == true || ${filesystem_native_workload_baseline} == true ]]; then
    cargo_profile_args=(--profile benchmarks)
    cargo_profile_dir=benchmarks
  fi
  echo "Building managed XFS test binaries with profile ${cargo_profile_dir} and ${CARGO_BUILD_JOBS:-default} Cargo jobs" >&2
  local build_messages=${work_dir}/cargo-test-artifacts.jsonl
  local selected_executables=${work_dir}/cargo-test-executables.tsv
  local lib_decoy=${target_dir}/${cargo_profile_dir}/deps/golem_worker_executor-stale-decoy
  local integration_decoy=${target_dir}/${cargo_profile_dir}/deps/integration-stale-decoy
  run_vm_cargo mkdir -p "${target_dir}/${cargo_profile_dir}/deps"
  run_vm_cargo touch -t 203801010000 "${lib_decoy}" "${integration_decoy}"
  run_vm_cargo chmod 0755 "${lib_decoy}" "${integration_decoy}"
  local build_command=(
    timeout --kill-after=30s 20m cargo test
    "${cargo_profile_args[@]}"
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

privileged_test_sequence=0

run_and_verify_privileged_test() {
  local result_file=$1
  local filter=$2
  shift 2

  set +e
  "$@" 2>&1 | tee "${result_file}"
  local status=$?
  set -e
  if [[ ${status} -ne 0 ]]; then
    echo "managed XFS selector '${filter}' command failed with status ${status}" >&2
    return "${status}"
  fi
  require_exactly_one_passed_test "${result_file}" "${filter}"
}

run_privileged_test() {
  local target=$1
  local selected=$2
  local filter=$3
  privileged_test_sequence=$((privileged_test_sequence + 1))
  local result_file=${work_dir}/privileged-test-${privileged_test_sequence}.result

  if [[ ${reuse_test_binaries} == 1 ]]; then
    local target_args=()
    if [[ ${target} == integration ]]; then
      target_args=(--test integration)
    else
      target_args=(--lib)
    fi
    run_and_verify_privileged_test "${result_file}" "${filter}" \
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
      run_and_verify_privileged_test "${result_file}" "${filter}" \
        timeout --kill-after=30s 5m \
        "${capable_binary}" "${filter}" --exact --include-ignored --nocapture --report-time
    )
  else
    run_and_verify_privileged_test "${result_file}" "${filter}" \
      timeout --kill-after=30s 5m \
      "${capable_binary}" "${filter}" --exact --include-ignored --nocapture --report-time
  fi
}

run_filesystem_benchmark() {
  local mode=$1
  local selected=$2
  local filter=wasi::filesystem_guest_latency_benchmark
  local capable_binary=${work_dir}/integration-benchmark-${mode}
  local result_file=${work_dir}/filesystem-benchmark-${mode}.result
  local perf_file=${work_dir}/filesystem-benchmark-${mode}.perf
  local strace_file=${work_dir}/filesystem-benchmark-${mode}.strace
  local strace_result=${work_dir}/filesystem-benchmark-${mode}-strace.result

  cp "${selected}" "${capable_binary}"
  chmod 0755 "${capable_binary}"
  (
    cd "${repo_root}/golem-worker-executor"
    run_and_verify_privileged_test "${result_file}" "${filter}" \
      env GOLEM_FILESYSTEM_BENCH_MODE="${mode}" \
      timeout --kill-after=30s 30m \
      perf stat -x, -o "${perf_file}" \
      -e task-clock,context-switches,cpu-migrations,page-faults \
      -- "${capable_binary}" "${filter}" --exact --include-ignored --nocapture --report-time
  )
  echo "FILESYSTEM_BENCHMARK_PERF mode=${mode}"
  while IFS= read -r line; do
    echo "${line}"
  done < "${perf_file}"

  if command -v strace >/dev/null 2>&1; then
    (
      cd "${repo_root}/golem-worker-executor"
      run_and_verify_privileged_test "${strace_result}" "${filter}" \
        env GOLEM_FILESYSTEM_BENCH_MODE="${mode}" GOLEM_FILESYSTEM_BENCH_QUICK=1 \
        timeout --kill-after=30s 10m \
        strace -f -c -o "${strace_file}" \
        "${capable_binary}" "${filter}" --exact --include-ignored --nocapture --report-time
    )
    echo "FILESYSTEM_BENCHMARK_STRACE mode=${mode}"
    while IFS= read -r line; do
      echo "${line}"
    done < "${strace_file}"
  else
    echo "strace is unavailable; skipping syscall counts for mode=${mode}" >&2
  fi
}

run_filesystem_workload_benchmark() {
  local mode=$1
  local selected=$2
  local filter=services::agent_filesystem::lifecycle::workload_benchmark::filesystem_workload_benchmark
  local capable_binary=${work_dir}/lib-workload-benchmark-${mode}
  local result_file=${work_dir}/filesystem-workload-benchmark-${mode}.result
  local perf_file=${work_dir}/filesystem-workload-benchmark-${mode}.perf
  local single_agent_result=${work_dir}/filesystem-workload-benchmark-${mode}-single-agent.result
  local single_agent_perf=${work_dir}/filesystem-workload-benchmark-${mode}-single-agent.perf
  local strace_file=${work_dir}/filesystem-workload-benchmark-${mode}.strace
  local strace_result=${work_dir}/filesystem-workload-benchmark-${mode}-strace.result

  cp "${selected}" "${capable_binary}"
  chmod 0755 "${capable_binary}"
  run_and_verify_privileged_test "${result_file}" "${filter}" \
    run_filesystem_workload_command \
    GOLEM_FILESYSTEM_BENCH_MODE="${mode}" \
    timeout --kill-after=30s 30m \
    perf stat -x, -o "${perf_file}" \
    -e task-clock,context-switches,cpu-migrations,page-faults \
    -- "${capable_binary}" "${filter}" --exact --include-ignored --nocapture --report-time
  echo "FILESYSTEM_WORKLOAD_BENCHMARK_PERF mode=${mode}"
  while IFS= read -r line; do
    echo "${line}"
  done < "${perf_file}"

  if [[ ${mode} == managed ]]; then
    run_and_verify_privileged_test "${single_agent_result}" "${filter}" \
      run_filesystem_workload_command \
      GOLEM_FILESYSTEM_BENCH_MODE="${mode}" GOLEM_FILESYSTEM_BENCH_SINGLE_AGENT=1 \
      timeout --kill-after=30s 30m \
      perf stat -x, -o "${single_agent_perf}" \
      -e task-clock,context-switches,cpu-migrations,page-faults \
      -- "${capable_binary}" "${filter}" --exact --include-ignored --nocapture --report-time
    verify_context_switch_reduction "${single_agent_perf}" 554590 95
    echo "FILESYSTEM_WORKLOAD_BENCHMARK_SINGLE_AGENT_PERF mode=${mode}"
    while IFS= read -r line; do
      echo "${line}"
    done < "${single_agent_perf}"
  fi

  if command -v strace >/dev/null 2>&1; then
    run_and_verify_privileged_test "${strace_result}" "${filter}" \
      run_filesystem_workload_command \
      GOLEM_FILESYSTEM_BENCH_MODE="${mode}" GOLEM_FILESYSTEM_BENCH_QUICK=1 \
      timeout --kill-after=30s 10m \
      strace -f -c -o "${strace_file}" \
      "${capable_binary}" "${filter}" --exact --include-ignored --nocapture --report-time
    echo "FILESYSTEM_WORKLOAD_BENCHMARK_STRACE mode=${mode}"
    while IFS= read -r line; do
      echo "${line}"
    done < "${strace_file}"
  else
    echo "strace is unavailable; skipping filesystem workload syscall counts for mode=${mode}" >&2
  fi
}

run_filesystem_native_workload_baseline() {
  local selected=$1
  local filter=services::agent_filesystem::lifecycle::workload_benchmark::filesystem_native_workload_baseline
  local capable_binary=${work_dir}/lib-native-workload-baseline
  local result_file=${work_dir}/filesystem-native-workload-baseline.result
  local perf_file=${work_dir}/filesystem-native-workload-baseline.perf

  cp "${selected}" "${capable_binary}"
  chmod 0755 "${capable_binary}"
  run_and_verify_privileged_test "${result_file}" "${filter}" \
    timeout --kill-after=30s 10m \
    perf stat -x, -o "${perf_file}" \
    -e task-clock,context-switches,cpu-migrations,page-faults \
    -- "${capable_binary}" "${filter}" --exact --include-ignored --nocapture --report-time
  echo "FILESYSTEM_NATIVE_WORKLOAD_BASELINE_PERF"
  while IFS= read -r line; do
    echo "${line}"
  done < "${perf_file}"
}

if [[ $# -gt 0 ]]; then
  run_test "$@"
  exit
fi

if [[ ${filesystem_workload_benchmark} == false && ${filesystem_native_workload_baseline} == false ]]; then
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
fi

lib_test_binary=
integration_test_binary=
if [[ ${reuse_test_binaries} == 0 ]]; then
  build_test_binaries
fi

if [[ ${filesystem_benchmark} == true || ${filesystem_workload_benchmark} == true || ${filesystem_native_workload_baseline} == true ]]; then
  if ! command -v perf >/dev/null 2>&1; then
    echo "missing required command for filesystem benchmark: perf" >&2
    exit 1
  fi
  if [[ ${reuse_test_binaries} == 1 ]]; then
    echo "filesystem benchmark does not support GOLEM_MANAGED_XFS_REUSE_TEST_BINARIES=1" >&2
    exit 1
  fi
  if [[ ${filesystem_benchmark} == true || ${filesystem_workload_benchmark} == true ]]; then
    benchmark_modes=(managed managed-unmetered unmanaged)
    if [[ ${reverse_benchmark_modes} == 1 ]]; then
      benchmark_modes=(unmanaged managed-unmetered managed)
    fi
  fi
  if [[ ${filesystem_benchmark} == true ]]; then
    for mode in "${benchmark_modes[@]}"; do
      run_filesystem_benchmark "${mode}" "${integration_test_binary}"
    done
  fi
  if [[ ${filesystem_benchmark} == true || ${filesystem_workload_benchmark} == true ]]; then
    for mode in "${benchmark_modes[@]}"; do
      run_filesystem_workload_benchmark "${mode}" "${lib_test_binary}"
    done
  fi
  run_filesystem_native_workload_baseline "${lib_test_binary}"
  exit
fi

run_privileged_test \
  lib \
  "${lib_test_binary}" \
  sandbox_filesystem::xfs::tests::managed_xfs_sandbox_filesystem_owns_allocation_limits_and_cleanup

run_privileged_test \
  lib \
  "${lib_test_binary}" \
  services::agent_filesystem::lifecycle::tests::managed_xfs_allocated_bytes_flow_through_resource_billing

run_privileged_test \
  integration \
  "${integration_test_binary}" \
  wasi::p2_p3_quota_classification_on_managed_xfs

run_privileged_test \
  integration \
  "${integration_test_binary}" \
  wasi::managed_xfs_physical_pressure_unloads_loaded_idle_and_retries_safe_write

run_privileged_test \
  integration \
  "${integration_test_binary}" \
  wasi::managed_xfs_resource_billing_survives_idle_and_replay

run_privileged_test \
  integration \
  "${integration_test_binary}" \
  wasi::filesystem_downgrade_blocks_guest_until_limit_recovers
