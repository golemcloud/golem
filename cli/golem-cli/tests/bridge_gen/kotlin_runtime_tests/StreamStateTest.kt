package golem.bridge.runtime

import golem.bridge.runtime.StreamSessionState.Delivery
import golem.bridge.runtime.StreamSessionState.InputState
import golem.bridge.runtime.StreamSessionState.OutputQueue
import golem.bridge.runtime.StreamSessionState.PendingData
import golem.bridge.runtime.StreamSessionState.PendingInput
import golem.bridge.runtime.StreamSessionState.PendingTerminal
import java.math.BigInteger
import java.util.concurrent.CompletableFuture
import java.util.concurrent.CompletionException
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertSame
import kotlin.test.assertTrue

class StreamStateTest {
    private fun big(value: Long) = BigInteger.valueOf(value)

    private val sent = CompletableFuture.completedFuture<Void?>(null)

    private fun packed(first: Long, count: Long): PendingData {
        fun range(from: Long): PendingData = PendingData(
            big(from),
            big(first + count - from),
            (first + count - from).toInt(),
            { sent },
            { highWater -> range(highWater.toLong()) },
        )
        return range(first)
    }

    private fun atomic(first: Long, bytes: Int): PendingData = PendingData(big(first), BigInteger.ONE, bytes, { sent })

    private fun failureOf(body: () -> Unit): String? = assertFailsWith<BridgeException> { body() }.message

    private fun step(value: Int, bytes: Int = 1) = Delivery(bytes, { AgentStreamStep.Item(value) }, null)

    private fun item(future: CompletableFuture<AgentStreamStep<Int>>): Int =
        (future.getNow(null) as AgentStreamStep.Item<Int>).value

    private fun failureOf(future: CompletableFuture<AgentStreamStep<Int>>): Throwable {
        val thrown = assertFailsWith<CompletionException> { future.join() }
        return thrown.cause!!
    }

    private class Released {
        val total = AtomicLong(0)
        val calls = AtomicInteger(0)
        val release: (Int) -> Unit = { total.addAndGet(it.toLong()); calls.incrementAndGet() }
    }

    @Test
    fun a_reserved_range_is_released_by_its_cumulative_ack() {
        val total = AtomicLong(0)
        val state = InputState(total)
        state.reserve(atomic(0, 10))
        assertEquals(10, state.pendingBytes)
        assertEquals(10L, total.get())
        assertEquals(BigInteger.ONE, state.nextSequence)
        assertTrue(state.acknowledge(BigInteger.ONE, false))
        assertEquals(0, state.pendingBytes)
        assertEquals(0L, total.get())
    }

    @Test
    fun a_duplicate_ack_after_release_is_a_no_op_and_a_conflicting_one_is_rejected() {
        val state = InputState()
        state.reserve(atomic(0, 10))
        assertTrue(state.acknowledge(BigInteger.ONE, false))
        assertFalse(state.acknowledge(BigInteger.ONE, false))
        assertEquals("conflicting duplicate input ACK", failureOf { state.acknowledge(BigInteger.ZERO, false) })
        assertEquals("conflicting duplicate input ACK", failureOf { state.acknowledge(BigInteger.ONE, true) })
    }

    @Test
    fun an_ack_inside_a_pending_range_trims_it_once() {
        val total = AtomicLong(0)
        val state = InputState(total)
        val (reserved, _) = state.reserve(packed(0, 4))
        assertEquals(4, reserved.bytes)
        assertFalse(state.acknowledge(big(2), false))
        assertEquals(2, state.pendingBytes)
        assertEquals(2L, total.get())
        val replay = assertNotNull(state.remap(5, big(2), false))
        assertEquals(big(2), replay.first.first)
        assertEquals(big(4), replay.first.highWater)
        assertEquals(2, replay.first.bytes)
        assertEquals(2L, total.get())
        assertTrue(state.acknowledge(big(4), false))
        assertEquals(0L, total.get())
    }

