package cloud.golem.runtime

import cloud.golem.wasm.alloc
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadLong
import kotlin.test.Test
import kotlin.test.assertEquals

/**
 * Verifies [lowerReadOnlyInto] writes `option<read-only-config>` at the abi-dump-verified offsets:
 * option tag@0; read-only-config @8 { cache-policy @0 (tag@8, ttl-duration u64 @16),
 * uses-principal: bool @24 }. Pure linear-memory ops (no host WIT import), so they run under the
 * plain wasmWasi nodejs runner. [lowerOptionStringInto] (the @Prompt hint wiring) is covered here too.
 */
class ReadOnlyLoweringTest {

    private fun buf() = alloc(32, 8) // option(8) + read-only-config(24)

    @Test
    fun not_read_only_is_none() {
        val b = buf()
        lowerReadOnlyInto(b, 0, null)
        assertEquals(0, loadByte(b).toInt() and 0xFF, "option tag should be none")
    }

    @Test
    fun until_write_is_the_default_policy() {
        val b = buf()
        lowerReadOnlyInto(b, 0, "until-write")
        assertEquals(1, loadByte(b).toInt() and 0xFF, "option = some")
        assertEquals(1, loadByte(b + 8).toInt() and 0xFF, "cache-policy = until-write")
        assertEquals(0, loadByte(b + 8 + 16).toInt() and 0xFF, "uses-principal = false")
    }

    @Test
    fun no_cache_policy() {
        val b = buf()
        lowerReadOnlyInto(b, 0, "no-cache")
        assertEquals(1, loadByte(b).toInt() and 0xFF)
        assertEquals(0, loadByte(b + 8).toInt() and 0xFF, "cache-policy = no-cache")
    }

    @Test
    fun ttl_policy_carries_nanos() {
        val b = buf()
        lowerReadOnlyInto(b, 0, "ttl(5000000000)")
        assertEquals(1, loadByte(b).toInt() and 0xFF)
        assertEquals(2, loadByte(b + 8).toInt() and 0xFF, "cache-policy = ttl")
        assertEquals(5_000_000_000L, loadLong(b + 8 + 8), "ttl duration nanos")
        assertEquals(0, loadByte(b + 8 + 16).toInt() and 0xFF, "uses-principal = false")
    }

    @Test
    fun option_string_none_and_some() {
        val none = buf()
        lowerOptionStringInto(none, 0, "")
        assertEquals(0, loadByte(none).toInt() and 0xFF, "empty hint = none")

        val some = buf()
        lowerOptionStringInto(some, 0, "do the thing")
        assertEquals(1, loadByte(some).toInt() and 0xFF, "non-empty hint = some")
    }
}
