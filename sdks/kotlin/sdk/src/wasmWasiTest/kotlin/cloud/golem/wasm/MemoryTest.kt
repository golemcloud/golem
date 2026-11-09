package cloud.golem.wasm

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

// The arena grows on demand: valid requests advance a bump pointer and extend the linear memory only as far as the request needs,
// impossible ones fail loudly, and scoped scratch memory is released so a loop can allocate far more in total than is ever resident.
class MemoryTest {
    @Test
    fun alloc_rejects_negative_sizes_and_bad_alignment() {
        resetHeap()
        assertFailsWith<IllegalArgumentException> { alloc(-1, 4) }
        assertFailsWith<IllegalArgumentException> { alloc(8, 0) }
        assertFailsWith<IllegalArgumentException> { alloc(8, 3) }
    }

    @Test
    fun alloc_fails_instead_of_crossing_the_address_space() {
        resetHeap()
        assertFailsWith<IllegalStateException> { alloc(Int.MAX_VALUE, 1) }
        resetHeap()
    }

    @Test
    fun alloc_grows_past_the_old_fixed_window_and_the_whole_block_is_usable() {
        resetHeap()
        val size = 40 * 1024 * 1024
        val block = alloc(size, 8)
        storeByte(block, 1)
        storeByte(block + size - 1, 2)
        assertEquals(1.toByte(), loadByte(block))
        assertEquals(2.toByte(), loadByte(block + size - 1))
        resetHeap()
    }

    @Test
    fun a_small_agent_only_extends_memory_by_what_it_allocates() {
        resetArenaForTest()
        alloc(100, 1)
        assertTrue(arenaCapacity() <= arenaBase() + 64 * 1024, "first small allocation grew the arena to ${arenaCapacity()}")
        resetHeap()
    }

    @Test
    fun the_arena_keeps_its_pages_after_a_reset_so_a_later_call_does_not_regrow() {
        resetArenaForTest()
        alloc(3 * 1024 * 1024, 1)
        val grown = arenaCapacity()
        resetHeap()
        alloc(1024, 1)
        assertEquals(grown, arenaCapacity())
        resetHeap()
    }

    @Test
    fun an_overrun_of_the_default_headroom_fails_the_next_allocation_loudly() {
        resetArenaForTest()
        alloc(64, 8)
        storeLong(1024 * 1024 - 8, 0L)
        val failure = assertFailsWith<IllegalStateException> { alloc(8, 8) }
        assertTrue("headroom" in (failure.message ?: ""), "message was: ${failure.message}")
        resetArenaForTest()
    }

    @Test
    fun an_ordinary_stdlib_print_does_not_trip_the_canary() {
        resetArenaForTest()
        val block = alloc(8, 8)
        storeLong(block, 42L)
        println("a short line")
        assertEquals(42L, loadLong(block))
        alloc(8, 8)
        resetArenaForTest()
    }

    @Test
    fun the_first_allocation_starts_exactly_at_the_base() {
        resetArenaForTest()
        assertEquals(arenaBase(), alloc(8, 8))
        resetArenaForTest()
    }

    @Test
    fun the_headroom_can_be_set_before_the_first_allocation_and_the_arena_starts_there() {
        resetArenaForTest()
        assertEquals(256 * 1024, configureArenaHeadroom(256 * 1024))
        assertEquals(256 * 1024, arenaBase())
        assertEquals(256 * 1024, alloc(8, 8))
        resetArenaForTest()
        assertEquals(1024 * 1024, arenaBase())
    }

    @Test
    fun setting_the_same_headroom_again_is_harmless_but_a_different_one_after_the_arena_exists_fails() {
        resetArenaForTest()
        configureArenaHeadroom(128 * 1024)
        alloc(8, 8)
        assertEquals(128 * 1024, configureArenaHeadroom(128 * 1024))
        assertFailsWith<IllegalStateException> { configureArenaHeadroom(256 * 1024) }
        resetArenaForTest()
    }

    @Test
    fun a_headroom_that_is_not_whole_pages_or_is_out_of_range_is_refused() {
        resetArenaForTest()
        assertFailsWith<IllegalArgumentException> { configureArenaHeadroom(0) }
        assertFailsWith<IllegalArgumentException> { configureArenaHeadroom(32 * 1024) }
        assertFailsWith<IllegalArgumentException> { configureArenaHeadroom(100_000) }
        assertFailsWith<IllegalArgumentException> { configureArenaHeadroom(MAX_ARENA_HEADROOM + 64 * 1024) }
        assertEquals(64 * 1024, configureArenaHeadroom(64 * 1024))
        assertEquals(MAX_ARENA_HEADROOM, configureArenaHeadroom(MAX_ARENA_HEADROOM))
        resetArenaForTest()
    }

    @Test
    fun the_canary_sits_at_the_end_of_a_smaller_headroom_and_an_overrun_names_that_headroom() {
        resetArenaForTest()
        configureArenaHeadroom(128 * 1024)
        alloc(64, 8)
        storeLong(128 * 1024 - 16, 0L)
        alloc(8, 8)
        storeLong(128 * 1024 - 8, 0L)
        val failure = assertFailsWith<IllegalStateException> { alloc(8, 8) }
        assertTrue("131072-byte headroom" in (failure.message ?: ""), "message was: ${failure.message}")
        resetArenaForTest()
    }

    @Test
    fun alloc_is_aligned_and_monotonic() {
        resetHeap()
        val a = alloc(3, 1)
        val b = alloc(8, 8)
        assertEquals(0, b % 8)
        assertTrue(b >= a + 3)
        resetHeap()
    }

    @Test
    fun withArenaMark_releases_scratch_so_a_long_loop_fits() {
        resetHeap()
        val first = alloc(1, 1)
        resetHeap()
        repeat(1_000) { withArenaMark { alloc(64 * 1024, 1) } }
        assertEquals(first, alloc(1, 1))
        resetHeap()
    }

    @Test
    fun withArenaMark_releases_on_failure() {
        resetHeap()
        val mark = alloc(1, 1)
        assertFailsWith<IllegalStateException> {
            withArenaMark {
                alloc(16, 1)
                error("boom")
            }
        }
        assertEquals(mark + 1, alloc(1, 1))
        resetHeap()
    }

    @Test
    fun cabiRealloc_keeps_the_prefix_when_it_moves_a_block() {
        resetHeap()
        val old = cabiRealloc(0, 0, 1, 8)
        for (i in 0 until 8) storeByte(old + i, (i + 1).toByte())
        val grown = cabiRealloc(old, 8, 1, 16)
        for (i in 0 until 8) assertEquals((i + 1).toByte(), loadByte(grown + i))
        val shrunk = cabiRealloc(grown, 16, 1, 4)
        for (i in 0 until 4) assertEquals((i + 1).toByte(), loadByte(shrunk + i))
        resetHeap()
    }

    @Test
    fun stall_guard_tolerates_progress_and_fails_a_long_run_without_it() {
        val guard = StallGuard("test")
        repeat(STALLED_OPERATIONS_LIMIT) { guard.record(false) }
        guard.record(true)
        repeat(STALLED_OPERATIONS_LIMIT) { guard.record(false) }
        assertFailsWith<IllegalStateException> { guard.record(false) }
    }
}
