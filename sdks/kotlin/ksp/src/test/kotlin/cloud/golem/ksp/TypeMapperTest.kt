package cloud.golem.ksp

import kotlin.test.Test
import kotlin.test.assertEquals

class TypeMapperTest {

    @Test
    fun `fqnToWit maps Int to s32`() = assertEquals("s32", TypeMapper.fqnToWit("kotlin.Int"))

    @Test
    fun `fqnToWit maps Long to s64`() = assertEquals("s64", TypeMapper.fqnToWit("kotlin.Long"))

    @Test
    fun `fqnToWit maps String to string`() = assertEquals("string", TypeMapper.fqnToWit("kotlin.String"))

    @Test
    fun `fqnToWit maps Boolean to bool`() = assertEquals("bool", TypeMapper.fqnToWit("kotlin.Boolean"))

    @Test
    fun `fqnToWit maps Unit to unit`() = assertEquals("()", TypeMapper.fqnToWit("kotlin.Unit"))

    @Test
    fun `round-trip Int s32`() {
        val wit = TypeMapper.fqnToWit("kotlin.Int")
        assertEquals("kotlin.Int", TypeMapper.witToFqn(wit))
    }

    @Test
    fun `round-trip String string`() {
        val wit = TypeMapper.fqnToWit("kotlin.String")
        assertEquals("kotlin.String", TypeMapper.witToFqn(wit))
    }

    /**
     * C.6 invariant (the part expressible at the type level): C's Kotlin->WIT
     * mapping is the inverse of A's WIT->Kotlin mapping for the types the counter
     * uses. Phase A maps s32->Int and string->String; C maps Int->s32 and
     * String->string, so a value the user writes survives the round trip.
     * (Widths A collapses to Int — see A-Q1 — are excluded by design.)
     */
    @Test
    fun `counter types round-trip exactly`() {
        for (fqn in listOf("kotlin.Int", "kotlin.String", "kotlin.Unit")) {
            assertEquals(fqn, TypeMapper.witToFqn(TypeMapper.fqnToWit(fqn)))
        }
    }
}