    @Test
    fun a_partial_ack_inside_an_atomic_item_is_rejected_and_leaves_the_range_pending() {
        val total = AtomicLong(0)
        val state = InputState(total)
        val range = PendingData(BigInteger.ZERO, big(4), 4, { sent })
        state.reserve(range)
        assertEquals("input high-water split an atomic item", failureOf { state.acknowledge(big(2), false) })
        assertEquals(4, state.pendingBytes)
        assertEquals(4L, total.get())
        assertTrue(state.acknowledge(big(4), false))
        assertEquals(0L, total.get())
    }

    @Test
    fun a_remap_replays_the_pending_range_exactly_once() {
        val total = AtomicLong(0)
        val state = InputState(total)
        val range = atomic(0, 8)
        state.reserve(range)
        state.detach()
        assertEquals(0L, state.currentChannel)
        val first = assertNotNull(state.remap(7, BigInteger.ZERO, false))
        assertSame(range, first.first)
        assertEquals(7L, first.second)
        assertEquals(7L, state.currentChannel)
        assertEquals(8L, total.get())
        val second = assertNotNull(state.remap(9, BigInteger.ZERO, false))
        assertSame(range, second.first)
        assertEquals(9L, second.second)
        assertEquals(8L, total.get())
        assertEquals(8, state.pendingBytes)
    }

    @Test
    fun a_remap_past_the_pending_range_releases_it() {
        val total = AtomicLong(0)
        val state = InputState(total)
        state.reserve(atomic(0, 8))
        assertNull(state.remap(3, BigInteger.ONE, false))
        assertEquals(0L, total.get())
        assertEquals(0, state.pendingBytes)
        assertEquals(3L, state.currentChannel)
    }

    @Test
    fun a_remap_inside_a_packed_range_trims_it() {
        val total = AtomicLong(0)
        val state = InputState(total)
        state.reserve(packed(0, 6))
        val replay = assertNotNull(state.remap(2, big(4), false))
        assertEquals(big(4), replay.first.first)
        assertEquals(big(6), replay.first.highWater)
        assertEquals(2L, total.get())
    }

    @Test
    fun a_remap_with_no_pending_range_returns_nothing() {
        val state = InputState()
        assertNull(state.remap(1, BigInteger.ZERO, false))
        state.reserve(atomic(0, 1))
        assertTrue(state.acknowledge(BigInteger.ONE, false))
        assertNull(state.remap(2, BigInteger.ONE, false))
    }

    @Test
    fun a_remap_the_client_cannot_honour_is_rejected() {
        val beyond = InputState()
        beyond.reserve(atomic(0, 1))
        assertEquals("input high-water is beyond sent data", failureOf { beyond.remap(1, big(2), false) })

        val split = InputState()
        split.reserve(PendingData(BigInteger.ZERO, big(4), 4, { sent }))
        assertEquals("input high-water split an atomic item", failureOf { split.remap(1, big(2), false) })

        val outsidePending = InputState()
        outsidePending.reserve(atomic(0, 1))
        outsidePending.acknowledge(BigInteger.ONE, false)
        outsidePending.reserve(atomic(1, 1))
        assertEquals("input high-water conflicts with pending range", failureOf { outsidePending.remap(1, BigInteger.ZERO, false) })

        val unsentTerminal = InputState()
        unsentTerminal.reserve(atomic(0, 1))
        assertEquals("server reported an unsent input terminal", failureOf { unsentTerminal.remap(1, BigInteger.ZERO, true) })

        val lostTerminal = InputState()
        lostTerminal.reserve(PendingTerminal(BigInteger.ZERO) { sent })
        lostTerminal.acknowledge(BigInteger.ZERO, true)
        assertEquals("server lost the input terminal", failureOf { lostTerminal.remap(1, BigInteger.ZERO, false) })

        val badChannel = InputState()
        assertEquals("invalid input channel", failureOf { badChannel.remap(0, BigInteger.ZERO, false) })

        val negative = InputState()
        assertEquals("input high-water is beyond sent data", failureOf { negative.remap(1, big(-1), false) })
    }

