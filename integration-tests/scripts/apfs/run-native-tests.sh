#!/usr/bin/env bash
set -euo pipefail

if [[ $# -eq 0 ]]; then
  echo "Usage: $0 <native test executable> [test arguments ...]" >&2
  exit 1
fi
if [[ $(uname -s) != Darwin ]]; then
  echo "The native APFS suite requires macOS" >&2
  exit 1
fi

fixture=$(mktemp -d "${TMPDIR:-/tmp}/golem-apfs-tests.XXXXXX")
mount_point="${fixture}/non-cloning-volume"
attach_attempted=false

cleanup() {
  local result=$?
  trap - EXIT
  if [[ ${attach_attempted} == true ]]; then
    if ! hdiutil detach "${mount_point}" > "${fixture}/detach.log" 2>&1; then
      echo "Could not detach the fixture; retained ${fixture}" >&2
      while IFS= read -r line; do printf '%s\n' "${line}" >&2; done < "${fixture}/detach.log"
      if [[ ${result} -eq 0 ]]; then result=1; fi
      exit "${result}"
    fi
  fi
  rm -rf "${fixture}"
  exit "${result}"
}
trap cleanup EXIT

if ! hdiutil create -size 32m -fs HFS+ -type SPARSE -volname golem-apfs-refusal \
  "${fixture}/non-cloning-volume.sparseimage" > "${fixture}/create.log" 2>&1; then
  while IFS= read -r line; do printf '%s\n' "${line}" >&2; done < "${fixture}/create.log"
  exit 1
fi
mkdir "${mount_point}"
attach_attempted=true
if ! hdiutil attach "${fixture}/non-cloning-volume.sparseimage" -nobrowse -owners on \
  -mountpoint "${mount_point}" > "${fixture}/attach.log" 2>&1; then
  while IFS= read -r line; do printf '%s\n' "${line}" >&2; done < "${fixture}/attach.log"
  exit 1
fi

set +e
GOLEM_APFS_NON_CLONE_ROOT="${mount_point}" "$@" 2>&1 | tee "${fixture}/tests.log"
result=${PIPESTATUS[0]}
set -e
if [[ ${result} -ne 0 ]]; then exit "${result}"; fi
if ! grep -Eq 'test result: ok; [1-9][0-9]* passed;' "${fixture}/tests.log"; then
  echo "The native APFS suite did not report an executed test" >&2
  exit 1
fi
