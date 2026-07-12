package cloud.golem.ksp

import kotlin.test.Test
import kotlin.test.assertEquals

/**
 * Unit-tests the [ConverterCodegen] output for the `result<T,E>` (Kotlin `Either`) and `datetime`
 * type descriptors — pure string codegen, so no compile-testing needed.
 */
class ConverterCodegenTest {

    private val s32 = TypeDesc.Prim("s32")
    private val str = TypeDesc.Prim("string")

    @Test
    fun result_toWit_maps_either_arms_and_unit() {
        assertEquals("result<s32,string>", TypeDesc.ResultT(ok = s32, err = str).toWit())
        assertEquals("result<s32,_>", TypeDesc.ResultT(ok = s32, err = TypeDesc.UnitT).toWit())
        assertEquals("datetime", TypeDesc.DatetimeT.toWit())
    }

    @Test
    fun result_decode_maps_ok_to_right_err_to_left() {
        assertEquals(
            "(x as SchemaValue.ResultVal).let { if (it.ok) cloud.golem.runtime.Either.Right((it.inner!! as SchemaValue.S32).v) " +
                "else cloud.golem.runtime.Either.Left((it.inner!! as SchemaValue.Str).v) }",
            ConverterCodegen.decode(TypeDesc.ResultT(ok = s32, err = str), "x"),
        )
    }

    @Test
    fun result_encode_maps_right_to_ok_left_to_err() {
        assertEquals(
            "(k).let { when (it) { " +
                "is cloud.golem.runtime.Either.Right -> SchemaValue.ResultVal(true, SchemaValue.S32(it.value)); " +
                "is cloud.golem.runtime.Either.Left -> SchemaValue.ResultVal(false, SchemaValue.Str(it.value)) } }",
            ConverterCodegen.encode(TypeDesc.ResultT(ok = s32, err = str), "k"),
        )
    }

    @Test
    fun result_unit_arm_uses_Unit_and_null() {
        val td = TypeDesc.ResultT(ok = TypeDesc.UnitT, err = str)
        assertEquals(true, ConverterCodegen.decode(td, "x").contains("cloud.golem.runtime.Either.Right(Unit)"))
        assertEquals(true, ConverterCodegen.encode(td, "k").contains("SchemaValue.ResultVal(true, null)"))
    }

    @Test
    fun datetime_decode_and_encode() {
        assertEquals(
            "(x as SchemaValue.DatetimeVal).let { cloud.golem.Datetime(it.seconds, it.nanoseconds) }",
            ConverterCodegen.decode(TypeDesc.DatetimeT, "x"),
        )
        assertEquals(
            "SchemaValue.DatetimeVal((k).seconds, (k).nanoseconds)",
            ConverterCodegen.encode(TypeDesc.DatetimeT, "k"),
        )
    }
}
