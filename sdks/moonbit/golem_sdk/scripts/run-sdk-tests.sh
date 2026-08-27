#!/usr/bin/env bash

set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

if [[ ${GOLEM_MOONRUN_DISPATCH:-} == 1 ]]; then
  artifact=""
  for arg in "$@"; do
    if [[ $arg == *.wasm ]]; then
      artifact=$arg
    fi
  done

  if [[ -z $artifact ]]; then
    echo "Golem MoonBit test runner did not receive a WASM artifact" >&2
    exit 2
  fi

  if grep -aFq 'golem:core/types@2.0.0' "$artifact"; then
    exec node "$script_dir/run-wasm-test.mjs" "$artifact"
  fi

  unset GOLEM_MOONRUN_DISPATCH MOONRUN_OVERRIDE
  exec "$GOLEM_REAL_MOONRUN" "$@"
fi

real_moonrun=$(command -v moonrun)
GOLEM_MOONRUN_DISPATCH=1 \
  GOLEM_REAL_MOONRUN=$real_moonrun \
  MOONRUN_OVERRIDE=$script_dir/run-sdk-tests.sh \
  moon test --target wasm "$@"