    @Test
    fun an_unacknowledged_terminal_is_replayed_until_it_is_acknowledged() {
        val state = InputState()
        val terminal = PendingTerminal(BigInteger.ZERO) { sent }
        state.reserve(terminal)
        assertTrue(state.isTerminal)
        val replay = assertNotNull(state.remap(4, BigInteger.ZERO, false))
        assertSame(terminal, replay.first)
        assertNull(state.remap(5, BigInteger.ZERO, true))
        assertEquals(0, state.pendingBytes)
    }

    @Test
    fun a_cancelled_input_may_be_remapped_without_its_terminal() {
        val state = InputState()
        state.reserve(atomic(0, 3))
        state.cancel()
        assertTrue(state.isTerminal)
        assertEquals("server lost the input terminal", failureOf { state.remap(1, BigInteger.ONE, false) })

        val local = InputState()
        local.reserve(atomic(0, 3))
        local.cancelLocal()
        assertNull(local.remap(1, BigInteger.ONE, false))
    }

    @Test
    fun a_cancel_releases_the_pending_charge() {
        val total = AtomicLong(0)
        val state = InputState(total)
        state.reserve(atomic(0, 12))
        state.cancel()
        assertEquals(0L, total.get())
        assertEquals(0, state.pendingBytes)
    }

    @Test
    fun an_ack_the_client_cannot_have_sent_is_rejected() {
        val state = InputState()
        state.reserve(atomic(0, 1))
        assertEquals("input ACK is beyond sent data", failureOf { state.acknowledge(big(2), false) })
        assertEquals("input ACK is beyond sent data", failureOf { state.acknowledge(big(-1), false) })
        assertEquals("input ACK reports an unsent terminal", failureOf { state.acknowledge(BigInteger.ONE, true) })

        val behind = InputState()
        behind.reserve(atomic(0, 1))
        behind.acknowledge(BigInteger.ONE, false)
        behind.reserve(atomic(1, 1))
        assertEquals("input ACK conflicts with pending range", failureOf { behind.acknowledge(BigInteger.ZERO, false) })
    }

    @Test
    fun a_second_range_cannot_be_reserved_while_one_is_pending_or_out_of_order() {
        val state = InputState()
        state.reserve(atomic(0, 1))
        assertEquals("conflicting input stream state", failureOf { state.reserve(atomic(1, 1)) })
        state.acknowledge(BigInteger.ONE, false)
        assertEquals("conflicting input stream state", failureOf { state.reserve(atomic(5, 1)) })
        state.reserve(PendingTerminal(BigInteger.ONE) { sent })
        state.acknowledge(BigInteger.ONE, true)
        assertEquals("conflicting input stream state", failureOf { state.reserve(atomic(1, 1)) })
    }

    @Test
    fun an_unacknowledged_charge_over_the_limit_is_rejected() {
        val total = AtomicLong(0)
        val state = InputState(total)
        assertEquals(
            "unacknowledged input exceeds the protocol limit",
            failureOf { state.reserve(atomic(0, StreamSessionProtocol.MAX_REPLAY_BYTES + 1)) },
        )
        assertEquals(0L, total.get())
        assertEquals(0, state.pendingBytes)
        assertEquals(BigInteger.ZERO, state.nextSequence)
        state.reserve(atomic(0, StreamSessionProtocol.MAX_REPLAY_BYTES))
        assertEquals(StreamSessionProtocol.MAX_REPLAY_BYTES.toLong(), total.get())
    }

    @Test
    fun the_session_wide_unacknowledged_input_is_capped_at_16_mib() {
        val mib = 1024 * 1024
        val total = AtomicLong(0)
        val a = InputState(total)
        val b = InputState(total)
        val c = InputState(total)
        a.reserve(atomic(0, 10 * mib))
        b.reserve(atomic(0, 6 * mib - 1))
        assertEquals(16L * mib - 1, total.get())
        c.reserve(atomic(0, 1))
        assertEquals(16L * mib, total.get())
        val d = InputState(total)
        assertEquals("unacknowledged input exceeds the protocol limit", failureOf { d.reserve(atomic(0, 1)) })
        assertEquals(16L * mib, total.get())
        a.acknowledge(BigInteger.ONE, false)
        d.reserve(atomic(0, 10 * mib))
        assertEquals(16L * mib, total.get())
        assertEquals("unacknowledged input exceeds the protocol limit", failureOf { InputState(total).reserve(atomic(0, 1)) })
    }

