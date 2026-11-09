package cloud.golem.permissions

import cloud.golem.runtime.Layout
import cloud.golem.runtime.SchemaValue
import cloud.golem.runtime.lowerSchemaTypeNodeInto
import cloud.golem.runtime.lowerStringToPtrLen
import cloud.golem.wasm.SchemaTags
import cloud.golem.wasm.alloc
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.resetHeap
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeInt
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue
import kotlin.time.Duration
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Duration.Companion.nanoseconds
import kotlin.time.Duration.Companion.seconds

/** Ports of the Rust permission tests that need no host: error mapping, lifespans and the handle ownership of cards. */
class PermissionsTest {
    private fun lifted(tag: Int, message: String): PermissionError {
        resetHeap()
        val base = alloc(12, 4)
        storeByte(base, tag.toByte())
        val (ptr, len) = lowerStringToPtrLen(message)
        storeInt(base + 4, ptr)
        storeInt(base + 8, len)
        return liftPermissionError(base)
    }

    @Test
    fun permission_error_covers_every_wit_case() {
        val kinds = listOf(
            "not-permitted",
            "lower-bound-too-broad",
            "upper-bound-too-broad",
            "wallet-multi-source-required",
            "card-revoked",
            "card-expired",
            "install-recipient-mismatch",
            "unknown-resource-class",
            "invalid-pattern",
            "invalid-verb",
            "invalid-owner",
            "invalid-recipient",
        )
        // golem:permissions/types.permission-error has exactly these cases; a WIT change must update this test.
        assertEquals(12, kinds.size)
        val seen = HashSet<String>()
        kinds.forEachIndexed { tag, kind ->
            val message = "m$tag"
            val error = lifted(tag, message)
            assertEquals(kind, error.kind)
            assertEquals(message, error.message)
            assertEquals("$kind: $message", error.toString())
            assertTrue(seen.add(kind))
        }
        assertFailsWith<IllegalStateException> { lifted(12, "x") }
    }

    @Test
    fun lifespan_from_iso8601_accepts_only_days_and_time_units() {
        for (value in listOf("PT1H", "P1DT2H", "PT30.5S", "P2D")) {
            assertEquals(value, Lifespan.iso8601(value).iso8601)
        }
        for (value in listOf("P1Y", "P1M", "P1W", "PT", "1h", "5s", "", "-PT1S")) {
            assertFailsWith<IllegalArgumentException>("$value must be rejected") { Lifespan.iso8601(value) }
        }
    }

    @Test
    fun lifespan_from_duration_is_a_fixed_iso8601_duration() {
        assertFailsWith<IllegalArgumentException> { Lifespan.of(Duration.ZERO) }
        assertEquals("P1DT1H1M1S", Lifespan.of(90_061.seconds).iso8601)
        assertEquals("PT2.5S", Lifespan.of(2_500.milliseconds).iso8601)
        assertEquals("PT0.000000001S", Lifespan.of(1.nanoseconds).iso8601)
        assertFailsWith<IllegalArgumentException> { Lifespan.of(Duration.INFINITE) }
    }

    // The cards themselves reference the host imports, which the node runner cannot link; the ownership rule lives in HandleSlot.

    @Test
    fun a_handle_moves_out_exactly_once() {
        val slot = HandleSlot(7)
        assertEquals("PermissionCard(<present>)", slot.describe("PermissionCard"))
        assertEquals(7, slot.borrow())
        assertEquals(7, slot.take())
        assertEquals("PermissionCard(<moved>)", slot.describe("PermissionCard"))
        assertTrue(!slot.isPresent)
        val error = assertFailsWith<IllegalStateException> { slot.borrow() }
        assertTrue(error.message.orEmpty().contains("already been moved"), error.message)
        assertFailsWith<IllegalStateException> { slot.take() }
    }

    @Test
    fun a_transferred_handle_is_moved_and_cannot_be_encoded_twice() {
        val slot = HandleSlot(11)
        val value = SchemaValue.PermissionCardVal(slot.take())
        assertEquals("PolymorphicPermissionCard(<moved>)", slot.describe("PolymorphicPermissionCard"))
        assertFailsWith<IllegalStateException> { slot.take() }

        val received = HandleSlot(value.handle)
        assertEquals("PolymorphicPermissionCard(<present>)", received.describe("PolymorphicPermissionCard"))
        assertEquals(11, received.take())
    }

    @Test
    fun card_types_declare_their_polymorphic_flag_on_the_wire() {
        fun polymorphic(witType: String): Boolean {
            resetHeap()
            val node = alloc(Layout.SCHEMA_TYPE_NODE_SIZE, Layout.SCHEMA_TYPE_NODE_ALIGN)
            lowerSchemaTypeNodeInto(node, witType, emptyMap())
            val body = node + Layout.STN_BODY
            assertEquals(SchemaTags.TYPE_PERMISSION_CARD, loadByte(body).toInt())
            return loadByte(body + Layout.SCHEMA_TYPE_BODY_PAYLOAD_OFFSET).toInt() != 0
        }
        assertEquals(false, polymorphic("permission-card"))
        assertEquals(true, polymorphic("polymorphic-permission-card"))
    }
}
