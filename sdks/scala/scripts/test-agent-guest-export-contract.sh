#!/usr/bin/env bash
set -euo pipefail

sdk_root="$(cd "$(dirname "$0")/.." && pwd)"
exports_dts="$sdk_root/wit/dts/exports.d.ts"
guest_runtime="$sdk_root/core/js/src/main/scala/golem/runtime/guest/Guest.scala"
middleware_exports_dts="$sdk_root/.generated/tool-middleware-guest-dts/exports.d.ts"
middleware_guest_runtime="$sdk_root/core/js/src/main/scala/golem/runtime/guest/ToolMiddlewareGuest.scala"

test_generated_agent_guest_namespace_is_exported_by_scala_runtime() {
  local namespace
  namespace="$({
    awk '
      /^  export namespace / {
        namespace = $3
      }
      /export function discoverAgentTypes/ {
        print namespace
        exit
      }
    ' "$exports_dts"
  })"

  if [[ -z "$namespace" ]]; then
    echo "FAIL: could not find the generated discoverAgentTypes namespace in $exports_dts" >&2
    return 1
  fi

  if ! grep -qF -- "@JSExportTopLevel(\"$namespace\")" "$guest_runtime"; then
    echo "FAIL: generated wrapper calls $namespace.discoverAgentTypes, but the Scala runtime does not export @JSExportTopLevel(\"$namespace\")" >&2
    return 1
  fi
}

test_generated_tool_guest_namespace_is_exported_by_scala_runtime() {
  local namespace
  namespace="$({
    awk '
      /^  export namespace / {
        namespace = $3
      }
      /export function discoverTools/ {
        print namespace
        exit
      }
    ' "$exports_dts"
  })"

  if [[ -z "$namespace" ]]; then
    echo "FAIL: could not find the generated discoverTools namespace in $exports_dts" >&2
    return 1
  fi

  if ! grep -qF -- "@JSExportTopLevel(\"$namespace\")" "$guest_runtime"; then
    echo "FAIL: generated wrapper calls $namespace.discoverTools, but the Scala runtime does not export @JSExportTopLevel(\"$namespace\")" >&2
    return 1
  fi
}

test_generated_middleware_guest_namespace_is_exported_by_scala_runtime() {
  local namespace
  namespace="$({
    awk '
      /^  export namespace / {
        namespace = $3
      }
      /export function discoverToolMiddlewares/ {
        print namespace
        exit
      }
    ' "$middleware_exports_dts"
  })"

  if [[ -z "$namespace" ]]; then
    echo "FAIL: could not find the generated discoverToolMiddlewares namespace in $middleware_exports_dts" >&2
    return 1
  fi

  if ! grep -qF -- "@JSExportTopLevel(\"$namespace\")" "$middleware_guest_runtime"; then
    echo "FAIL: generated wrapper calls $namespace.discoverToolMiddlewares, but the Scala runtime does not export @JSExportTopLevel(\"$namespace\")" >&2
    return 1
  fi
}

test_generated_agent_guest_namespace_is_exported_by_scala_runtime
test_generated_tool_guest_namespace_is_exported_by_scala_runtime
test_generated_middleware_guest_namespace_is_exported_by_scala_runtime