    @Test
    fun the_output_queue_is_bounded_by_items_and_bytes() {
        val items = Released()
        val byItems = OutputQueue<Int>(items.release)
        repeat(StreamSessionProtocol.MAX_QUEUED_ITEMS) { byItems.offer(step(it, 1)) }
        assertEquals(StreamSessionProtocol.MAX_QUEUED_ITEMS, byItems.queuedItemCount)
        assertEquals("output stream queue exceeded the protocol limit", failureOf { byItems.offer(step(0, 1)) })
        assertEquals(1L, items.total.get())
        assertEquals(StreamSessionProtocol.MAX_QUEUED_ITEMS, byItems.queuedItemCount)

        val bytes = Released()
        val byBytes = OutputQueue<Int>(bytes.release)
        byBytes.offer(step(0, StreamSessionProtocol.MAX_QUEUED_BYTES))
        assertEquals("output stream queue exceeded the protocol limit", failureOf { byBytes.offer(step(1, 1)) })
        assertEquals(1L, bytes.total.get())
        assertEquals(StreamSessionProtocol.MAX_QUEUED_BYTES.toLong(), byBytes.queuedByteCount)
    }

    @Test
    fun a_pulled_item_releases_its_charge() {
        val released = Released()
        val queue = OutputQueue<Int>(released.release)
        queue.offer(step(1, 5))
        queue.offer(step(2, 7))
        assertEquals(12L, queue.queuedByteCount)
        assertEquals(1, item(queue.pull()))
        assertEquals(5L, released.total.get())
        assertEquals(7L, queue.queuedByteCount)
        assertEquals(2, item(queue.pull()))
        assertEquals(12L, released.total.get())
        assertEquals(0L, queue.queuedByteCount)
    }

    @Test
    fun a_message_after_an_output_terminal_is_rejected() {
        val released = Released()
        val queue = OutputQueue<Int>(released.release)
        queue.offer(step(1, 3), isTerminal = true)
        assertTrue(queue.isTerminal)
        assertEquals("message received after output stream terminal", failureOf { queue.offer(step(2, 4)) })
        assertEquals(4L, released.total.get())

        val marked = OutputQueue<Int>(released.release)
        marked.markTerminal()
        assertEquals("message received after output stream terminal", failureOf { marked.offer(step(3, 2)) })
    }

    @Test
    fun a_queued_item_is_delivered_before_the_end_of_a_terminated_stream() {
        val queue = OutputQueue<Int>(Released().release)
        queue.offer(step(1), isTerminal = true)
        assertEquals(1, item(queue.pull()))
        assertSame(AgentStreamStep.End, queue.pull().getNow(null))
    }

    @Test
    fun a_waiting_pull_is_satisfied_by_a_later_offer() {
        val released = Released()
        val queue = OutputQueue<Int>(released.release)
        val first = queue.pull()
        val second = queue.pull()
        assertFalse(first.isDone)
        assertFalse(second.isDone)
        queue.offer(step(10, 3))
        assertEquals(10, item(first))
        assertFalse(second.isDone)
        queue.offer(step(20, 4))
        assertEquals(20, item(second))
        assertEquals(7L, released.total.get())
        assertEquals(0, queue.queuedItemCount)
    }

    @Test
    fun a_waiting_pull_is_not_bounded_by_the_queue_limits() {
        val queue = OutputQueue<Int>(Released().release)
        val waiting = queue.pull()
        queue.offer(step(1, StreamSessionProtocol.MAX_QUEUED_BYTES + 1))
        assertEquals(1, item(waiting))
    }

