package cloud.golem.runtime

import cloud.golem.runtime.host.PredicateValue
import cloud.golem.runtime.host.RetryApi
import cloud.golem.runtime.host.RetryDecision
import cloud.golem.runtime.host.RetryPolicy
import cloud.golem.runtime.wasi.MonotonicClock

// Native port of the saga transaction helpers (Scala's Transactions.scala / Rust's `transaction`
// module) for Golem 1.6.0.
//
// These are *sagas*, not atomic regions. Golem 1.6.0 removed atomic regions and oplog-index
// rewinding: attempts and compensations append to the oplog, they never rewind execution, and they
// do not undo concurrent work. Successful steps compensate in reverse order.
//
// ==Fallible Transactions==
// [Transactions.fallibleTransaction]: when an operation fails, the registered compensations run and
// the failure is returned with its rollback status.
//
// ==Infallible Transactions==
// [Transactions.infallibleTransaction]: runs new attempts until the body succeeds. After each fully
// compensated failure the host resolves the `saga` retry policy (verb `saga`, the agent's URI, and
// properties `attempt` -- 1 for the first failure -- and `error-kind` = `operation-error`) and either
// supplies a delay before the next attempt or reports the policy exhausted. Partial compensation stops
// the saga without another attempt. Exceptions thrown by the body are programming errors: they
// propagate without compensation (the invocation fails and Golem's own retry handling applies).

/** Minimal local Either -- this SDK has no Arrow/stdlib Either dependency. */
sealed class Either<out L, out R> {
    data class Left<out L>(val value: L) : Either<L, Nothing>()
    data class Right<out R>(val value: R) : Either<Nothing, R>()
}

/** Describes how a transaction failed. */
sealed class TransactionFailure<out Err> {
    /** Every successful operation was compensated and the saga retry policy is exhausted. */
    data class RetryExhausted<Err>(val error: Err) : TransactionFailure<Err>()

    /** Every successful operation was compensated but the host rejected the retry policy or continuation. */
    data class RetryPolicyError<Err>(val failure: Err, val message: String) : TransactionFailure<Err>()

    data class FailedAndRolledBackCompletely<Err>(val error: Err) : TransactionFailure<Err>()
    data class FailedAndRolledBackPartially<Err>(val error: Err, val compensationFailure: Err) : TransactionFailure<Err>()
}

/**
 * The host-free core of [Transactions.fallibleTransaction] (kept top-level so unit tests can exercise
 * the compensation logic without pulling in the retry/clock host imports `Transactions` references).
 */
internal fun <A, Err> runFallibleTransaction(body: (FallibleTransaction<Err>) -> Either<Err, A>): Either<TransactionFailure<Err>, A> {
    val tx = FallibleTransaction<Err>()
    return when (val result = body(tx)) {
        is Either.Right -> result
        is Either.Left -> Either.Left(tx.onFailure(result.value))
    }
}

/** An operation with execute and compensate steps. */
class Operation<In, Out, Err>(
    private val run: (In) -> Either<Err, Out>,
    private val compensateFn: (In, Out) -> Either<Err, Unit>,
) {
    fun execute(input: In): Either<Err, Out> = run(input)
    fun compensate(input: In, output: Out): Either<Err, Unit> = compensateFn(input, output)
}

/** The most attempts [Transactions.infallibleTransaction] makes whatever the host retry policy says. */
const val INFALLIBLE_ATTEMPTS_MAX = 10_000L

internal fun requireAttemptsMax(attemptsMax: Long) {
    require(attemptsMax in 1..INFALLIBLE_ATTEMPTS_MAX) { "attemptsMax must be in 1..$INFALLIBLE_ATTEMPTS_MAX" }
}

object Transactions {
    /** Creates an [Operation] from execute and compensate functions. */
    fun <In, Out, Err> operation(
        run: (In) -> Either<Err, Out>,
        compensate: (In, Out) -> Either<Err, Unit>,
    ): Operation<In, Out, Err> = Operation(run, compensate)

    /**
     * Runs a transaction that returns errors instead of retrying. When [body] returns an error the
     * registered compensations run in reverse order (best-effort) and the failure is returned with
     * its rollback status.
     */
    fun <A, Err> fallibleTransaction(body: (FallibleTransaction<Err>) -> Either<Err, A>): Either<TransactionFailure<Err>, A> = runFallibleTransaction(body)

    /**
     * Runs new attempts of [body] until it succeeds or [attemptsMax] attempts failed (the host retry policy may stop it sooner). Each failed attempt compensates the successful
     * operations in reverse order before the next one; attempts append durable history and never
     * rewind it. After each fully compensated failure the host resolves the `saga` retry policy (see
     * the file header) -- the delay it returns is waited out with a durable sleep. Exhaustion, policy
     * errors and partial compensation are returned as typed [TransactionFailure]s.
     */
    fun <A, Err> infallibleTransaction(
        attemptsMax: Long = INFALLIBLE_ATTEMPTS_MAX,
        body: (FallibleTransaction<Err>) -> Either<Err, A>,
    ): Either<TransactionFailure<Err>, A> = runInfallibleTransaction(RealSagaHost, attemptsMax, body)
}

