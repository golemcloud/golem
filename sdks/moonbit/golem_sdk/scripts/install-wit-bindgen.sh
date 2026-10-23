#!/usr/bin/env bash

set -euo pipefail

readonly WIT_BINDGEN_REPOSITORY="https://github.com/golemcloud/wit-bindgen"
readonly WIT_BINDGEN_BRANCH="wit-bindgen-golem-1.6"
readonly SDK_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
readonly WIT_BINDGEN_REV="${WIT_BINDGEN_REV:-$(git ls-remote --exit-code "$WIT_BINDGEN_REPOSITORY" "refs/heads/$WIT_BINDGEN_BRANCH" | cut -f1)}"

[[ "$WIT_BINDGEN_REV" =~ ^[0-9a-f]{40}$ ]] || { echo 'ERROR: invalid wit-bindgen source revision' >&2; exit 1; }
if [[ "${1:-}" == '--resolve-revision' ]]; then
  printf '%s\n' "$WIT_BINDGEN_REV"
  exit 0
fi

cargo install --locked \
  --git "$WIT_BINDGEN_REPOSITORY" \
  --rev "$WIT_BINDGEN_REV" \
  wit-bindgen-cli

wit_bindgen_version="$(wit-bindgen --version)"
if [[ ! "$wit_bindgen_version" =~ 0\.59\.0\ \([0-9a-f]{9,40}\ [0-9]{4}-[0-9]{2}-[0-9]{2}\) ]]; then
  echo "ERROR: installed wit-bindgen did not report the expected Git version: $wit_bindgen_version" >&2
  exit 1
fi
printf '%s\n' "$wit_bindgen_version" >"$SDK_ROOT/.wit-bindgen-version"
