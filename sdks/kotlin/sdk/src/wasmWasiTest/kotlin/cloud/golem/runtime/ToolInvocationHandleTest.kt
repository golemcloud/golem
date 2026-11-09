package cloud.golem.runtime

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertIs
import kotlin.test.assertSame
import kotlin.test.assertTrue

class ToolInvocationHandleTest {
    private class FakePort(val label: String, val log: MutableList<String>) : CallPort {
        var cancelled = false
        var released = 0
        var stdoutSink: ByteSink? = null
        var stderrSink: ByteSink? = null

        override fun collect(stdout: ByteSink?, stderr: ByteSink?): UnderlyingOutcome {
            log.add("collect $label")
            stdoutSink = stdout
            stderrSink = stderr
            return if (cancelled) UnderlyingOutcome.Err(UnderlyingError.Cancelled) else ok(label)
        }

        override fun cancel() {
            log.add("cancel $label")
            cancelled = true
        }

        override fun release() {
            released++
        }
    }

    private class Sink : ByteSink {
        override fun write(bytes: ByteArray): Boolean = true
    }

    companion object {
        fun ok(text: String) = UnderlyingOutcome.Ok(ToolUnderlyingResult(TypedSchemaValue("string", SchemaValue.Str(text)), null, null))
    }

    @Test
    fun two_started_calls_overlap() {
        val log = ArrayList<String>()
        val a = FakePort("a", log)
        val b = FakePort("b", log)
        val first = StartedInvocation(a)
        val second = StartedInvocation(b)
        assertEquals(emptyList(), log)
        assertEquals(ok("b").value.result, assertIs<UnderlyingOutcome.Ok>(second.await()).value.result)
        assertEquals(ok("a").value.result, assertIs<UnderlyingOutcome.Ok>(first.await()).value.result)
        assertEquals(listOf("collect b", "collect a"), log)
    }

    @Test
    fun cancel_yields_a_cancelled_outcome() {
        val log = ArrayList<String>()
        val call = StartedInvocation(FakePort("a", log))
        call.cancel()
        assertEquals(UnderlyingError.Cancelled, assertIs<UnderlyingOutcome.Err>(call.await()).error)
        assertEquals(listOf("cancel a", "collect a"), log)
    }

    @Test
    fun cancelling_twice_asks_the_tool_once() {
        val log = ArrayList<String>()
        val call = StartedInvocation(FakePort("a", log))
        call.cancel()
        call.cancel()
        assertEquals(listOf("cancel a"), log)
    }

    @Test
    fun awaiting_twice_returns_the_same_outcome_and_collects_once() {
        val log = ArrayList<String>()
        val call = StartedInvocation(FakePort("a", log))
        val first = call.await()
        assertSame(first, call.await())
        assertEquals(listOf("collect a"), log)
    }

    @Test
    fun a_cancel_after_completion_is_ignored() {
        val log = ArrayList<String>()
        val call = StartedInvocation(FakePort("a", log))
        call.await()
        call.cancel()
        assertEquals(listOf("collect a"), log)
    }

    @Test
    fun the_host_handles_are_released_when_the_call_completes() {
        val port = FakePort("a", ArrayList())
        val call = StartedInvocation(port)
        call.await()
        call.release()
        assertEquals(1, port.released)
    }

    @Test
    fun a_revoked_call_that_was_never_awaited_cannot_be_awaited() {
        val port = FakePort("a", ArrayList())
        val call = StartedInvocation(port)
        call.release()
        assertEquals(1, port.released)
        assertFailsWith<IllegalStateException> { call.await() }
    }

    @Test
    fun an_await_after_a_failed_collect_rethrows_without_collecting_again() {
        val log = ArrayList<String>()
        val boom = IllegalStateException("stream broke")
        val port = object : CallPort {
            var released = 0
            override fun collect(stdout: ByteSink?, stderr: ByteSink?): UnderlyingOutcome {
                log.add("collect")
                throw boom
            }

            override fun cancel() {}

            override fun release() {
                released++
            }
        }
        val call = StartedInvocation(port)
        assertSame(boom, assertFailsWith<IllegalStateException> { call.await() })
        assertSame(boom, assertFailsWith<IllegalStateException> { call.await() })
        assertEquals(listOf("collect"), log)
        assertEquals(1, port.released)
    }

    @Test
    fun a_revoked_call_keeps_its_outcome() {
        val call = StartedInvocation(FakePort("a", ArrayList()))
        val outcome = call.await()
        call.release()
        assertSame(outcome, call.await())
    }

    @Test
    fun forwarded_outputs_reach_the_collection() {
        val port = FakePort("a", ArrayList())
        val call = StartedInvocation(port)
        val out = Sink()
        call.forwardOutputs(out, null)
        call.await()
        assertSame(out, port.stdoutSink)
        assertEquals(null, port.stderrSink)
    }

    @Test
    fun outputs_cannot_be_forwarded_after_the_call_finished() {
        val call = StartedInvocation(FakePort("a", ArrayList()))
        call.await()
        assertFailsWith<IllegalStateException> { call.forwardOutputs(Sink(), null) }
    }

    @Test
    fun a_typed_invocation_decodes_the_outcome_and_delegates_cancel() {
        val log = ArrayList<String>()
        val typed = TypedInvocation<Nothing, String>(StartedInvocation(FakePort("a", log)), { _, _ -> null }) { (it as SchemaValue.Str).v }
        typed.cancel()
        assertEquals(Either.Left(ToolInvokeError.Cancelled), typed.await())
        assertTrue(log.contains("cancel a"))
    }

    @Test
    fun a_typed_invocation_with_outputs_carries_them() {
        val handle = object : ToolInvocationHandle {
            override fun await() = UnderlyingOutcome.Ok(
                ToolUnderlyingResult(TypedSchemaValue("string", SchemaValue.Str("v")), "o".encodeToByteArray(), null),
            )

            override fun cancel() {}

            override fun forwardOutputs(stdout: ByteSink?, stderr: ByteSink?) {}
        }
        val typed = TypedInvocationWithOutputs<Nothing, String>(handle, { _, _ -> null }) { (it as SchemaValue.Str).v }
        val ok = assertIs<Either.Right<WithOutputs<String>>>(typed.await()).value
        assertEquals("v", ok.value)
        assertEquals("o", ok.stdout?.decodeToString())
    }
}
