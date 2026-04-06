#!/usr/bin/env bash
set -euo pipefail

# Generates a QuickJS-based `agent_guest.wasm` (guest runtime) for Scala.js-style agents.
#
# Why this exists:
# - The guest runtime is version-sensitive to the Golem server/CLI WIT surface.
# - When upgrading Golem, regenerating the guest runtime avoids mysterious linker/discovery failures.
#
# This script:
# 1) stages a WIT package for `golem:agent-guest` (using wit/main.wit + wit/deps/)
# 2) runs `wasm-rquickjs generate-wrapper-crate` with a `@slot` for user JS injection.
#    Unlike the TS SDK, we do NOT embed a separate SDK JS module here.
#    Scala.js bundles the SDK into the user's `scala.js`, which golem-cli injects later.
# 3) builds the component with `cargo build --target wasm32-wasip2`
# 4) updates embedded plugin resources (used by sbt/mill plugins).
#
# Prerequisites:
# - WIT deps must be synced first: `cargo make wit` from the repo root
#
# Requirements:
# - `wasm-rquickjs` (from crate `wasm-rquickjs-cli`)
# - Rust toolchain with `wasm32-wasip2` target (`rustup target add wasm32-wasip2`)
#
# Usage:
#   cd sdks/scala && ./scripts/generate-agent-guest-wasm.sh
#

# sdk_root is sdks/scala
sdk_root="$(cd "$(dirname "$0")/.." && pwd)"

if ! command -v wasm-rquickjs &>/dev/null; then
  echo "[agent-guest] ERROR: wasm-rquickjs not found. Install it with: cargo install wasm-rquickjs-cli" >&2
  exit 1
fi

wit_dir="$sdk_root/wit"
gen_dir="$sdk_root/.generated"
agent_wit_root="$gen_dir/agent-wit-root"
wrapper_dir="$gen_dir/agent-guest-wrapper"
out_wasm="$wrapper_dir/target/wasm32-wasip2/release/agent_guest.wasm"

echo "[agent-guest] sdk_root=$sdk_root" >&2

mkdir -p "$gen_dir"

if [[ ! -f "$wit_dir/main.wit" ]]; then
  echo "[agent-guest] ERROR: missing WIT definition at $wit_dir/main.wit" >&2
  exit 1
fi

if [[ ! -d "$wit_dir/deps" ]]; then
  echo "[agent-guest] ERROR: missing WIT dependencies at $wit_dir/deps/" >&2
  echo "[agent-guest]   Run 'cargo make wit' from the repository root first." >&2
  exit 1
fi

echo "[agent-guest] Staging WIT package for golem:agent-guest..." >&2
rm -rf "$agent_wit_root"
mkdir -p "$agent_wit_root"

cp "$wit_dir/main.wit" "$agent_wit_root/main.wit"
mkdir -p "$agent_wit_root/deps"
for dep in "$wit_dir"/deps/*/; do
  dep_name="$(basename "$dep")"
  cp -r "$dep" "$agent_wit_root/deps/$dep_name"
done

dts_dir="$gen_dir/agent-guest-dts"
echo "[agent-guest] Generating TypeScript d.ts definitions..." >&2
rm -rf "$dts_dir"
wasm-rquickjs generate-dts \
  --wit "$agent_wit_root" \
  --world golem:agent-guest/agent-guest \
  --output "$dts_dir"
echo "[agent-guest] TypeScript definitions written to $dts_dir" >&2
ls -1 "$dts_dir"/*.d.ts 2>/dev/null | while read -r f; do echo "  $(basename "$f")"; done >&2

echo "[agent-guest] Generating wrapper crate with wasm-rquickjs..." >&2
rm -rf "$wrapper_dir"
wasm-rquickjs generate-wrapper-crate \
  --wit "$agent_wit_root" \
  --world golem:agent-guest/agent-guest \
  --js-modules "user=@slot" \
  --output "$wrapper_dir"

echo "[agent-guest] Building guest runtime (cargo build --target wasm32-wasip2 --release)..." >&2
if [[ -f "$HOME/.cargo/env" ]]; then
  # shellcheck disable=SC1090
  . "$HOME/.cargo/env"
fi

( cd "$wrapper_dir" && env -u ARGV0 rustup run stable cargo build --target wasm32-wasip2 --release --features full,golem )

if [[ ! -f "$out_wasm" ]]; then
  echo "[agent-guest] ERROR: build did not produce $out_wasm" >&2
  exit 1
fi

echo "[agent-guest] Built: $out_wasm" >&2
sha256sum "$out_wasm" 2>/dev/null || shasum -a 256 "$out_wasm" >&2

echo "[agent-guest] Installing into plugin embedded resources..." >&2
mkdir -p "$sdk_root/sbt/src/main/resources/golem/wasm"
mkdir -p "$sdk_root/mill/resources/golem/wasm"
install -m 0644 "$out_wasm" "$sdk_root/sbt/src/main/resources/golem/wasm/agent_guest.wasm"
install -m 0644 "$out_wasm" "$sdk_root/mill/resources/golem/wasm/agent_guest.wasm"

echo "[agent-guest] Copying TypeScript d.ts definitions to wit/dts/..." >&2
rm -rf "$sdk_root/wit/dts"
cp -r "$dts_dir" "$sdk_root/wit/dts"

echo "[agent-guest] Done." >&2