    @Test
    fun a_failing_item_surfaces_on_the_pull_that_reaches_it() {
        val queue = OutputQueue<Int>(Released().release)
        val thrown = IllegalStateException("decode failed")
        queue.offer(Delivery(1, { throw thrown }, null))
        queue.offer(Delivery(1, null, { AgentStreamError("code", "detail") }))
        queue.offer(Delivery(1, null, { throw thrown }))
        assertSame(thrown, failureOf(queue.pull()))
        assertEquals("code: detail", failureOf(queue.pull()).message)
        assertSame(thrown, failureOf(queue.pull()))
    }

    @Test
    fun a_failing_item_given_to_a_waiting_pull_fails_that_pull() {
        val queue = OutputQueue<Int>(Released().release)
        val waiting = queue.pull()
        queue.offer(Delivery(0, null, { AgentStreamCancelled("reason") }))
        assertEquals("stream cancelled: reason", failureOf(waiting).message)
    }

    @Test
    fun a_delivery_holds_exactly_one_of_a_step_and_an_error() {
        val message = "a delivery holds exactly one of a step and an error"
        assertEquals(message, failureOf { Delivery<Int>(0, null, null) })
        assertEquals(message, failureOf { Delivery<Int>(0, { AgentStreamStep.End }, { RuntimeException() }) })
    }

    @Test
    fun a_fail_discards_the_queue_and_is_reported_once() {
        val released = Released()
        val queue = OutputQueue<Int>(released.release)
        queue.offer(step(1, 4))
        queue.fail(AgentStreamError("boom", "bad"))
        assertEquals(4L, released.total.get())
        assertTrue(queue.isTerminal)
        assertEquals("boom: bad", failureOf(queue.pull()).message)
        assertSame(AgentStreamStep.End, queue.pull().getNow(null))
    }

    @Test
    fun a_fail_reaches_every_waiting_pull() {
        val queue = OutputQueue<Int>(Released().release)
        val a = queue.pull()
        val b = queue.pull()
        queue.fail(AgentStreamError("boom", "bad"))
        assertEquals("boom: bad", failureOf(a).message)
        assertEquals("boom: bad", failureOf(b).message)
        assertSame(AgentStreamStep.End, queue.pull().getNow(null))
    }

    @Test
    fun prepare_resume_rewinds_to_the_checkpoint_and_releases_the_queue() {
        val released = Released()
        val queue = OutputQueue<Int>(released.release)
        queue.accept(BigInteger.ZERO, big(3))
        queue.offer(step(1, 5))
        queue.offer(step(2, 6))
        queue.accept(big(3), big(2))
        assertEquals(big(5), queue.nextSequence)
        queue.checkpoint(big(3))
        queue.prepareResume()
        assertEquals(big(3), queue.nextSequence)
        assertEquals(11L, released.total.get())
        assertEquals(0, queue.queuedItemCount)
        assertEquals(0L, queue.queuedByteCount)
        assertFalse(queue.isTerminal)
        queue.accept(big(3), big(1))
        assertEquals(big(4), queue.nextSequence)
    }

    @Test
    fun prepare_resume_restores_the_checkpointed_terminal_and_clears_a_later_one() {
        val queue = OutputQueue<Int>(Released().release)
        queue.accept(BigInteger.ZERO, big(2))
        queue.checkpoint(big(2), terminal = true)
        queue.markTerminal()
        queue.prepareResume()
        assertTrue(queue.isTerminal)

        val unfinished = OutputQueue<Int>(Released().release)
        unfinished.accept(BigInteger.ZERO, big(2))
        unfinished.markTerminal()
        unfinished.prepareResume()
        assertFalse(unfinished.isTerminal)
        assertEquals(BigInteger.ZERO, unfinished.nextSequence)
    }

    @Test
    fun the_output_sequence_must_be_contiguous_and_the_checkpoint_monotonic() {
        val queue = OutputQueue<Int>(Released().release)
        assertEquals("output stream sequence conflict", failureOf { queue.accept(BigInteger.ONE, BigInteger.ONE) })
        queue.accept(BigInteger.ZERO, big(4))
        assertEquals("output stream sequence conflict", failureOf { queue.accept(big(3), BigInteger.ONE) })
        assertEquals("output checkpoint sequence conflict", failureOf { queue.checkpoint(big(5)) })
        queue.checkpoint(big(2))
        assertEquals("output checkpoint sequence conflict", failureOf { queue.checkpoint(BigInteger.ONE) })
        queue.markTerminal()
        assertEquals("output stream sequence conflict", failureOf { queue.accept(big(4), BigInteger.ONE) })
    }

