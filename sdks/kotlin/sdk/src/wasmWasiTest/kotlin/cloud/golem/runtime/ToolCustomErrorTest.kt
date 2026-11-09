@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.runtime

import cloud.golem.wasm.alloc
import kotlin.test.Test
import kotlin.test.assertEquals

/** A declared tool error survives the canonical-ABI lowering the guest does and the lift a caller does. */
class ToolCustomErrorTest {
    private fun roundTrip(error: ToolError): ToolError {
        val base = alloc(44, 4)
        lowerToolErrorInto(base, error)
        return liftToolError(base)
    }

    @Test
    fun payloadless_error_uses_the_empty_tuple() {
        val error = ToolError.CustomError("not-found", TypedSchemaValue(UNIT_WIT_TYPE, SchemaValue.TupleVal(emptyList())))
        assertEquals(error, roundTrip(error))
    }

    @Test
    fun payload_error_keeps_name_and_typed_payload() {
        val error = ToolError.CustomError(
            "quota-exceeded",
            TypedSchemaValue("record<limit:s32,reason:string>", SchemaValue.Record(listOf(SchemaValue.S32(5), SchemaValue.Str("daily")))),
        )
        assertEquals(error, roundTrip(error))
    }
}
