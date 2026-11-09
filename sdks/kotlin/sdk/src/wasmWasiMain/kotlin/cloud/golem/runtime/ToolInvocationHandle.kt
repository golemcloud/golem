package cloud.golem.runtime

/** Where forwarded output goes: the caller's stdout or stderr writer. [write] returns `false` once the caller has stopped reading. */
interface ByteSink {
    fun write(bytes: ByteArray): Boolean
}

/**
 * A wrapped call that has been started and not yet awaited. Several handles may be outstanding at once. A handle is valid only during the
 * middleware invocation that started it; the runtime releases it when the handler returns.
 */
interface ToolInvocationHandle {
    /** Waits for the call and returns its outcome. A second call returns the same outcome. */
    fun await(): UnderlyingOutcome

    /** Asks the wrapped tool to stop; [await] then yields [UnderlyingError.Cancelled] unless the call had already finished. */
    fun cancel()

    /**
     * Writes the wrapped tool's stdout and stderr to [stdout] and [stderr] as they arrive instead of collecting them (the outcome then carries
     * `null` for a forwarded stream). Must be called before [await].
     */
    fun forwardOutputs(stdout: ByteSink?, stderr: ByteSink?)
}

/** What a [StartedInvocation] needs from the host. */
internal interface CallPort {
    fun collect(stdout: ByteSink?, stderr: ByteSink?): UnderlyingOutcome

    fun cancel()

    /** Drops the host's handles for the call. */
    fun release()
}

internal class StartedInvocation(private val port: CallPort) : ToolInvocationHandle {
    private var outcome: UnderlyingOutcome? = null
    private var stdout: ByteSink? = null
    private var stderr: ByteSink? = null
    private var cancelled = false
    private var released = false
    private var failure: Throwable? = null

    override fun await(): UnderlyingOutcome {
        outcome?.let { return it }
        failure?.let { throw it }
        check(!released) { "the underlying tool was revoked: a started call is only valid during the middleware invocation" }
        val result = try {
            port.collect(stdout, stderr)
        } catch (e: Throwable) {
            failure = e
            release()
            throw e
        }
        outcome = result
        release()
        return result
    }

    override fun cancel() {
        if (outcome != null || released || cancelled) return
        cancelled = true
        port.cancel()
    }

    override fun forwardOutputs(stdout: ByteSink?, stderr: ByteSink?) {
        check(outcome == null && !released) { "outputs can only be forwarded before the call is awaited" }
        this.stdout = stdout
        this.stderr = stderr
    }

    internal fun release() {
        if (released) return
        released = true
        port.release()
    }
}

/** A started call of a typed command, decoding its outcome. */
class TypedInvocation<E, T>(
    private val handle: ToolInvocationHandle,
    private val decodeError: (name: String, payload: TypedSchemaValue) -> E?,
    private val decodeResult: (SchemaValue?) -> T,
) {
    fun await(): Either<ToolInvokeError<E>, T> = handle.await().typed(decodeError, decodeResult)

    fun cancel() = handle.cancel()
}

/** A started call of a typed command that declares stdout or stderr. */
class TypedInvocationWithOutputs<E, T>(
    private val handle: ToolInvocationHandle,
    private val decodeError: (name: String, payload: TypedSchemaValue) -> E?,
    private val decodeResult: (SchemaValue?) -> T,
) {
    fun await(): Either<ToolInvokeError<E>, WithOutputs<T>> = handle.await().typedWithOutputs(decodeError, decodeResult)

    fun cancel() = handle.cancel()

    /** Forwards the wrapped tool's outputs to the caller instead of collecting them; call before [await]. */
    fun forwardOutputs(stdout: ByteSink?, stderr: ByteSink?) = handle.forwardOutputs(stdout, stderr)
}
