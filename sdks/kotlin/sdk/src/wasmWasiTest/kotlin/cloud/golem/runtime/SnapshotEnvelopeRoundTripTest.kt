package cloud.golem.runtime

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

class SnapshotEnvelopeRoundTripTest {
    @Test fun roundTrip() {
        val d = SnapshotEnvelope.decode(SnapshotEnvelope.encode(byteArrayOf(1, 2, 3), byteArrayOf(9, 8, 7, 6)))
        assertEquals(listOf<Byte>(1, 2, 3), d.principal.toList())
        assertEquals(listOf<Byte>(9, 8, 7, 6), d.state.toList())
    }

    @Test fun emptyState() {
        val d = SnapshotEnvelope.decode(SnapshotEnvelope.encode(byteArrayOf(3), ByteArray(0)))
        assertEquals(listOf<Byte>(3), d.principal.toList())
        assertEquals(0, d.state.size)
    }

    @Test fun rejectsUnknownVersion() {
        assertFailsWith<IllegalStateException> { SnapshotEnvelope.decode(byteArrayOf(99, 0, 0, 0, 0)) }
    }
}