    @Test
    fun an_output_sequence_past_u64_is_rejected() {
        val queue = OutputQueue<Int>(Released().release)
        assertEquals("stream sequence overflow", failureOf { queue.accept(BigInteger.ZERO, StreamSessionProtocol.MAX_U64.add(BigInteger.ONE)) })
        assertEquals(BigInteger.ZERO, queue.nextSequence)
    }

    @Test
    fun an_output_stream_may_not_change_its_lane() {
        val observed = OutputQueue<Int>(Released().release)
        observed.observeLane("json")
        observed.observeLane("json")
        assertEquals("output stream lane changed", failureOf { observed.observeLane("u8") })
        assertEquals("output stream lane changed", failureOf { observed.setDecoder({ 0 }, "u8") })

        val expected = OutputQueue<Int>(Released().release)
        expected.setDecoder({ 0 }, "json")
        assertEquals("output stream lane changed", failureOf { expected.observeLane("u8") })
        assertEquals("output stream lane changed", failureOf { expected.setDecoder({ 1 }, "u8") })
        expected.observeLane("json")
        expected.setDecoder({ 2 }, "json")
        assertEquals(2, expected.decode!!(SchemaValue.Bool(true)))
    }

    @Test
    fun an_output_stream_is_exposed_once() {
        val queue = OutputQueue<Int>(Released().release)
        queue.expose()
        assertEquals("output stream was exposed more than once", failureOf { queue.expose() })
    }

    @Test
    fun cancel_and_drop_are_idempotent_and_never_throw_from_any_state() {
        val fresh = OutputQueue<Int>(Released().release)
        fresh.cancel().join()
        fresh.cancel().join()
        fresh.drop().join()
        assertSame(AgentStreamStep.End, fresh.pull().getNow(null))

        val dropped = OutputQueue<Int>(Released().release)
        dropped.drop().join()
        dropped.drop().join()
        dropped.cancel().join()

        val terminated = OutputQueue<Int>(Released().release)
        terminated.offer(step(1), isTerminal = true)
        terminated.cancel().join()
        terminated.drop().join()
        terminated.cancel().join()
        assertSame(AgentStreamStep.End, terminated.pull().getNow(null))

        val failed = OutputQueue<Int>(Released().release)
        failed.fail(AgentStreamError("boom", "bad"))
        failed.cancel().join()
        failed.drop().join()
    }

    @Test
    fun a_cancel_sends_the_remote_cancel_once_with_its_reason() {
        val reasons = mutableListOf<String>()
        val queue = OutputQueue<Int>(Released().release)
        queue.cancelWith = { reason -> reasons.add(reason); sent }
        queue.cancel().join()
        queue.cancel().join()
        queue.drop().join()
        assertEquals(listOf("cancelled"), reasons)

        val dropReasons = mutableListOf<String>()
        val abandoned = OutputQueue<Int>(Released().release)
        abandoned.cancelWith = { reason -> dropReasons.add(reason); sent }
        abandoned.drop().join()
        assertEquals(listOf("consumer-drop"), dropReasons)
    }

    @Test
    fun a_cancel_whose_remote_send_fails_does_not_throw() {
        val throwing = OutputQueue<Int>(Released().release)
        throwing.cancelWith = { throw IllegalStateException("socket closed") }
        throwing.cancel().join()
        throwing.drop().join()

        val failing = OutputQueue<Int>(Released().release)
        failing.cancelWith = { CompletableFuture.failedFuture(IllegalStateException("socket closed")) }
        failing.cancel().join()
        failing.cancel().join()
    }

