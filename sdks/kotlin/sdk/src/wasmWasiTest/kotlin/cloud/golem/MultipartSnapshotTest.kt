package cloud.golem

import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

class MultipartSnapshotTest {
    @Test
    fun encode_decode_round_trips_parts_in_any_insertion_order() {
        val snapshot = MultipartSnapshot(
            linkedMapOf(
                "index" to SnapshotPart(byteArrayOf(1, 2, 3), "application/octet-stream"),
                "a_b.c-d" to SnapshotPart("héllo".encodeToByteArray(), "Text/Plain"),
                "empty" to SnapshotPart(ByteArray(0)),
            ),
        )
        val back = MultipartSnapshot.decode(snapshot.encode())
        assertEquals(snapshot, back)
        assertEquals("text/plain", back.parts.getValue("a_b.c-d").contentType)
        assertContentEquals("héllo".encodeToByteArray(), back.requirePart("a_b.c-d", "text/plain"))
    }

    @Test
    fun a_zero_part_snapshot_is_valid() {
        assertEquals(MultipartSnapshot(emptyMap()), MultipartSnapshot.decode(MultipartSnapshot(emptyMap()).encode()))
    }

    @Test
    fun invalid_names_and_content_types_are_rejected() {
        for (bad in listOf("", "-x", ".x", "a b", "a/b", "é")) {
            assertFailsWith<IllegalArgumentException>("name '$bad'") { MultipartSnapshot(mapOf(bad to SnapshotPart(ByteArray(0)))) }
        }
        for (bad in listOf("", "text", "text/plain; charset=utf-8", "a/b/c", "text/pl ain")) {
            assertFailsWith<IllegalArgumentException>("type '$bad'") { SnapshotPart(ByteArray(0), bad) }
        }
    }

    @Test
    fun requirePart_reports_missing_and_mistyped_parts() {
        val snapshot = MultipartSnapshot(mapOf("a" to SnapshotPart(byteArrayOf(1), "image/png")))
        assertFailsWith<IllegalArgumentException> { snapshot.requirePart("b") }
        assertFailsWith<IllegalArgumentException> { snapshot.requirePart("a", "image/jpeg") }
        assertContentEquals(byteArrayOf(1), snapshot.requirePart("a", "IMAGE/PNG"))
    }

    @Test
    fun malformed_bytes_are_rejected() {
        val good = MultipartSnapshot(mapOf("a" to SnapshotPart(byteArrayOf(1, 2)))).encode()
        assertFailsWith<IllegalArgumentException> { MultipartSnapshot.decode(ByteArray(0)) }
        assertFailsWith<IllegalArgumentException> { MultipartSnapshot.decode(byteArrayOf(1, 2, 3, 4, 5, 6)) }
        assertFailsWith<IllegalArgumentException> { MultipartSnapshot.decode(good.copyOfRange(0, good.size - 1)) }
        assertFailsWith<IllegalArgumentException> { MultipartSnapshot.decode(good + byteArrayOf(0)) }
    }

    @Test
    fun dynamic_opaque_parts_survive_the_round_trip() {
        val boundaryLike = "--golem-snapshot-0\r\npayload\r\n--golem-snapshot-1--".encodeToByteArray()
        val snapshot = MultipartSnapshot(
            linkedMapOf(
                "index" to SnapshotPart(ByteArray(256) { it.toByte() }, "Application/Octet-Stream"),
                "__proto__" to SnapshotPart(ByteArray(0), "Application/Octet-Stream"),
                "state" to SnapshotPart(byteArrayOf(0, -1, 13, 10), "Application/Octet-Stream"),
                "framing" to SnapshotPart(boundaryLike, "application/octet-stream"),
            ),
        )
        val back = MultipartSnapshot.decode(snapshot.encode())
        assertEquals(snapshot, back)
        assertEquals(4, back.parts.size)
        for ((name, part) in snapshot.parts) assertContentEquals(part.bytes, back.requirePart(name, "APPLICATION/OCTET-STREAM"))
        assertFailsWith<IllegalArgumentException> { back.requirePart("missing", "text/plain") }
        assertFailsWith<IllegalArgumentException> { back.requirePart("index", "text/plain") }
        assertFailsWith<IllegalArgumentException> { back.requirePart("index", "application/octet-stream; charset=utf-8") }
    }

    @Test
    fun a_content_type_cannot_carry_a_header_injection() {
        assertFailsWith<IllegalArgumentException> { SnapshotPart(ByteArray(0), "text/plain\r\nHeader: injected") }
    }

    @Test
    fun encode_carries_state_bytes_beside_the_parts() {
        val snapshot = MultipartSnapshot(mapOf("history" to SnapshotPart(byteArrayOf(7, 8), "application/octet-stream")))
        val wire = snapshot.encode("typed-state".encodeToByteArray())
        assertContentEquals("typed-state".encodeToByteArray(), MultipartSnapshot.decodeState(wire))
        assertEquals(snapshot, MultipartSnapshot.decode(wire))
    }

    @Test
    fun an_agent_without_typed_state_encodes_an_empty_state_slot() {
        val wire = MultipartSnapshot(mapOf("a" to SnapshotPart(byteArrayOf(1)))).encode()
        assertContentEquals(ByteArray(0), MultipartSnapshot.decodeState(wire))
    }

    @Test
    fun a_part_named_state_is_not_confused_with_the_state_slot() {
        val snapshot = MultipartSnapshot(mapOf("state" to SnapshotPart("a part, not the slot".encodeToByteArray(), "text/plain")))
        val wire = snapshot.encode("the slot".encodeToByteArray())
        assertContentEquals("the slot".encodeToByteArray(), MultipartSnapshot.decodeState(wire))
        assertContentEquals("a part, not the slot".encodeToByteArray(), MultipartSnapshot.decode(wire).requirePart("state", "text/plain"))
    }

    @Test
    fun a_truncated_state_slot_is_rejected() {
        val wire = MultipartSnapshot(emptyMap()).encode(byteArrayOf(1, 2, 3, 4))
        assertFailsWith<IllegalArgumentException> { MultipartSnapshot.decodeState(wire.copyOfRange(0, wire.size - 1)) }
        assertFailsWith<IllegalArgumentException> { MultipartSnapshot.decode(wire.copyOfRange(0, wire.size - 1)) }
    }
}
