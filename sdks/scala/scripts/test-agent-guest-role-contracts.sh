#!/usr/bin/env bash
set -euo pipefail

sdk_root="$(cd "$(dirname "$0")/.." && pwd)"
sbt_wasm_dir="$sdk_root/sbt/src/main/resources/golem/wasm"
mill_wasm_dir="$sdk_root/mill/resources/golem/wasm"

if ! command -v wasm-tools &>/dev/null; then
  echo "ERROR: wasm-tools is required to inspect guest role contracts" >&2
  exit 1
fi

assert_has() {
  local wit="$1"
  local expected="$2"
  if ! grep -qF -- "$expected" <<<"$wit"; then
    echo "FAIL: expected component WIT to contain: $expected" >&2
    return 1
  fi
}

assert_lacks() {
  local wit="$1"
  local unexpected="$2"
  if grep -qF -- "$unexpected" <<<"$wit"; then
    echo "FAIL: expected component WIT not to contain: $unexpected" >&2
    return 1
  fi
}

for artifact in agent_guest.wasm tool_middleware_guest.wasm agent_tool_middleware_guest.wasm; do
  if ! cmp -s "$sbt_wasm_dir/$artifact" "$mill_wasm_dir/$artifact"; then
    echo "FAIL: sbt and Mill package different bytes for $artifact" >&2
    exit 1
  fi
done

ordinary_wit="$(wasm-tools component wit "$sbt_wasm_dir/agent_guest.wasm")"
assert_has "$ordinary_wit" "export golem:agent/guest@2.0.0;"
assert_has "$ordinary_wit" "export golem:tool/guest@0.1.0;"
assert_has "$ordinary_wit" "import golem:tool/host@0.1.0;"
assert_lacks "$ordinary_wit" "export golem:tool/tool-middleware-guest@0.1.0;"

middleware_wit="$(wasm-tools component wit "$sbt_wasm_dir/tool_middleware_guest.wasm")"
assert_has "$middleware_wit" "export golem:tool/tool-middleware-guest@0.1.0;"
assert_lacks "$middleware_wit" "export golem:agent/guest@2.0.0;"
assert_lacks "$middleware_wit" "export golem:tool/guest@0.1.0;"
assert_lacks "$middleware_wit" "import golem:tool/host@0.1.0;"

combined_wit="$(wasm-tools component wit "$sbt_wasm_dir/agent_tool_middleware_guest.wasm")"
assert_has "$combined_wit" "export golem:agent/guest@2.0.0;"
assert_has "$combined_wit" "export golem:tool/guest@0.1.0;"
assert_has "$combined_wit" "export golem:tool/tool-middleware-guest@0.1.0;"
assert_has "$combined_wit" "import golem:tool/host@0.1.0;"

echo "Scala guest role component contracts verified"
