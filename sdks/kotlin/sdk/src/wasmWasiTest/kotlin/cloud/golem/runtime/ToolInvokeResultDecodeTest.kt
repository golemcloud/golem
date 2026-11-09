package cloud.golem.runtime

import cloud.golem.wasm.alloc
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeInt
import cloud.golem.wasm.writeStringField
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertNull

/**
 * Decodes hand-built, host-shaped `result<invocation-result, tool-rpc-error>` buffers (the wire shape
 * `tool-rpc.invoke-and-await` / `future-invoke-result.get` return in Golem 1.6.0) via [liftToolInvokeResult], at
 * offsets computed from `wasm-tools component wit --json`. Pure linear-memory ops (no host WIT import), so they run
 * under the plain wasmWasi nodejs runner. The host-calling paths themselves are verified on a real executor.
 *
 * Layout: `result<invocation-result, tool-rpc-error>` = 56B, tag@0, payload@4.
 *   ok = invocation-result @4: result option (tag@4, tsv@8..39), stdout option @40, stderr option @48 (both absent).
 *   err = tool-rpc-error @4: tag@4; string payloads ptr@8 / len@12; remote-tool-error: tool-error @8 (tag@8).
 */
class ToolInvokeResultDecodeTest {

    @Test
    fun ok_with_composite_result() {
        val tsv = TypedSchemaValue(
            "record<x:s32,msg:string>",
            SchemaValue.Record(listOf(SchemaValue.S32(7), SchemaValue.Str("hi"))),
        )
        val buf = alloc(56, 4)
        storeByte(buf, 0) // result tag = ok
        storeByte(buf + 4, 1) // invocation-result.result option = some
        lowerTypedSchemaValueInto(buf + 8, tsv)
        storeByte(buf + 40, 0) // stdout option = none
        storeByte(buf + 48, 0) // stderr option = none

        val ok = assertIs<ToolInvokeResult.Ok>(liftToolInvokeResult(buf))
        assertEquals(tsv, ok.value.result)
    }

    @Test
    fun ok_without_result() {
        val buf = alloc(56, 4)
        storeByte(buf, 0) // ok
        storeByte(buf + 4, 0) // result option = none

        val ok = assertIs<ToolInvokeResult.Ok>(liftToolInvokeResult(buf))
        assertNull(ok.value.result)
    }

    @Test
    fun err_not_found_carries_message() {
        val buf = alloc(56, 4)
        storeByte(buf, 1) // result tag = err
        storeByte(buf + 4, 2) // tool-rpc-error tag = not-found
        writeStringField(buf + 4, 4, "no such command") // string @ error+4 (ptr@8 / len@12)

        val err = assertIs<ToolInvokeResult.Err>(liftToolInvokeResult(buf))
        assertEquals(ToolRpcError.NotFound("no such command"), err.error)
    }

    @Test
    fun err_remote_tool_error_carries_the_tool_error() {
        val buf = alloc(56, 4)
        storeByte(buf, 1) // err
        storeByte(buf + 4, 4) // tool-rpc-error tag = remote-tool-error
        storeByte(buf + 8, 3) // tool-error tag = constraint-violation
        writeStringField(buf + 8, 4, "value must not be bad") // string @ tool-error+4 (ptr@12 / len@16)

        val err = assertIs<ToolInvokeResult.Err>(liftToolInvokeResult(buf))
        assertEquals(ToolRpcError.RemoteToolError(ToolError.ConstraintViolation("value must not be bad")), err.error)
    }

    @Test
    fun err_remote_invalid_command_path_carries_the_path() {
        val buf = alloc(56, 4)
        storeByte(buf, 1) // err
        storeByte(buf + 4, 4) // remote-tool-error
        storeByte(buf + 8, 1) // tool-error tag = invalid-command-path (list<string> ptr@12 / len@16)
        val list = alloc(16, 4)
        writeStringField(list, 0, "sub")
        writeStringField(list, 8, "leaf")
        storeInt(buf + 12, list)
        storeInt(buf + 16, 2)

        val err = assertIs<ToolInvokeResult.Err>(liftToolInvokeResult(buf))
        assertEquals(ToolRpcError.RemoteToolError(ToolError.InvalidCommandPath(listOf("sub", "leaf"))), err.error)
    }

    @Test
    fun err_cancelled_has_no_payload() {
        val buf = alloc(56, 4)
        storeByte(buf, 1) // err
        storeByte(buf + 4, 5) // tool-rpc-error tag = cancelled

        val err = assertIs<ToolInvokeResult.Err>(liftToolInvokeResult(buf))
        assertEquals(ToolRpcError.Cancelled, err.error)
    }

    @Test
    fun err_remote_custom_error_carries_name_and_typed_payload() {
        val buf = alloc(56, 4)
        storeByte(buf, 1) // err
        storeByte(buf + 4, 4) // remote-tool-error
        storeByte(buf + 8, 5) // tool-error tag = custom-error: custom-tool-error @12 = { name@12, payload tsv@20 }
        writeStringField(buf + 8, 4, "out-of-range")
        val payload = TypedSchemaValue("s32", SchemaValue.S32(42))
        lowerTypedSchemaValueInto(buf + 20, payload)

        val err = assertIs<ToolInvokeResult.Err>(liftToolInvokeResult(buf))
        assertEquals(ToolRpcError.RemoteToolError(ToolError.CustomError("out-of-range", payload)), err.error)
    }
}
