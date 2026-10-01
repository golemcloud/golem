#!/usr/bin/env bash

set -euo pipefail

cd "$(dirname "$0")/.."

readonly GENERATED_ROOTS=(
  interface
  async-core
  world
  gen
)

scripts/check-wit-bindgen-version.sh
scripts/check-wit-bindgen-fork-version.sh
scripts/regen-bindings.sh

tmp_root="$(mktemp -d "${TMPDIR:-/tmp}/golem-moonbit-bindings-check.XXXXXX")"
trap 'rm -rf "$tmp_root"' EXIT

for root in "${GENERATED_ROOTS[@]}"; do
  cp -R "$root" "$tmp_root/$root"
done

scripts/regen-bindings.sh

for root in "${GENERATED_ROOTS[@]}"; do
  if ! diff -qr "$tmp_root/$root" "$root"; then
    echo "ERROR: MoonBit WIT binding generation is not deterministic for $root" >&2
    exit 1
  fi
done

echo "MoonBit WIT binding generation is deterministic"
