#!/usr/bin/env bash

set -euo pipefail

readonly WIT_BINDGEN_REPOSITORY="https://github.com/golemcloud/wit-bindgen"
readonly WIT_BINDGEN_REVISION="67b83d42c135db8c5c42bf26c53ec162bbdca5ea"
SDK_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
readonly SDK_ROOT

cargo install --locked \
  --git "$WIT_BINDGEN_REPOSITORY" \
  --rev "$WIT_BINDGEN_REVISION" \
  wit-bindgen-cli

wit_bindgen_version="$(wit-bindgen --version)"
if [[ ! "$wit_bindgen_version" =~ 0\.63\.0\ \([0-9a-f]{9,40}\ [0-9]{4}-[0-9]{2}-[0-9]{2}\) ]]; then
  echo "ERROR: installed wit-bindgen did not report the expected Git version: $wit_bindgen_version" >&2
  exit 1
fi
printf '%s\n' "$wit_bindgen_version" >"$SDK_ROOT/.wit-bindgen-version"
