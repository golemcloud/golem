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
 * Decodes hand-built, host-shaped `result<invocation-result, rpc-error>` buffers (the wire shape
 * `tool-rpc.invoke-and-await` / `future-invoke-result.get` return) via [liftToolInvokeResult], at
 * the abi-dump-verified literal offsets. Pure linear-memory ops (no host WIT import), so they run
 * under the plain wasmWasi nodejs runner. The host-calling invoke paths themselves are
 * compile-verified; this exercises the decode logic they depend on.
 *
 * Layout: `result<invocation-result, rpc-error>` = 48B, tag@0, payload@4.
 *   ok = invocation-result @4: result option (tag@4, tsv@8..39), stdout option (tag@40, handle@44).
 *   err = rpc-error @4: tag@4, string ptr@8 / len@12.
 */
class ToolInvokeResultDecodeTest {

    @Test
    fun ok_with_composite_result_and_no_stdout() {
        val tsv = TypedSchemaValue(
            "record<x:s32,msg:string>",
            SchemaValue.Record(listOf(SchemaValue.S32(7), SchemaValue.Str("hi"))),
        )
        val buf = alloc(48, 4)
        storeByte(buf, 0) // result tag = ok
        storeByte(buf + 4, 1) // invocation-result.result option = some
        lowerTypedSchemaValueInto(buf + 8, tsv)
        storeByte(buf + 40, 0) // stdout option = none

        val ok = assertIs<ToolInvokeResult.Ok>(liftToolInvokeResult(buf))
        assertEquals(tsv, ok.value.result)
        assertNull(ok.value.stdoutHandle)
    }

    @Test
    fun ok_with_stdout_handle_and_no_result() {
        val buf = alloc(48, 4)
        storeByte(buf, 0) // ok
        storeByte(buf + 4, 0) // result option = none
        storeByte(buf + 40, 1) // stdout option = some
        storeInt(buf + 44, 4242) // stdout output-stream handle

        val ok = assertIs<ToolInvokeResult.Ok>(liftToolInvokeResult(buf))
        assertNull(ok.value.result)
        assertEquals(4242, ok.value.stdoutHandle)
    }

    @Test
    fun err_not_found_carries_message() {
        val buf = alloc(48, 4)
        storeByte(buf, 1) // result tag = err
        storeByte(buf + 4, 2) // rpc-error tag = not-found
        writeStringField(buf + 4, 4, "no such command") // string @ rpc-error+4 (ptr@8 / len@12)

        val err = assertIs<ToolInvokeResult.Err>(liftToolInvokeResult(buf))
        assertEquals(ToolRpcError.NotFound("no such command"), err.error)
    }

    @Test
    fun err_remote_tool_error_has_no_message() {
        val buf = alloc(48, 4)
        storeByte(buf, 1) // err
        storeByte(buf + 4, 4) // rpc-error tag = remote-tool-error (no string payload)

        val err = assertIs<ToolInvokeResult.Err>(liftToolInvokeResult(buf))
        assertEquals(ToolRpcError.RemoteToolError, err.error)
    }
}