    @Test
    fun a_cancel_after_the_terminal_sends_nothing() {
        val reasons = mutableListOf<String>()
        val queue = OutputQueue<Int>(Released().release)
        queue.cancelWith = { reasons.add(it); sent }
        queue.markTerminal()
        queue.cancel().join()
        assertEquals(emptyList(), reasons)
        assertNull(queue.remapCancel { reasons.add(it); sent })
        assertEquals(emptyList(), reasons)
    }

    @Test
    fun a_cancel_releases_queued_items_ends_waiting_pulls_and_discards_later_offers() {
        val released = Released()
        val queue = OutputQueue<Int>(released.release)
        queue.offer(step(1, 5))
        queue.offer(step(2, 6))
        queue.cancel().join()
        assertEquals(11L, released.total.get())
        assertEquals(0L, queue.queuedByteCount)
        assertSame(AgentStreamStep.End, queue.pull().getNow(null))
        queue.offer(step(3, 9))
        assertEquals(20L, released.total.get())
        assertEquals(0, queue.queuedItemCount)
        assertFalse(queue.isTerminal)

        val waiter = OutputQueue<Int>(Released().release)
        val waiting = waiter.pull()
        waiter.drop().join()
        assertSame(AgentStreamStep.End, waiting.getNow(null))
    }

    @Test
    fun a_cancel_that_could_not_be_sent_is_resent_after_a_remap() {
        val queue = OutputQueue<Int>(Released().release)
        queue.cancelWith = { CompletableFuture.failedFuture(IllegalStateException("socket closed")) }
        queue.cancel().join()
        val reasons = mutableListOf<String>()
        val resent = queue.remapCancel { reasons.add(it); sent }
        assertNotNull(resent).join()
        assertEquals(listOf("cancelled"), reasons)
        assertNull(OutputQueue<Int>(Released().release).remapCancel { sent })
    }

    @Test
    fun a_terminal_clears_a_cancel_still_waiting_to_be_resent() {
        val queue = OutputQueue<Int>(Released().release)
        queue.cancelWith = { CompletableFuture.failedFuture(IllegalStateException("socket closed")) }
        queue.drop().join()
        queue.markTerminal()
        assertNull(queue.remapCancel { sent })
    }

    @Test
    fun an_item_continued_after_acceptance_is_delivered_first_or_declined_when_stopped() {
        val released = Released()
        val queue = OutputQueue<Int>(released.release)
        queue.offer(step(2, 1))
        assertTrue(queue.continueAccepted(step(1, 4)))
        assertEquals(5L, queue.queuedByteCount)
        assertEquals(1, item(queue.pull()))
        assertEquals(2, item(queue.pull()))
        assertEquals(5L, released.total.get())

        val waiting = queue.pull()
        assertTrue(queue.continueAccepted(step(3, 2)))
        assertEquals(3, item(waiting))
        assertEquals(7L, released.total.get())

        queue.cancel().join()
        assertFalse(queue.continueAccepted(step(4, 8)))
        assertEquals(7L, released.total.get())
    }

    @Test
    fun caller_supplied_code_runs_without_the_queue_lock() {
        lateinit var queue: OutputQueue<Int>
        val releaseSaw = AtomicInteger(-1)
        queue = OutputQueue<Int>({
            val reader = Thread { releaseSaw.set(queue.queuedItemCount) }
            reader.start()
            reader.join(5000)
        })
        val waiting = queue.pull()
        val observed = AtomicInteger(-1)
        waiting.thenAccept {
            val reader = Thread { observed.set(queue.queuedItemCount) }
            reader.start()
            reader.join(5000)
        }
        val offering = Thread { queue.offer(step(1, 1)) }
        offering.start()
        offering.join(10000)
        assertFalse(offering.isAlive)
        assertEquals(0, observed.get())
        assertEquals(0, releaseSaw.get())

        val thunkSawLock = AtomicInteger(-1)
        queue.offer(Delivery(0, {
            val reader = Thread { thunkSawLock.set(queue.queuedItemCount) }
            reader.start()
            reader.join(5000)
            AgentStreamStep.Item(2)
        }, null))
        assertEquals(2, item(queue.pull()))
        assertEquals(0, thunkSawLock.get())
    }
}
