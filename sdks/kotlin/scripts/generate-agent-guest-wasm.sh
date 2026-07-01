#!/usr/bin/env bash
set -euo pipefail

# Generates the generic QuickJS-based `agent_guest.wasm` guest runtime for Kotlin/JS agents.
#
# Mirrors sdks/scala/scripts/generate-agent-guest-wasm.sh. The runtime has ONLY a `user=@slot`
# for the injected agent JS — the SDK is NOT embedded as a separate module; Kotlin/JS bundles
# the SDK into the user's agent bundle (which golem-cli injects later). The wasm is therefore
# SDK-version-independent and only needs regenerating when the Golem WIT surface changes.
#
# Prerequisites: WIT deps synced (`cargo make wit` from repo root populates sibling SDK wit/);
#                `wasm-rquickjs` (crate `wasm-rquickjs-cli`); Rust + wasm32-wasip2 target.
#
# Usage:  cd sdks/kotlin && ./scripts/generate-agent-guest-wasm.sh

sdk_root="$(cd "$(dirname "$0")/.." && pwd)"

if ! command -v wasm-rquickjs &>/dev/null; then
  echo "[agent-guest] ERROR: wasm-rquickjs not found. Install: cargo install wasm-rquickjs-cli" >&2
  exit 1
fi

wit_dir="$sdk_root/wit"
gen_dir="$sdk_root/.generated"
agent_wit_root="$gen_dir/agent-wit-root"
wrapper_dir="$gen_dir/agent-guest-wrapper"
out_wasm="$wrapper_dir/target/wasm32-wasip2/release/agent_guest.wasm"

[[ -f "$wit_dir/main.wit" ]] || { echo "[agent-guest] ERROR: missing $wit_dir/main.wit" >&2; exit 1; }
[[ -d "$wit_dir/deps"     ]] || { echo "[agent-guest] ERROR: missing $wit_dir/deps (run 'cargo make wit')" >&2; exit 1; }

echo "[agent-guest] Staging WIT package..." >&2
rm -rf "$agent_wit_root"; mkdir -p "$agent_wit_root/deps"
cp "$wit_dir/main.wit" "$agent_wit_root/main.wit"
for dep in "$wit_dir"/deps/*/; do cp -r "$dep" "$agent_wit_root/deps/$(basename "$dep")"; done

echo "[agent-guest] Generating wrapper crate (slot-only)..." >&2
rm -rf "$wrapper_dir"
wasm-rquickjs generate-wrapper-crate \
  --wit "$agent_wit_root" \
  --world kotlin-agent-guest \
  --js-modules "user=@slot" \
  --output "$wrapper_dir"

# Default features ('normal' = fetch/node-http/crypto/zlib/logging/encoding). We do NOT use
# 'full' (pulls sqlite3-sys, which needs a wasi clang sysroot) nor 'golem' (golem-rust pins an
# older golem:api WIT). The counter / agent dispatch does not need them.
echo "[agent-guest] Building guest runtime (cargo component build --target wasm32-wasip2 --release)..." >&2
[[ -f "$HOME/.cargo/env" ]] && . "$HOME/.cargo/env"
( cd "$wrapper_dir" && env -u ARGV0 cargo component build --target wasm32-wasip2 --release )

out_wasm="$(find "$wrapper_dir/target/wasm32-wasip2/release" -maxdepth 1 -name '*.wasm' -not -path '*/deps/*' | head -1)"
[[ -n "$out_wasm" && -f "$out_wasm" ]] || { echo "[agent-guest] ERROR: build did not produce a .wasm" >&2; exit 1; }
echo "[agent-guest] Built: $out_wasm" >&2

echo "[agent-guest] Validating..." >&2
wasm-tools validate "$out_wasm" --features component-model

echo "[agent-guest] Installing into the Gradle plugin's embedded resources..." >&2
res="$sdk_root/gradle-plugin/src/main/resources/golem/wasm"
mkdir -p "$res"
install -m 0644 "$out_wasm" "$res/agent_guest.wasm"
install -m 0644 "$out_wasm" "$gen_dir/agent_guest.wasm"
echo "[agent-guest] Done -> $gen_dir/agent_guest.wasm (+ plugin resources)." >&2
