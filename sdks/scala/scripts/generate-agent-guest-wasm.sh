#!/usr/bin/env bash
set -euo pipefail

# Generates the QuickJS-based guest runtimes for every Scala component role.
#
# Why this exists:
# - The guest runtime is version-sensitive to the Golem server/CLI WIT surface.
# - When upgrading Golem, regenerating the guest runtime avoids mysterious linker/discovery failures.
#
# This script:
# 1) stages a WIT package for each role (using wit/main.wit + wit/deps/)
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

wit_bindgen_line='wit-bindgen-p3 = { package = "wit-bindgen", version = "0.58.0", default-features = false, features = ["async", "async-spawn", "macros", "inter-task-wakeup"], optional = true }'
forked_line='wit-bindgen-p3 = { package = "wit-bindgen", git = "https://github.com/golemcloud/wit-bindgen", rev = "4407232ead86d9bcbd06cbebd790a52120a4087a", version = "=0.59.0", default-features = false, features = ["async", "async-spawn", "macros", "inter-task-wakeup"], optional = true }'

if [[ -f "$HOME/.cargo/env" ]]; then
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi

mkdir -p "$sdk_root/sbt/src/main/resources/golem/wasm"
mkdir -p "$sdk_root/mill/resources/golem/wasm"

target_dir="${CARGO_TARGET_DIR:-$gen_dir/agent-guest-target}"
if [[ "$target_dir" != /* ]]; then
  mkdir -p "$sdk_root/$target_dir"
  target_dir="$(cd "$sdk_root/$target_dir" && pwd)"
else
  mkdir -p "$target_dir"
fi

# role|world|artifact
roles=(
  "agent-guest|golem:agent-guest/agent-guest|agent_guest.wasm"
  "tool-middleware-guest|golem:agent-guest/tool-middleware-guest|tool_middleware_guest.wasm"
  "agent-tool-middleware-guest|golem:agent-guest/agent-tool-middleware-guest|agent_tool_middleware_guest.wasm"
)

for role_spec in "${roles[@]}"; do
  IFS='|' read -r role world artifact <<<"$role_spec"
  wit_root="$gen_dir/$role-wit-root"
  dts_dir="$gen_dir/$role-dts"
  wrapper_dir="$gen_dir/$role-wrapper"

  echo "[agent-guest] Staging WIT package for $world..." >&2
  rm -rf "$wit_root" "$dts_dir" "$wrapper_dir"
  mkdir -p "$wit_root/deps"
  cp "$wit_dir/main.wit" "$wit_root/main.wit"
  for dep in "$wit_dir"/deps/*/; do
    dep_name="$(basename "$dep")"
    cp -r "$dep" "$wit_root/deps/$dep_name"
  done

  echo "[agent-guest] Generating TypeScript d.ts definitions for $role..." >&2
  wasm-rquickjs generate-dts \
    --wit "$wit_root" \
    --world "$world" \
    --target wasi-p3 \
    --output "$dts_dir"

  shopt -s nullglob
  dts_files=("$dts_dir"/*.d.ts)
  shopt -u nullglob
  if (( ${#dts_files[@]} == 0 )); then
    echo "[agent-guest] ERROR: declaration generation for $role produced no .d.ts files" >&2
    exit 1
  fi
  for f in "${dts_files[@]}"; do echo "  $(basename "$f")"; done >&2

  echo "[agent-guest] Generating $role wrapper crate with wasm-rquickjs..." >&2
  wasm-rquickjs generate-wrapper-crate \
    --wit "$wit_root" \
    --world "$world" \
    --target wasi-p3 \
    --js-modules "user=@slot" \
    --output "$wrapper_dir"

  # wasm-rquickjs has no dependency override for its generated skeleton, so
  # point Preview 3 bindings at Golem's outline-lift fork for every role.
  cargo_toml="$wrapper_dir/Cargo.toml"
  if [[ "$(grep -cF -- "$wit_bindgen_line" "$cargo_toml")" != "1" ]]; then
    echo "[agent-guest] ERROR: expected exactly one wit-bindgen dependency line in $cargo_toml" >&2
    echo "[agent-guest]   The wasm-rquickjs skeleton may have changed; update this script." >&2
    exit 1
  fi
  WB_LINE="$wit_bindgen_line" FORK_LINE="$forked_line" \
    perl -ni -e '
      chomp(my $chomped = $_);
      if ($chomped eq $ENV{WB_LINE}) { print "$ENV{FORK_LINE}\n"; next; }
      print;
    ' "$cargo_toml"
  if ! grep -qF -- "$forked_line" "$cargo_toml" || grep -qF -- "$wit_bindgen_line" "$cargo_toml"; then
    echo "[agent-guest] ERROR: failed to rewrite wit-bindgen dependency in $cargo_toml" >&2
    exit 1
  fi

  # The generated lock still pins the replaced upstream dependency graph.
  rm -f "$wrapper_dir/Cargo.lock"

  out_wasm="$target_dir/wasm32-wasip2/release/$artifact"
  rm -f "$out_wasm"

  echo "[agent-guest] Building $role guest runtime..." >&2
  (
    cd "$wrapper_dir"
    env -u ARGV0 rustup run stable cargo build \
      --target wasm32-wasip2 \
      --target-dir "$target_dir" \
      --release \
      --no-default-features \
      --features full-p3,golem
  )
  if [[ ! -f "$out_wasm" ]]; then
    echo "[agent-guest] ERROR: build did not produce $out_wasm" >&2
    exit 1
  fi

  echo "[agent-guest] Built: $out_wasm" >&2
  sha256sum "$out_wasm" 2>/dev/null || shasum -a 256 "$out_wasm" >&2
  install -m 0644 "$out_wasm" "$sdk_root/sbt/src/main/resources/golem/wasm/$artifact"
  install -m 0644 "$out_wasm" "$sdk_root/mill/resources/golem/wasm/$artifact"

  if [[ "$role" == "agent-guest" ]]; then
    echo "[agent-guest] Copying ordinary TypeScript d.ts definitions to wit/dts/..." >&2
    rm -rf "$sdk_root/wit/dts"
    cp -r "$dts_dir" "$sdk_root/wit/dts"
  fi
done

"$sdk_root/scripts/test-agent-guest-export-contract.sh"

echo "[agent-guest] Done." >&2
