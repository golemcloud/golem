package cloud.golem.runtime

// Native port of Scala's Checkpoint.scala. Same situation as Transactions.kt: pure application
// logic built entirely on HostApi.getOplogIndex/setOplogIndex (no new WIT surface, no new host
// imports), ported synchronously rather than Future-based -- the underlying host calls have no
// async boundary, so Scala's Future wrapping (a Scala.js environment artifact) doesn't carry
// over. Reuses Transactions.kt's Either (same package, first and so-far-only other consumer).

/**
 * Captures the current oplog index and can revert execution to that point. Create one with
 * [Checkpoint.invoke] (mirrors Scala's `Checkpoint()` factory call), or use
 * [Checkpoint.withCheckpoint] / [Checkpoint.withCheckpointTry] to run a block with automatic
 * revert on failure.
 */
class Checkpoint private constructor(private val oplogIndex: Long) {
    /** Reverts execution to the oplog index captured when this checkpoint was created. Never returns normally. */
    fun revert(): Nothing {
        HostApi.setOplogIndex(oplogIndex)
        error("Unreachable: reverted to checkpoint")
    }

    /** Returns the successful value, or reverts to the checkpoint if [result] is a [Either.Left]. */
    fun <T> unwrapOrRevert(result: Either<*, T>): T = when (result) {
        is Either.Right -> result.value
        is Either.Left -> revert()
    }

    /** Runs [fn], reverting to the checkpoint if it returns a [Either.Left]. */
    fun <T> runOrRevert(fn: () -> Either<*, T>): T = unwrapOrRevert(fn())

    /** Runs [fn], reverting to the checkpoint if it throws. */
    fun <T> tryOrRevert(fn: () -> T): T = try {
        fn()
    } catch (e: Throwable) {
        revert()
    }

    /** Reverts to the checkpoint if [condition] is false. */
    fun assertOrRevert(condition: Boolean) {
        if (!condition) revert()
    }

    companion object {
        /** Creates a new checkpoint at the current oplog index. */
        operator fun invoke(): Checkpoint = Checkpoint(HostApi.getOplogIndex())

        /** Creates a checkpoint and runs [fn]; reverts if it returns a [Either.Left]. */
        fun <T> withCheckpoint(fn: (Checkpoint) -> Either<*, T>): T {
            val cp = Checkpoint()
            return cp.unwrapOrRevert(fn(cp))
        }

        /** Creates a checkpoint and runs [fn]; reverts if it throws. */
        fun <T> withCheckpointTry(fn: (Checkpoint) -> T): T {
            val cp = Checkpoint()
            return try {
                fn(cp)
            } catch (e: Throwable) {
                cp.revert()
            }
        }
    }
}
