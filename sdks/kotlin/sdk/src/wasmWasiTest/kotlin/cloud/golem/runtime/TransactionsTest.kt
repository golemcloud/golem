package cloud.golem.runtime

import cloud.golem.runtime.host.PredicateValue
import cloud.golem.runtime.host.RetryDecision
import cloud.golem.runtime.host.RetryPolicy
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

/** The saga helpers' pure logic (no host calls): compensation ordering and failure classification. */
class TransactionsTest {
    private val log = mutableListOf<String>()

    private fun op(name: String, fails: Boolean = false, compensationFails: Boolean = false) = Operation<Int, Int, String>(
        run = { input ->
            log += "run $name"
            if (fails) Either.Left("$name failed") else Either.Right(input + 1)
        },
        compensateFn = { _, _ ->
            log += "undo $name"
            if (compensationFails) Either.Left("$name undo failed") else Either.Right(Unit)
        },
    )

    @Test
    fun fallible_success_registers_no_rollback() {
        val result = runFallibleTransaction<Int, String> { tx ->
            val a = tx.execute(op("a"), 1) as Either.Right
            tx.execute(op("b"), a.value)
        }
        assertEquals(Either.Right(3), result)
        assertEquals(listOf("run a", "run b"), log)
    }

    @Test
    fun fallible_failure_compensates_in_reverse_order() {
        val result = runFallibleTransaction<Int, String> { tx ->
            val a = tx.execute(op("a"), 1) as Either.Right
            val b = tx.execute(op("b"), a.value) as Either.Right
            tx.execute(op("c", fails = true), b.value)
        }
        assertEquals(Either.Left(TransactionFailure.FailedAndRolledBackCompletely("c failed")), result)
        assertEquals(listOf("run a", "run b", "run c", "undo b", "undo a"), log)
    }

    @Test
    fun fallible_compensation_failure_stops_the_rollback() {
        val result = runFallibleTransaction<Int, String> { tx ->
            val a = tx.execute(op("a"), 1) as Either.Right
            val b = tx.execute(op("b", compensationFails = true), a.value) as Either.Right
            tx.execute(op("c", fails = true), b.value)
        }
        val failure = (result as Either.Left).value
        assertTrue(failure is TransactionFailure.FailedAndRolledBackPartially)
        assertEquals("c failed", failure.error)
        assertEquals("b undo failed", failure.compensationFailure)
        assertEquals(listOf("run a", "run b", "run c", "undo b"), log) // `a` is never compensated
    }

    @Test
    fun uuid_formats_canonically() {
        val uuid = Uuid(0x0123456789abcdefL, -0x0123456789abcdf0L) // low = 0xfedcba9876543210
        assertEquals("01234567-89ab-cdef-fedc-ba9876543210", uuid.toCanonicalString())
        assertEquals("00000000-0000-0000-0000-000000000000", Uuid(0L, 0L).toCanonicalString())
    }

    @Test
    fun infallible_attempt_limit_must_be_in_range() {
        assertFailsWith<IllegalArgumentException> { requireAttemptsMax(0) }
        assertFailsWith<IllegalArgumentException> { requireAttemptsMax(INFALLIBLE_ATTEMPTS_MAX + 1) }
        requireAttemptsMax(1)
        requireAttemptsMax(INFALLIBLE_ATTEMPTS_MAX)
    }

    // The saga host of the Rust unit tests: the retry policy allows two retries, 10 ns apart, and the state carries the attempt number.
    private class FakeSagaHost : SagaHost {
        var clock = 0L
        var elapsed = 0L
        var policyError = false
        var resolveError = false

        override fun now(): Long = clock

        override fun waitFor(nanos: Long) {
            clock += nanos
        }

        override fun noun(): Either<String, String> = Either.Right("urn:worker:component/agent")

        override fun resolve(verb: String, noun: String, properties: List<Pair<String, PredicateValue>>): Either<String, RetryPolicy?> {
            assertEquals("saga", verb)
            assertEquals("urn:worker:component/agent", noun)
            assertEquals("error-kind", properties[1].first)
            assertEquals(PredicateValue.Text("operation-error"), properties[1].second)
            if (resolveError) {
                resolveError = false
                return Either.Left("invalid resolver predicate")
            }
            return Either.Right(null)
        }

        override fun next(
            policy: RetryPolicy?,
            state: ByteArray,
            elapsedNanos: Long,
            verb: String,
            noun: String,
            properties: List<Pair<String, PredicateValue>>,
        ): Either<String, RetryDecision> {
            assertEquals(null, policy)
            elapsed = elapsedNanos
            val resolved = resolve(verb, noun, properties)
            if (resolved is Either.Left) return resolved
            assertEquals("attempt", properties[0].first)
            val attempt = (properties[0].second as PredicateValue.Integer).value
            if (policyError) {
                policyError = false
                return Either.Left("invalid policy")
            }
            assertEquals(if (attempt == 1L) emptyList() else listOf((attempt - 1).toByte()), state.toList())
            assertTrue(elapsedNanos >= (attempt - 1) * 10)
            return Either.Right(RetryDecision(byteArrayOf(attempt.toByte()), if (attempt < 3) 10L else null))
        }
    }