/** What an infallible transaction needs from the host: the clock and the `saga` retry policy resolution. Tests substitute a fake. */
internal interface SagaHost {
    fun now(): Long

    fun waitFor(nanos: Long)

    fun noun(): Either<String, String>

    fun resolve(verb: String, noun: String, properties: List<Pair<String, PredicateValue>>): Either<String, RetryPolicy?>

    fun next(
        policy: RetryPolicy?,
        state: ByteArray,
        elapsedNanos: Long,
        verb: String,
        noun: String,
        properties: List<Pair<String, PredicateValue>>,
    ): Either<String, RetryDecision>
}

/** The host behind [Transactions.infallibleTransaction]; kept apart from [SagaHost] so that tests of the transaction logic do not link the host imports. */
internal object RealSagaHost : SagaHost {
    override fun now(): Long = MonotonicClock.now()

    override fun waitFor(nanos: Long) = MonotonicClock.waitFor(nanos)

    override fun noun(): Either<String, String> = try {
        val id = HostApi.getSelfMetadata().agentId
        Either.Right("urn:worker:${id.componentId.uuid.toCanonicalString()}/${id.agentId}")
    } catch (e: AgentOperationException) {
        Either.Left(e.message ?: "agent operation failed")
    }

    override fun resolve(verb: String, noun: String, properties: List<Pair<String, PredicateValue>>): Either<String, RetryPolicy?> = RetryApi.resolveRetryPolicy(verb, noun, properties)

    override fun next(
        policy: RetryPolicy?,
        state: ByteArray,
        elapsedNanos: Long,
        verb: String,
        noun: String,
        properties: List<Pair<String, PredicateValue>>,
    ): Either<String, RetryDecision> = RetryApi.nextRetryDelay(policy, state, elapsedNanos, verb, noun, properties)
}

/** The host-free core of [Transactions.infallibleTransaction], kept top-level for the same reason as [runFallibleTransaction]. */
internal fun <A, Err> runInfallibleTransaction(
    host: SagaHost,
    attemptsMax: Long,
    body: (FallibleTransaction<Err>) -> Either<Err, A>,
): Either<TransactionFailure<Err>, A> {
    requireAttemptsMax(attemptsMax)
    val started = host.now()
    var state = ByteArray(0)
    var attempt = 0L
    while (true) {
        val tx = FallibleTransaction<Err>()
        val error = when (val result = body(tx)) {
            is Either.Right -> return result
            is Either.Left -> result.value
        }
        val failure = tx.onFailure(error)
        if (failure !is TransactionFailure.FailedAndRolledBackCompletely) return Either.Left(failure)
        attempt++
        if (attempt >= attemptsMax) return Either.Left(TransactionFailure.RetryExhausted(error))
        val decision = nextDecision(host, state, host.now() - started, attempt)
        if (decision is Either.Left) return Either.Left(TransactionFailure.RetryPolicyError(error, decision.value))
        val next = (decision as Either.Right).value
        state = next.state
        val delay = next.delayNanos ?: return Either.Left(TransactionFailure.RetryExhausted(error))
        host.waitFor(delay)
    }
}

private fun nextDecision(host: SagaHost, state: ByteArray, elapsedNanos: Long, attempt: Long): Either<String, RetryDecision> {
    val noun = when (val n = host.noun()) {
        is Either.Left -> return n
        is Either.Right -> n.value
    }
    val properties = listOf(
        "attempt" to PredicateValue.Integer(attempt),
        "error-kind" to PredicateValue.Text("operation-error"),
    )
    val policy = when (val resolved = host.resolve("saga", noun, properties)) {
        is Either.Left -> return resolved
        is Either.Right -> resolved.value
    }
    return host.next(policy, state, elapsedNanos, "saga", noun, properties)
}

/** Transaction context: operations are executed through it so that their compensations are registered. */
class FallibleTransaction<Err> internal constructor() {
    private val compensations = mutableListOf<() -> Either<Err, Unit>>()

    /** Executes [operation]. On success registers its compensation; on failure returns the error
     * (the enclosing transaction helper then rolls back). */
    fun <In, Out> execute(operation: Operation<In, Out, Err>, input: In): Either<Err, Out> = when (val result = operation.execute(input)) {
        is Either.Right -> {
            compensations.add(0) { operation.compensate(input, result.value) }
            result
        }
        is Either.Left -> result
    }

    /** Runs all registered compensations in reverse order, stopping at the first compensation failure. */
    internal fun onFailure(error: Err): TransactionFailure<Err> {
        for (c in compensations) {
            when (val r = c()) {
                is Either.Left -> return TransactionFailure.FailedAndRolledBackPartially(error, r.value)
                is Either.Right -> {}
            }
        }
        return TransactionFailure.FailedAndRolledBackCompletely(error)
    }
}
