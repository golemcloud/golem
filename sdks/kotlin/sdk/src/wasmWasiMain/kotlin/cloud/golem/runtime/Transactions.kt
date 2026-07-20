package cloud.golem.runtime

// Native port of Scala's Transactions.scala: compensating-transaction (saga) helpers built
// entirely on already-existing HostApi/Guards primitives (getOplogIndex/setOplogIndex,
// markAtomicOperation, trap) -- no new WIT surface, no new host imports. Synchronous throughout
// for the same reason as Guards.kt: none of the underlying host calls are actually async, so
// Scala's Future-based structure (a Scala.js environment artifact) doesn't carry over; this SDK
// has no coroutines dependency yet and doesn't need one for this.
//
// ==Infallible Transactions==
// Use [infallibleTransaction] when operations must eventually succeed. On failure,
// compensations run and the transaction retries automatically.
//
// ==Fallible Transactions==
// Use [fallibleTransaction] when you want explicit error handling instead of automatic retry.

/** Minimal local Either -- this SDK has no Arrow/stdlib Either dependency and this is its first use. */
sealed class Either<out L, out R> {
    data class Left<out L>(val value: L) : Either<L, Nothing>()
    data class Right<out R>(val value: R) : Either<Nothing, R>()
}

/** Describes how a fallible transaction failed. */
sealed class TransactionFailure<out Err> {
    data class FailedAndRolledBackCompletely<Err>(val error: Err) : TransactionFailure<Err>()
    data class FailedAndRolledBackPartially<Err>(val error: Err, val compensationFailure: Err) : TransactionFailure<Err>()
}

/** An atomic operation with execute and compensate steps. */
class Operation<In, Out, Err>(
    private val run: (In) -> Either<Err, Out>,
    private val compensateFn: (In, Out) -> Either<Err, Unit>,
) {
    fun execute(input: In): Either<Err, Out> = run(input)
    fun compensate(input: In, output: Out): Either<Err, Unit> = compensateFn(input, output)
}

object Transactions {
    /** Creates an [Operation] from execute and compensate functions. */
    fun <In, Out, Err> operation(
        run: (In) -> Either<Err, Out>,
        compensate: (In, Out) -> Either<Err, Unit>,
    ): Operation<In, Out, Err> = Operation(run, compensate)

    // The signal InfallibleTransaction.rollback() throws to request a retry from the
    // infallibleTransaction loop below, distinguished by identity from any other (unexpected)
    // exception -- those trap instead of retrying, matching the Scala source exactly.
    private object RetrySignal : RuntimeException()

    /**
     * Runs a transaction that retries on failure. When any operation fails: all registered
     * compensations run in reverse order, the oplog index resets to the transaction start, and
     * the entire transaction body re-executes. Keeps retrying until all operations succeed.
     */
    fun <A> infallibleTransaction(body: (InfallibleTransaction) -> A): A {
        while (true) {
            val guard = Guards.markAtomicOperation()
            val begin = HostApi.getOplogIndex()
            val tx = InfallibleTransaction()
            try {
                val result = body(tx)
                guard.drop()
                return result
            } catch (e: Throwable) {
                if (e === RetrySignal) {
                    HostApi.setOplogIndex(begin)
                    guard.drop()
                    continue
                }
                HostApi.trap("infallibleTransaction failed: ${e.stackTraceToString()}")
            }
        }
    }

    /**
     * Runs a transaction that returns errors instead of retrying. When an operation fails:
     * registered compensations run in reverse order (best-effort), and the failure is returned
     * with rollback status.
     */
    fun <A, Err> fallibleTransaction(body: (FallibleTransaction<Err>) -> Either<Err, A>): Either<TransactionFailure<Err>, A> {
        val guard = Guards.markAtomicOperation()
        val tx = FallibleTransaction<Err>()
        return try {
            when (val result = body(tx)) {
                is Either.Right -> {
                    guard.drop()
                    Either.Right(result.value)
                }
                is Either.Left -> {
                    val failure = tx.onFailure(result.value)
                    guard.drop()
                    Either.Left(failure)
                }
            }
        } catch (e: Throwable) {
            HostApi.trap("fallibleTransaction failed: ${e.stackTraceToString()}")
        }
    }

    /** Transaction context for infallible transactions: operations that fail trigger automatic rollback and retry. */
    class InfallibleTransaction internal constructor() {
        private val compensations = mutableListOf<() -> Unit>()

        /** Executes [operation] within the transaction. On success, registers the compensation
         * for potential rollback. On failure, runs all compensations and signals retry. */
        fun <In, Out, Err> execute(operation: Operation<In, Out, Err>, input: In): Out = when (val result = operation.execute(input)) {
            is Either.Right -> {
                compensations.add(0) {
                    when (val c = operation.compensate(input, result.value)) {
                        is Either.Right -> Unit
                        is Either.Left -> error("Infallible compensation failed: ${c.value}")
                    }
                }
                result.value
            }
            is Either.Left -> rollback()
        }

        private fun rollback(): Nothing {
            for (c in compensations) c()
            throw RetrySignal
        }
    }

    /** Transaction context for fallible transactions: operations that fail trigger best-effort rollback without retry. */
    class FallibleTransaction<Err> internal constructor() {
        private val compensations = mutableListOf<() -> Either<Err, Unit>>()

        /** Executes [operation] within the transaction. On success, registers the compensation
         * for potential rollback. On failure, returns the error (call [onFailure] to roll back). */
        fun <In, Out> execute(operation: Operation<In, Out, Err>, input: In): Either<Err, Out> = when (val result = operation.execute(input)) {
            is Either.Right -> {
                compensations.add(0) { operation.compensate(input, result.value) }
                result
            }
            is Either.Left -> result
        }

        /** Triggers rollback after a failure: runs all registered compensations in reverse order. */
        fun onFailure(error: Err): TransactionFailure<Err> {
            for (c in compensations) {
                when (val r = c()) {
                    is Either.Left -> return TransactionFailure.FailedAndRolledBackPartially(error, r.value)
                    is Either.Right -> {}
                }
            }
            return TransactionFailure.FailedAndRolledBackCompletely(error)
        }
    }
}
