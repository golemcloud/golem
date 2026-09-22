#!/usr/bin/env bash
#
# Regenerate the WIT bindings for the Golem MoonBit SDK and apply the required
# post-processing fixes. Regeneration requires the pinned Golem wit-bindgen fork
# documented in ../AGENTS.md; stock wit-bindgen does not support this SDK's P3
# async WIT surface.
#
# Run from the `golem_sdk` module root:
#   bash scripts/regen-bindings.sh
#
# Steps:
#   1. Generate the default agent/tool/middleware world.
#   2. Preserve hand-maintained stubs and package descriptors.
#   3. Split the oversized generated middleware argument lift whose unsplit
#      shape currently triggers a MoonBit compiler ICE.
#   4. Stabilize generated FFI helper and export-wrapper ordering, which the
#      pinned generator currently emits from unordered collections.
#   5. Fix the s8/s16 lifting bug: the generated code reads signed
#      bytes/shorts with `mbt_ffi_load8`/`mbt_ffi_load16` (i32.load8_s/load16_s)
#      AND then subtracts 0x100/0x10000 — a double sign-extension that corrupts
#      every s8/s16 value lifted from the component ABI. We strip the spurious
#      subtraction (the signed load alone already yields the correct value).
#   6. Remove an emitted `moon.pkg.json` only where a sibling hand-maintained
#      `moon.pkg` owns package metadata (the export stubs and gen link package).
#   7. Regenerate package interfaces and assert normalization and the s8/s16 fix.
#
set -euo pipefail

cd "$(dirname "$0")/.."

readonly WIT_BINDGEN_COMMIT="9c6dce16db01eeb83f44360f27c0c502d3da4462"
readonly WIT_BINDGEN_SHORT_COMMIT="${WIT_BINDGEN_COMMIT:0:9}"

wit_bindgen_version="$(wit-bindgen --version)"
if [[ "$wit_bindgen_version" != *"$WIT_BINDGEN_SHORT_COMMIT"* ]]; then
  cat >&2 <<EOF
ERROR: bindings must be regenerated with Golem's pinned wit-bindgen fork at
$WIT_BINDGEN_COMMIT, but found: $wit_bindgen_version

Install the pinned generator with:
  cargo install --locked --git https://github.com/golemcloud/wit-bindgen \\
    --rev $WIT_BINDGEN_COMMIT wit-bindgen-cli
EOF
  exit 1
fi

readonly GENERATED_ROOTS=(
  interface
  async-core
  world
  gen
)

tmp_root="$(mktemp -d "${TMPDIR:-/tmp}/golem-moonbit-bindings.XXXXXX")"
trap 'rm -rf "$tmp_root"' EXIT
readonly preserved_root="$tmp_root/preserved"

echo "==> Preserving hand-maintained generated-package files"
for root in "${GENERATED_ROOTS[@]}"; do
  if [[ ! -d "$root" ]]; then
    continue
  fi
  find "$root" -type f \
    \( -name 'moon.pkg' -o -name 'stub.mbt' -o -name 'stream_transfer.mbt' -o -name '*_test.mbt' -o -name '*_wbtest.mbt' \) \
    -print0 |
    while IFS= read -r -d '' file; do
      destination="$preserved_root/$file"
      mkdir -p "$(dirname "$destination")"
      cp "$file" "$destination"
    done
done

generate_world() {
  local output_name="$1"
  local world_name="$2"
  local gen_dir="$3"
  local output_dir="$tmp_root/$output_name"

  echo "==> Generating $world_name"
  wit-bindgen moonbit "$PWD/wit" \
    --world "$world_name" \
    --gen-dir "$gen_dir" \
    --derive-debug --derive-eq --derive-error \
    --project-name golemcloud/golem_sdk \
    --out-dir "$output_dir"
}

generate_world default agent-guest gen

echo "==> Validating middleware argument lifts"
python3 scripts/split-middleware-lift.py \
  "$tmp_root/default/gen/interface/golem/tool/tool-middleware-guest/ffi.mbt"

echo "==> Assembling generated roots"
rm -rf "${GENERATED_ROOTS[@]}" gen-tool-middleware gen-agent-tool-middleware \
  world/tool-middleware-guest world/agent-tool-middleware-guest
cp -R "$tmp_root/default/interface" interface
cp -R "$tmp_root/default/async-core" async-core
mkdir -p world
cp -R "$tmp_root/default/world/agent-guest" world/
cp -R "$tmp_root/default/gen" gen

if [[ -d "$preserved_root" ]]; then
  find "$preserved_root" -type f -print0 |
    while IFS= read -r -d '' file; do
      relative="${file#"$preserved_root/"}"
      mkdir -p "$(dirname "$relative")"
      cp "$file" "$relative"
    done
fi

rm -rf world/tool-middleware-guest world/agent-tool-middleware-guest

echo "==> Stabilizing generated FFI declaration order"
python3 scripts/normalize-generated-ffi.py "${GENERATED_ROOTS[@]}"

echo "==> Fixing s8/s16 double sign-extension"
# Only strip the subtraction where it follows the matching SIGNED load, so the
# unsigned `mbt_ffi_load8_u`/`mbt_ffi_load16_u` paths are never touched.
# `\b` after `load8`/`load16` ensures `load8_u`/`load16_u` are excluded, and the
# `0x10000` rule runs before `0x100` to avoid corrupting the wider literal.
find "${GENERATED_ROOTS[@]}" -name '*.mbt' -type f -print0 |
  while IFS= read -r -d '' f; do
  perl -i -pe 's/(mbt_ffi_load16\b[^\n]*?\)) - 0x10000\b/$1/g; s/(mbt_ffi_load8\b[^\n]*?\)) - 0x100\b/$1/g' "$f"
  perl -i -0pe 's/\n+\z/\n/' "$f"
done

echo "==> Removing generated package descriptors shadowed by hand-maintained moon.pkg files"
find "${GENERATED_ROOTS[@]}" -name 'moon.pkg.json' -type f -print0 |
  while IFS= read -r -d '' package; do
    if [[ -f "$(dirname "$package")/moon.pkg" ]]; then
      rm "$package"
    fi
  done

echo "==> Regenerating generated package interfaces"
generated_packages=()
while IFS= read -r -d '' descriptor; do
  generated_packages+=("$(dirname "$descriptor")")
done < <(
  find "${GENERATED_ROOTS[@]}" -type f \
    \( -name 'moon.pkg' -o -name 'moon.pkg.json' \) -print0
)
moon info --target wasm "${generated_packages[@]}"

echo "==> Verifying generated FFI declaration order"
python3 scripts/normalize-generated-ffi.py --check "${GENERATED_ROOTS[@]}"

echo "==> Verifying s8/s16 fix"
if rg -n -g '*.mbt' -e ' - 0x100\b' -e ' - 0x10000\b' \
  "${GENERATED_ROOTS[@]}" >/dev/null 2>&1; then
  echo "ERROR: residual s8/s16 double sign-extension found after post-processing:" >&2
  rg -n -g '*.mbt' -e ' - 0x100\b' -e ' - 0x10000\b' \
    "${GENERATED_ROOTS[@]}" >&2
  exit 1
fi

echo "==> Bindings regenerated and post-processed successfully"
