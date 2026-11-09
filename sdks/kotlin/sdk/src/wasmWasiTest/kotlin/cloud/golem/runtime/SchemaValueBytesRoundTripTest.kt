package cloud.golem.runtime

import kotlin.test.Test
import kotlin.test.assertEquals

class SchemaValueBytesRoundTripTest {
    private fun rt(v: SchemaValue) = assertEquals(v, SchemaValueBytes.decode(SchemaValueBytes.encode(v)), "round-trip: $v")

    @Test fun primitives() {
        rt(SchemaValue.Bool(true))
        rt(SchemaValue.S8(-8))
        rt(SchemaValue.S16(-16))
        rt(SchemaValue.S64(-64L))
        rt(SchemaValue.U8(8u))
        rt(SchemaValue.U16(16u))
        rt(SchemaValue.U64(64uL))
        rt(SchemaValue.F32(1.5f))
        rt(SchemaValue.F64(2.5))
        rt(SchemaValue.Chr('x'))
        rt(SchemaValue.Str("hello"))
        rt(SchemaValue.Unit_)
    }

    @Test fun nestedComposite() {
        rt(
            SchemaValue.Record(
                listOf(
                    SchemaValue.ListVal(listOf(SchemaValue.S32(1), SchemaValue.S32(2))),
                    SchemaValue.OptionVal(SchemaValue.Str("x")),
                    SchemaValue.OptionVal(null),
                    SchemaValue.EnumVal(2),
                    SchemaValue.VariantVal(1, SchemaValue.S32(9)),
                    SchemaValue.VariantVal(0, null),
                    SchemaValue.TupleVal(listOf(SchemaValue.S32(3), SchemaValue.Str("t"))),
                    SchemaValue.MapVal(listOf(SchemaValue.Str("k") to SchemaValue.S32(1))),
                    SchemaValue.ResultVal(true, SchemaValue.S32(7)),
                    SchemaValue.DatetimeVal(1000L, 500),
                ),
            ),
        )
    }
}
