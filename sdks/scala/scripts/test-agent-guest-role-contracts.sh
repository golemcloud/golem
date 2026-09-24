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

artifact=agent_guest.wasm
if ! cmp -s "$sbt_wasm_dir/$artifact" "$mill_wasm_dir/$artifact"; then
  echo "FAIL: sbt and Mill package different bytes for $artifact" >&2
  exit 1
fi

ordinary_wit="$(wasm-tools component wit "$sbt_wasm_dir/agent_guest.wasm")"
assert_has "$ordinary_wit" "export golem:agent/guest@2.0.0;"
assert_has "$ordinary_wit" "export golem:tool/guest@0.1.0;"
assert_has "$ordinary_wit" "export golem:tool/tool-middleware-guest@0.1.0;"
assert_has "$ordinary_wit" "import golem:tool/host@0.1.0;"
assert_has "$ordinary_wit" "import golem:agent/durable-streams@2.0.0;"

host_dts="$(cat "$sdk_root/wit/dts/golem_agent_2_0_0_host.d.ts")"
streams_dts="$(cat "$sdk_root/wit/dts/golem_agent_2_0_0_durable_streams.d.ts")"
assert_lacks "$host_dts" "DurableStream"
assert_has "$streams_dts" "declare module 'golem:agent/durable-streams@2.0.0'"
assert_has "$streams_dts" "export class DurableStreamReader"
assert_has "$streams_dts" "export class DurableStreamWriter"
assert_has "$streams_dts" "constructor(options: DurableStreamReaderOptions, auth: Secret | undefined)"
assert_has "$streams_dts" "constructor(options: DurableStreamWriterOptions, auth: Secret | undefined)"
assert_has "$streams_dts" "read(request: DurableStreamReadRequest)"
assert_has "$streams_dts" "append(request: DurableStreamAppendRequest)"
assert_lacks "$streams_dts" "readDurableStreamBatch"
assert_lacks "$streams_dts" "appendDurableStreamBatch"
assert_lacks "$streams_dts" "DurableStreamProducer"
assert_has "$ordinary_wit" "resource durable-stream-reader"
assert_has "$ordinary_wit" "resource durable-stream-writer"
assert_lacks "$ordinary_wit" "read-durable-stream-batch"
assert_lacks "$ordinary_wit" "append-durable-stream-batch"

echo "Scala guest component contract verified"
