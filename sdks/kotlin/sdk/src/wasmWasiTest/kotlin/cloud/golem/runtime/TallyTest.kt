package cloud.golem.runtime

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class TallyTest {
    @Test
    fun aTotalEqualToTheLimitIsWithinIt() {
        val tally = Tally(2)
        assertTrue(tally.add(1), "one of two is within the limit")
        assertTrue(tally.add(1), "two of two is within the limit")
        assertFalse(tally.exceeded)
    }

    @Test
    fun onlyAGreaterTotalExceedsTheLimit() {
        val tally = Tally(2)
        tally.add(2)
        assertFalse(tally.add(1), "the third element passes a limit of two")
        assertTrue(tally.exceeded)
        assertEquals(3L, tally.count, "a refused increment is still counted, so a late reporter can see it")
    }

    @Test
    fun aZeroLimitAdmitsNothingButAnEmptySource() {
        val tally = Tally(0)
        assertTrue(tally.add(0), "an empty source is within a zero limit")
        assertFalse(tally.add(1))
    }

    @Test
    fun byteCountsAccumulateBeyondIntRange() {
        // ToolStdin.readAll and captured output count bytes, and their ceiling is 16 MiB. The
        // running total is a Long so that a source far past the cap cannot wrap into a value that
        // looks admissible.
        val half = Int.MAX_VALUE / 2
        val tally = Tally(Int.MAX_VALUE)
        assertTrue(tally.add(half))
        assertTrue(tally.add(half), "two halves are still within Int.MAX_VALUE")
        assertFalse(tally.add(half), "a third half passes the limit")
        // Had the total been an Int it would have wrapped negative here and read as admissible.
        assertTrue(tally.count > Int.MAX_VALUE.toLong(), "the total is ${tally.count}")
        assertTrue(tally.exceeded)
    }

    @Test
    fun aNegativeLimitOrIncrementIsRejected() {
        assertFailsWith<IllegalArgumentException> { Tally(-1) }
        assertFailsWith<IllegalArgumentException> { Tally(1).add(-1) }
    }

    @Test
    fun aLimitIsCheckedAgainstItsCeiling() {
        Cap.requireLimit(0, 10, "maxItems")
        Cap.requireLimit(10, 10, "maxItems")
        val tooBig = assertFailsWith<IllegalArgumentException> { Cap.requireLimit(11, 10, "maxItems") }
        assertTrue(tooBig.message!!.contains("maxItems must be in 0..10"))
        assertFailsWith<IllegalArgumentException> { Cap.requireLimit(-1, 10, "maxItems") }
    }
}
