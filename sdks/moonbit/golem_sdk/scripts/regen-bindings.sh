#!/usr/bin/env bash
#
# Regenerate the WIT bindings for the Golem MoonBit SDK and apply the required
# post-processing fixes for stock `wit-bindgen` (no fork needed).
#
# Run from the `golem_sdk` module root:
#   bash scripts/regen-bindings.sh
#
# Steps:
#   1. Run stock `wit-bindgen moonbit` (--ignore-stub keeps hand-maintained stubs).
#   2. Fix the stock-bindgen s8/s16 lifting bug: the generated code reads signed
#      bytes/shorts with `mbt_ffi_load8`/`mbt_ffi_load16` (i32.load8_s/load16_s)
#      AND then subtracts 0x100/0x10000 — a double sign-extension that corrupts
#      every s8/s16 value lifted from the component ABI. We strip the spurious
#      subtraction (the signed load alone already yields the correct value).
#   3. Remove the `moon.pkg.json` files emitted by wit-bindgen: this repo tracks
#      hand-maintained plain `moon.pkg` files, and `moon` warns (and ignores the
#      json) when both exist.
#   4. Assert the s8/s16 bug is gone.
#
set -euo pipefail

cd "$(dirname "$0")/.."

echo "==> Running wit-bindgen (stock)"
wit-bindgen moonbit ./wit \
  --derive-show --derive-eq --derive-error \
  --project-name golemcloud/golem_sdk --ignore-stub

echo "==> Fixing stock-bindgen s8/s16 double sign-extension"
# Only strip the subtraction where it follows the matching SIGNED load, so the
# unsigned `mbt_ffi_load8_u`/`mbt_ffi_load16_u` paths are never touched.
# `\b` after `load8`/`load16` ensures `load8_u`/`load16_u` are excluded, and the
# `0x10000` rule runs before `0x100` to avoid corrupting the wider literal.
find interface world gen -name '*.mbt' -type f -print0 | while IFS= read -r -d '' f; do
  perl -i -pe 's/(mbt_ffi_load16\b[^\n]*?\)) - 0x10000\b/$1/g; s/(mbt_ffi_load8\b[^\n]*?\)) - 0x100\b/$1/g' "$f"
done

echo "==> Removing wit-bindgen-emitted moon.pkg.json (tracked moon.pkg is source of truth)"
find interface world gen -name 'moon.pkg.json' -type f -delete

echo "==> Verifying s8/s16 fix"
if rg -n -g '*.mbt' -e ' - 0x100\b' -e ' - 0x10000\b' interface world gen >/dev/null 2>&1; then
  echo "ERROR: residual s8/s16 double sign-extension found after post-processing:" >&2
  rg -n -g '*.mbt' -e ' - 0x100\b' -e ' - 0x10000\b' interface world gen >&2
  exit 1
fi

echo "==> Bindings regenerated and post-processed successfully"
