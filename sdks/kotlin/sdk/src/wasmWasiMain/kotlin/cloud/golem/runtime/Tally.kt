package cloud.golem.runtime

/**
 * The bound arithmetic behind the SDK's collection caps: `AgentStream.toList`,
 * `DurableStreamIterator.toList`, `ToolStdin.readAll`, captured tool output and
 * `Blobstore.listObjects`.
 *
 * Kept free of host imports on purpose. Every capped reader lives in a file that declares
 * `@WasmImport`s, and referencing one of those links its imports into whatever module reaches it,
 * which the node test runner cannot do. Holding the arithmetic here is what makes the boundary
 * directly testable.
 *
 * Each caller keeps its own failure rather than sharing one, because they are deliberately
 * different: a typed WIT case where one exists, an `Either` for the blobstore, and a recorded flag
 * for a callback that cannot throw.
 */
internal object Cap {
    /** Validates a caller-supplied limit against the SDK's own ceiling for that reader. */
    fun requireLimit(limit: Int, ceiling: Int, name: String) {
        require(limit in 0..ceiling) { "$name must be in 0..$ceiling" }
    }
}

/**
 * A running total measured against [limit], counting either elements or bytes. A total equal to
 * the limit is within it; only a greater total exceeds it.
 */
internal class Tally(private val limit: Int) {
    init {
        require(limit >= 0) { "a limit is not negative" }
    }

    private var total = 0L

    /** What has been added so far. */
    val count: Long get() = total

    /** True once [count] has passed [limit]. */
    val exceeded: Boolean get() = total > limit

    /**
     * Adds [increment] and returns whether the total is **still within** [limit]. A false return
     * leaves the increment counted, so [exceeded] stays true for a caller that reports later
     * rather than throwing at once.
     */
    fun add(increment: Int): Boolean {
        require(increment >= 0) { "an increment is not negative" }
        total += increment
        return total <= limit
    }
}
