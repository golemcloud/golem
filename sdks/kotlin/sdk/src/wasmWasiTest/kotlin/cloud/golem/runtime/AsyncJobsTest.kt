package cloud.golem.runtime

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * The post-return job queue that drives stream producers after `task.return`. Pure logic, and
 * testable only because it lives in a file free of host imports: the node runner cannot link the
 * component-model stream imports, so anything reachable from a file that declares one is untestable
 * here. That is why this queue was moved out of `SchemaStreams.kt`.
 */
class AsyncJobsTest {
    @Test
    fun jobs_run_in_order_including_jobs_they_queue() {
        val order = ArrayList<String>()
        AsyncJobs.after {
            order.add("first")
            AsyncJobs.after { order.add("queued by first") }
        }
        AsyncJobs.after { order.add("second") }
        assertTrue(AsyncJobs.hasPending())
        AsyncJobs.runAll(reclaim = false)
        assertEquals(listOf("first", "second", "queued by first"), order)
        assertTrue(!AsyncJobs.hasPending(), "the queue drains completely")
    }

    @Test
    fun the_reclaim_flag_is_set_only_while_jobs_run() {
        var seen: Boolean? = null
        AsyncJobs.after { seen = AsyncJobs.reclaimMemory }
        AsyncJobs.runAll(reclaim = true)
        assertEquals(true, seen)
        assertTrue(!AsyncJobs.reclaimMemory, "the flag is cleared once the queue drains")
    }

    @Test
    fun the_reclaim_flag_is_cleared_even_when_a_job_throws() {
        AsyncJobs.after { error("the producer failed") }
        kotlin.runCatching { AsyncJobs.runAll(reclaim = true) }
        assertTrue(!AsyncJobs.reclaimMemory, "a failing job must not leave reclaim armed")
    }

    @Test
    fun an_empty_queue_runs_cleanly() {
        assertTrue(!AsyncJobs.hasPending())
        AsyncJobs.runAll(reclaim = false)
    }

    @Test
    fun a_reentrant_call_restores_the_outer_reclaim_flag_instead_of_clearing_it() {
        var seenDuringReentrantCall: Boolean? = null
        var seenAfterReentrantCall: Boolean? = null
        AsyncJobs.after {
            // Simulates a producer job that itself triggers another async call whose own
            // post-return jobs must run without reclaim, e.g. a streamed RPC made while draining
            // this outer, already-returned invocation's own producers.
            AsyncJobs.after { seenDuringReentrantCall = AsyncJobs.reclaimMemory }
            AsyncJobs.runAll(reclaim = false)
            seenAfterReentrantCall = AsyncJobs.reclaimMemory
        }
        AsyncJobs.runAll(reclaim = true)
        assertEquals(false, seenDuringReentrantCall)
        assertEquals(true, seenAfterReentrantCall, "the outer call's reclaim flag must be restored, not cleared")
        assertTrue(!AsyncJobs.reclaimMemory, "the flag is cleared once the outermost call drains")
    }
}
