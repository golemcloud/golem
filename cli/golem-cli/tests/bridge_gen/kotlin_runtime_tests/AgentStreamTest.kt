package golem.bridge.runtime

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

class AgentStreamTest {
    @Test
    fun a_stream_has_one_consumer() {
        val stream = AgentStream.of(listOf(1, 2))
        AgentStream.claim(stream)
        assertFailsWith<BridgeException> { AgentStream.claim(stream) }
    }

    @Test
    fun a_closed_stream_reports_that_it_is_closed() {
        val stream = AgentStream.of(listOf(1, 2))
        stream.close()
        assertEquals("AgentStream is closed", assertFailsWith<BridgeException> { stream.next() }.message)
        assertEquals("AgentStream is closed", assertFailsWith<BridgeException> { AgentStream.claim(stream) }.message)
    }

    @Test
    fun a_claimed_stream_reports_that_it_has_a_consumer() {
        val stream = AgentStream.of(listOf(1, 2))
        AgentStream.claim(stream)
        stream.close()
        assertEquals("AgentStream already has a consumer", assertFailsWith<BridgeException> { stream.next() }.message)
    }

    @Test
    fun a_claimed_stream_cannot_be_read_locally() {
        val stream = AgentStream.of(listOf(1, 2))
        AgentStream.claim(stream)
        assertFailsWith<BridgeException> { stream.next() }
    }

    @Test
    fun of_delivers_its_values_then_ends() {
        val stream = AgentStream.of(listOf("a", "b"))
        assertEquals("a", stream.next())
        assertEquals("b", stream.next())
        assertNull(stream.next())
        assertNull(stream.next())
    }

    @Test
    fun to_list_collects_every_value() {
        assertEquals(listOf(1, 2, 3), AgentStream.of(listOf(1, 2, 3)).toList())
    }

    @Test
    fun to_list_refuses_more_items_than_its_limit() {
        assertFailsWith<BridgeException> { AgentStream.of(listOf(1, 2, 3)).toList(maxItems = 2) }
    }

    @Test
    fun to_list_refuses_a_limit_above_the_ceiling() {
        assertFailsWith<BridgeException> { AgentStream.of(listOf(1)).toList(maxItems = ITEMS_MAX + 1) }
    }

    @Test
    fun a_produced_body_runs_off_the_calling_thread() {
        val caller = Thread.currentThread()
        val seen = AtomicReference<Thread>()
        val stream = AgentStream.produce<Int> { sink -> seen.set(Thread.currentThread()); sink.write(1) }
        assertEquals(1, stream.next())
        assertNotEquals(caller, seen.get())
        assertTrue(seen.get().isDaemon)
    }

    @Test
    fun a_produced_body_that_throws_fails_the_pull() {
        val stream = AgentStream.produce<Int> { throw IllegalStateException("source gone") }
        val error = assertFailsWith<AgentStreamError> { stream.next() }
        assertEquals("producer-error", error.code)
        assertEquals("source gone", error.detail)
    }

    @Test
    fun cancelling_a_produced_stream_stops_its_writer() {
        val writerStopped = CountDownLatch(1)
        val lastWrite = AtomicBoolean(true)
        val stream = AgentStream.produce<Int> { sink ->
            var n = 0
            while (sink.write(n++)) { /* keep writing until the reader goes away */ }
            lastWrite.set(false)
            writerStopped.countDown()
        }
        assertEquals(0, stream.next())
        stream.close()
        assertTrue(writerStopped.await(10, TimeUnit.SECONDS), "the writer was still blocked after close")
        assertTrue(!lastWrite.get())
        assertNull(stream.next())
    }

    @Test
    fun a_remote_stream_delegates_to_its_consumer() {
        val cancelled = AtomicBoolean(false)
        val items = ArrayDeque(listOf(7, 8))
        val consumer = object : AgentStream.Consumer<Int> {
            override fun pull(): java.util.concurrent.CompletableFuture<AgentStreamStep<Int>> =
                java.util.concurrent.CompletableFuture.completedFuture(
                    items.removeFirstOrNull()?.let { AgentStreamStep.Item(it) } ?: AgentStreamStep.End
                )

            override fun cancel(): java.util.concurrent.CompletableFuture<Void?> {
                cancelled.set(true)
                return java.util.concurrent.CompletableFuture.completedFuture(null)
            }

            override fun drop(): java.util.concurrent.CompletableFuture<Void?> = cancel()
        }
        val stream = AgentStream.remote(consumer)
        assertEquals(7, stream.next())
        stream.close()
        assertTrue(cancelled.get())
    }
}