    private fun <A> failingBody(tx: FallibleTransaction<String>, op: Operation<Unit, Unit, String>, error: String, afterOp: () -> Unit = {}): Either<String, A> {
        val r = tx.execute(op, Unit)
        if (r is Either.Left) return r
        afterOp()
        return Either.Left(error)
    }

    @Test
    fun bounded_retries_wait_and_exhaust() {
        val host = FakeSagaHost()
        val op = Operation<Unit, Unit, String>(
            run = {
                host.clock += 2
                Either.Right(Unit)
            },
            compensateFn = { _, _ ->
                host.clock += 3
                Either.Right(Unit)
            },
        )
        var attempts = 0
        val result = runInfallibleTransaction<Unit, String>(host, INFALLIBLE_ATTEMPTS_MAX) { tx ->
            attempts++
            failingBody(tx, op, "operation")
        }
        assertEquals(Either.Left(TransactionFailure.RetryExhausted("operation")), result)
        assertEquals(3, attempts)
        assertEquals(35L, host.clock)
        assertEquals(35L, host.elapsed)
    }

    @Test
    fun resolver_error_preserves_original_failure() {
        val host = FakeSagaHost().apply { resolveError = true }
        var attempts = 0
        val result = runInfallibleTransaction<Unit, String>(host, INFALLIBLE_ATTEMPTS_MAX) {
            attempts++
            Either.Left("original")
        }
        assertEquals(Either.Left(TransactionFailure.RetryPolicyError("original", "invalid resolver predicate")), result)
        assertEquals(1, attempts)
    }

    @Test
    fun invalid_policy_never_retries() {
        val host = FakeSagaHost().apply { policyError = true }
        var attempts = 0
        val result = runInfallibleTransaction<Unit, String>(host, INFALLIBLE_ATTEMPTS_MAX) {
            attempts++
            Either.Left("operation")
        }
        assertEquals(Either.Left(TransactionFailure.RetryPolicyError("operation", "invalid policy")), result)
        assertEquals(1, attempts)
    }

    @Test
    fun infallible_stops_on_compensation_failure() {
        val op = Operation<Unit, Unit, String>(run = { Either.Right(Unit) }, compensateFn = { _, _ -> Either.Left("compensation") })
        var attempts = 0
        val result = runInfallibleTransaction<Unit, String>(FakeSagaHost(), INFALLIBLE_ATTEMPTS_MAX) { tx ->
            attempts++
            assertEquals(1, attempts)
            failingBody(tx, op, "body")
        }
        assertEquals(Either.Left(TransactionFailure.FailedAndRolledBackPartially("body", "compensation")), result)
    }

    @Test
    fun compensation_runs_in_reverse_order_and_stops_at_the_first_failure() {
        val compensated = mutableListOf<Int>()
        val op = Operation<Int, Int, String>(
            run = { Either.Right(it) },
            compensateFn = { input, output ->
                assertEquals(input, output)
                compensated += input
                if (input == 2) Either.Left("compensation") else Either.Right(Unit)
            },
        )
        val result = runFallibleTransaction<Unit, String> { tx ->
            tx.execute(op, 1)
            tx.execute(op, 2)
            tx.execute(op, 3)
            Either.Left("body")
        }
        assertEquals(Either.Left(TransactionFailure.FailedAndRolledBackPartially("body", "compensation")), result)
        assertEquals(listOf(3, 2), compensated)
    }

    @Test
    fun infallible_retries_with_fresh_compensations() {
        val log = mutableListOf<String>()
        val op1 = Operation<String, Unit, String>(
            run = {
                log += "op1 execute $it"
                Either.Right(Unit)
            },
            compensateFn = { input, _ ->
                log += "op1 rollback $input"
                Either.Right(Unit)
            },
        )
        var op2Calls = 0
        val op2 = Operation<Unit, Unit, String>(
            run = {
                log += "op2 execute"
                op2Calls++
                if (op2Calls < 3) Either.Left("op2 error") else Either.Right(Unit)
            },
            compensateFn = { _, r ->
                log += "op2 rollback $r"
                Either.Right(Unit)
            },
        )
        val result = runInfallibleTransaction<Int, String>(FakeSagaHost(), INFALLIBLE_ATTEMPTS_MAX) { tx ->
            val steps = listOf<() -> Either<String, Unit>>(
                { tx.execute(op1, "hello") },
                { tx.execute(op1, "world") },
                { tx.execute(op2, Unit) },
            )
            steps.firstNotNullOfOrNull { (it() as? Either.Left) } ?: Either.Right(11)
        }
        assertEquals(Either.Right(11), result)
        assertEquals(3, op2Calls)
        val attempt = listOf("op1 execute hello", "op1 execute world", "op2 execute")
        val rollback = listOf("op1 rollback world", "op1 rollback hello")
        assertEquals(attempt + rollback + attempt + rollback + attempt, log)
    }
}
