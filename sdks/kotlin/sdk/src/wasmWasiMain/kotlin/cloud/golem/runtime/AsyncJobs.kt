package cloud.golem.runtime

// Deliberately free of host imports: referencing anything in a file that declares a @WasmImport
// links that import into whatever module reaches it, and the node test runner cannot link the
// component-model stream imports. Keeping this queue separate makes it directly testable.

/** Work that runs after an async-lifted export delivered its result with `task.return` (stream producers keep writing then). */
internal object AsyncJobs {
    private val jobs = ArrayDeque<() -> Unit>()

    /** True while producers run after the invocation's result was delivered: its memory is released, so writes may reclaim theirs. */
    var reclaimMemory = false
        private set

    fun after(job: () -> Unit) {
        jobs.addLast(job)
    }

    fun hasPending(): Boolean = jobs.isNotEmpty()

    /**
     * Runs every queued job, including jobs queued by running ones. [reclaim] says whether the linear-memory arena may be reset after each
     * element: true only after `task.return` (otherwise an in-flight call's argument and result areas would be overwritten).
     *
     * Reentrant: a running job may itself call [runAll] (e.g. a streamed RPC call made from a `task.return` producer). The previous
     * [reclaimMemory] is restored on exit rather than hard-reset to false, so a reentrant call with [reclaim] = false does not disable
     * reclaim for the outer call's remaining producer writes.
     */
    fun runAll(reclaim: Boolean) {
        val previousReclaim = reclaimMemory
        reclaimMemory = reclaim
        try {
            while (jobs.isNotEmpty()) jobs.removeFirst()()
        } finally {
            reclaimMemory = previousReclaim
        }
    }
}
