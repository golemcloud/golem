package cloud.golem.runtime

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/**
 * The host resources in [Rpc] are dropped by `close()`, and the WIT's `cancel` is an ordinary
 * method beside the drop rather than a destructor. Passing a dropped handle back to the host
 * traps the agent, so every use goes through the slot and the drop happens exactly once. The
 * classes themselves call host imports the node runner cannot link, so the rule is tested here.
 */
class ReleasableHandleTest {
    @Test
    fun a_handle_is_usable_until_it_is_released() {
        val slot = ReleasableHandle("future", 7)
        assertFalse(slot.isReleased)
        assertEquals(7, slot.use())
        assertEquals(7, slot.use())
    }

    @Test
    fun releasing_returns_the_handle_once_and_nothing_after() {
        val slot = ReleasableHandle("future", 7)
        assertEquals(7, slot.release())
        assertTrue(slot.isReleased)
        assertEquals(0, slot.release())
        assertEquals(0, slot.release())
    }

    @Test
    fun using_a_released_handle_is_an_error_rather_than_a_trap() {
        val slot = ReleasableHandle("cancellation token", 11)
        slot.release()
        val error = assertFailsWith<IllegalStateException> { slot.use() }
        assertTrue(error.message!!.contains("cancellation token"), error.message!!)
        assertTrue(error.message!!.contains("closed"), error.message!!)
    }

    @Test
    fun a_handle_that_was_never_present_is_already_released() {
        val slot = ReleasableHandle("future", 0)
        assertTrue(slot.isReleased)
        assertEquals(0, slot.release())
        assertFailsWith<IllegalStateException> { slot.use() }
    }
}
