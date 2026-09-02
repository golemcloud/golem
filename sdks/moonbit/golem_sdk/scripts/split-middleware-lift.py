#!/usr/bin/env python3
"""Split the pinned generator's oversized middleware argument lift.

The MoonBit compiler currently ICEs on the generated ~7,500-line
wasmExportInvokeToolMiddleware function. This checked transformation only
accepts the exact shape emitted by the repository-pinned wit-bindgen revision.
"""

import hashlib
import sys
from pathlib import Path


EXPECTED_INVOKE_LIFT_SHA256 = (
    "563d4220b02be6c79fce3bd9242fc992a2049b7a66da6b836ea86b16749461b2"
)


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: split-middleware-lift.py <ffi.mbt>")
    path = Path(sys.argv[1])
    lines = path.read_text().splitlines()
    signature = "pub fn wasmExportInvokeToolMiddleware(p0 : Int) -> Int {"
    if lines.count(signature) != 1:
        raise SystemExit("expected exactly one invoke-tool-middleware binding")
    signature_index = lines.index(signature)
    raw_start = signature_index - 1
    task_return_prefix = "fn wasmExportAsyncInvokeToolMiddlewareTaskReturn("
    task_return_indices = [
        index for index, line in enumerate(lines) if line.startswith(task_return_prefix)
    ]
    if len(task_return_indices) != 1:
        raise SystemExit("expected exactly one invoke-tool-middleware task return")
    task_return_index = task_return_indices[0]
    raw_end = task_return_index - 1
    if raw_start < 0 or lines[raw_start] != "#doc(hidden)":
        raise SystemExit("unexpected truncated invoke-tool-middleware binding")
    if raw_end <= raw_start or lines[raw_end] != "///|":
        raise SystemExit("unexpected invoke-tool-middleware binding end")
    raw_lift = "\n".join(lines[raw_start:raw_end]) + "\n"
    digest = hashlib.sha256(raw_lift.encode()).hexdigest()
    if digest != EXPECTED_INVOKE_LIFT_SHA256:
        raise SystemExit(
            "unexpected invoke-tool-middleware lift digest: "
            f"expected {EXPECTED_INVOKE_LIFT_SHA256}, got {digest}"
        )
    output = lines[:raw_start]

    def source_line(original_line: int) -> int:
        return raw_start + (original_line - 11739)

    def block(start: int, end: int) -> list[str]:
        return [
            line[12:] if line.startswith("            ") else line
            for line in lines[source_line(start) : source_line(end + 1)]
        ]

    def helper(signature: str, start: int, end: int, result: str) -> None:
        output.extend(["///|", signature])
        output.extend("      " + line if line else "" for line in block(start, end))
        output.extend(["      " + result, "}", ""])

    helper(
        "fn __wit_bindgen_lift_invoke_tool_commands(p0 : Int) -> @common.CommandTree {",
        11750,
        15842,
        "@common.CommandTree::{nodes : array547}",
    )
    helper(
        "fn __wit_bindgen_lift_invoke_tool_schema(p0 : Int) -> @types.SchemaGraph {",
        15844,
        17386,
        "@types.SchemaGraph::{type_nodes : array745, defs : array750, root : mbt_ffi_load32((p0) + 48)}",
    )
    helper(
        "fn __wit_bindgen_lift_invoke_command_path(p0 : Int) -> Array[String] {",
        17388,
        17396,
        "array753",
    )
    helper(
        "fn __wit_bindgen_lift_invoke_input_graph(p0 : Int) -> @types.SchemaGraph {",
        17398,
        18940,
        "@types.SchemaGraph::{type_nodes : array951, defs : array956, root : mbt_ffi_load32((p0) + 76)}",
    )
    helper(
        "fn __wit_bindgen_lift_invoke_input_value(p0 : Int) -> @types.SchemaValueTree {",
        18942,
        19224,
        "@types.SchemaValueTree::{value_nodes : array987, root : mbt_ffi_load32((p0) + 88)}",
    )
    helper(
        "fn __wit_bindgen_lift_invoke_stdin(p0 : Int) -> @async-core.Stream[Byte]? {",
        19226,
        19234,
        "lifted990",
    )
    helper(
        "fn __wit_bindgen_lift_invoke_principal(p0 : Int) -> @common0.Principal {",
        19236,
        19337,
        "lifted1008",
    )
    output.extend(
        """///|
#doc(hidden)
pub fn wasmExportInvokeToolMiddleware(p0 : Int) -> Int {
      @async-core.with_waitableset(async fn() {
            @async-core.with_task_group(async fn(background_group) {
                  let middleware_name = mbt_ffi_ptr2str(mbt_ffi_load32((p0) + 0), mbt_ffi_load32((p0) + 4))
                  let tool_name = mbt_ffi_ptr2str(mbt_ffi_load32((p0) + 8), mbt_ffi_load32((p0) + 12))
                  let version = mbt_ffi_ptr2str(mbt_ffi_load32((p0) + 16), mbt_ffi_load32((p0) + 20))
                  let tool_metadata = @common.Tool::{
                        version,
                        commands: __wit_bindgen_lift_invoke_tool_commands(p0),
                        schema: __wit_bindgen_lift_invoke_tool_schema(p0),
                  }
                  let input = @types.TypedSchemaValue::{
                        graph: __wit_bindgen_lift_invoke_input_graph(p0),
                        value: __wit_bindgen_lift_invoke_input_value(p0),
                  }
                  let return_result: Ref[Result[@common.InvocationResult, @common.ToolError]?] = Ref(None)
                  return_result.val = Some(invoke_tool_middleware(
                        middleware_name,
                        tool_name,
                        tool_metadata,
                        __wit_bindgen_lift_invoke_command_path(p0),
                        input,
                        __wit_bindgen_lift_invoke_stdin(p0),
                        __wit_bindgen_lift_invoke_principal(p0),
                        @common.UnderlyingTool::UnderlyingTool(mbt_ffi_load32((p0) + 216)),
                        background_group,
                  ))
                  invoke_tool_middleware_task_return(return_result)
            })
      })
}
""".splitlines()
    )
    output.extend(lines[raw_end:])
    path.write_text("\n".join(output) + "\n")


if __name__ == "__main__":
    main()
