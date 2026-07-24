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
#   1. Run the pinned `wit-bindgen moonbit` (--ignore-stub keeps hand-maintained
#      stubs).
#   2. Fix the s8/s16 lifting bug: the generated code reads signed
#      bytes/shorts with `mbt_ffi_load8`/`mbt_ffi_load16` (i32.load8_s/load16_s)
#      AND then subtracts 0x100/0x10000 — a double sign-extension that corrupts
#      every s8/s16 value lifted from the component ABI. We strip the spurious
#      subtraction (the signed load alone already yields the correct value).
#   3. Remove an emitted `moon.pkg.json` only where a sibling hand-maintained
#      `moon.pkg` owns package metadata (the export stubs and gen link package).
#   4. Assert the s8/s16 bug is gone.
#
set -euo pipefail

cd "$(dirname "$0")/.."

readonly WIT_BINDGEN_COMMIT="4407232ead86d9bcbd06cbebd790a52120a4087a"
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

echo "==> Running pinned Golem wit-bindgen fork"
wit-bindgen moonbit ./wit \
  --derive-show --derive-eq --derive-error \
  --project-name golemcloud/golem_sdk --ignore-stub

echo "==> Fixing s8/s16 double sign-extension"
# Only strip the subtraction where it follows the matching SIGNED load, so the
# unsigned `mbt_ffi_load8_u`/`mbt_ffi_load16_u` paths are never touched.
# `\b` after `load8`/`load16` ensures `load8_u`/`load16_u` are excluded, and the
# `0x10000` rule runs before `0x100` to avoid corrupting the wider literal.
find interface world gen -name '*.mbt' -type f -print0 | while IFS= read -r -d '' f; do
  perl -i -pe 's/(mbt_ffi_load16\b[^\n]*?\)) - 0x10000\b/$1/g; s/(mbt_ffi_load8\b[^\n]*?\)) - 0x100\b/$1/g' "$f"
done

echo "==> Removing generated package descriptors shadowed by hand-maintained moon.pkg files"
find interface world gen async-core -name 'moon.pkg.json' -type f -print0 |
  while IFS= read -r -d '' package; do
    if [[ -f "$(dirname "$package")/moon.pkg" ]]; then
      rm "$package"
    fi
  done

echo "==> Verifying s8/s16 fix"
if rg -n -g '*.mbt' -e ' - 0x100\b' -e ' - 0x10000\b' interface world gen >/dev/null 2>&1; then
  echo "ERROR: residual s8/s16 double sign-extension found after post-processing:" >&2
  rg -n -g '*.mbt' -e ' - 0x100\b' -e ' - 0x10000\b' interface world gen >&2
  exit 1
fi

echo "==> Bindings regenerated and post-processed successfully"
