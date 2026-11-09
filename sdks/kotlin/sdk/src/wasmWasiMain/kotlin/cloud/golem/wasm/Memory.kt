@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.wasm

import kotlin.wasm.unsafe.Pointer
import kotlin.wasm.unsafe.withScopedMemoryAllocator

// Persistent bump allocator over a high linear-memory region that grows on demand. The stdlib's scoped allocator works in low addresses
// (resetting per scope), so a hand-managed bump pointer above `base` persists across the host's realloc -> invoke -> post-return calls.
// `base` is the headroom the stdlib's scopes get below the arena: its scoped allocator starts at address 0, so a scope that allocates
// more than `base` (printing a string longer than the headroom, say) would write into the arena. The headroom is 1 MiB unless a
// component sets another (`configureArenaHeadroom`): a smaller one makes every agent of the component cheaper by that much. A canary word in the last bytes of the headroom
// is overwritten by any such scope, and the next allocation fails with an error instead of handing out corrupted memory.
// Memory is extended only as far as an allocation reaches, so an agent's linear memory follows the largest call it has served and
// is never reserved up front. The pages stay once grown, because linear memory cannot shrink.
private const val DEFAULT_HEADROOM = 1024 * 1024

/** The largest headroom a component can ask for. */
internal const val MAX_ARENA_HEADROOM = 16 * 1024 * 1024
private const val PAGE = 64 * 1024

/** At the default headroom the first growth needs 17 pages; below this the exact request is not used, because the starting size is not known to be small enough. */
private const val FIRST_GROWTH_EXACT_FROM = 16L
private const val CANARY = 0x5A17C0DEA4E4A001L
private const val CANARY_SIZE = 8

/** The highest address the arena can reach: the end of the last whole page that fits a 32-bit address. */
private const val ARENA_LIMIT = Int.MAX_VALUE.toLong() and (PAGE - 1).toLong().inv()

private var base = DEFAULT_HEADROOM
private var bump = 0
private var capacity = 0
private var pages = 0L
private var allocatorStart = -1

private fun ensureInit() {
    if (bump == 0) {
        growTo(base.toLong() + PAGE)
        storeLong(base - CANARY_SIZE, CANARY)
        bump = base
    }
}

private fun checkCanary() {
    check(loadLong(base - CANARY_SIZE) == CANARY) {
        "alloc: a stdlib scoped allocation larger than the $base-byte headroom below the arena overwrote it; " +
            "keep strings passed to print or println well under that size"
    }
}

/**
 * Extends linear memory so that every address below [end] is backed. The stdlib's scoped allocator grows memory only when a request
 * of `n` bytes has `floor(n / PAGE) + 1` pages above the current size, and then to `floor(n / PAGE) + 2` pages (measured on Kotlin
 * 2.4.0). Asking for `needed - 1` pages therefore grows exactly when memory is short, whatever its size, and lands one page over
 * what is needed. The first growth of a module at the default headroom asks for `needed - 2` and lands exactly on `needed`: the memory
 * a module starts with is fixed, and the read below traps on the first call of every agent if it were ever not small enough. [pages]
 * is a lower bound on the memory that exists (the stdlib may have grown it for its own scopes).
 */
private fun growTo(end: Long) {
    val target = (end + PAGE - 1) / PAGE * PAGE
    val needed = target / PAGE
    if (needed > pages) {
        if (allocatorStart < 0) allocatorStart = withScopedMemoryAllocator { a -> a.allocate(1).address.toInt() }
        val short = if (pages == 0L && needed >= FIRST_GROWTH_EXACT_FROM) needed - 2 else needed - 1
        val request = (short * PAGE - allocatorStart).coerceAtLeast(0)
        withScopedMemoryAllocator { a -> a.allocate(request.toInt()) }
        pages = maxOf(pages, needed)
    }
    loadByte(target.toInt() - 1)
    capacity = target.toInt()
}

internal fun arenaBase(): Int = base

internal fun arenaCapacity(): Int = capacity

/** Forgets the arena's bookkeeping, as a freshly instantiated agent has it. Only for tests: the memory itself stays grown. */
internal fun resetArenaForTest() {
    epoch++
    bump = 0
    capacity = 0
    base = DEFAULT_HEADROOM
}

/**
 * Sets the headroom below the arena, in whole 64 KiB pages from one page to [MAX_ARENA_HEADROOM], and returns it. It has to run before
 * the arena's first allocation, which the generated entry point arranges when a component sets `arenaHeadroomKiB`; asking again for
 * the same size later is harmless and a different size fails. A stdlib scope larger than the headroom (a `println` of more text than
 * fits) corrupts the arena and fails the agent, so choose it from the longest string the agent prints.
 */
fun configureArenaHeadroom(bytes: Int): Int {
    require(bytes in PAGE..MAX_ARENA_HEADROOM && bytes % PAGE == 0) {
        "arena headroom must be a whole number of 64 KiB pages from 64 KiB to ${MAX_ARENA_HEADROOM / 1024} KiB, got $bytes bytes"
    }
    if (bump == 0) base = bytes else check(bytes == base) { "arena headroom is already $base bytes and cannot become $bytes after the arena was used" }
    return bytes
}

fun alloc(size: Int, align: Int): Int {
    require(size >= 0) { "alloc: negative size $size" }
    require(align > 0 && align and (align - 1) == 0) { "alloc: alignment $align is not a power of two" }
    ensureInit()
    checkCanary()
    val mask = align - 1
    val start = (bump + mask) and mask.inv()
    // Long arithmetic: `start + size` must not wrap.
    val end = start.toLong() + size.toLong()
    check(end <= ARENA_LIMIT) { "alloc: request of $size bytes at $start passes the end of the address space ($ARENA_LIMIT)" }
    if (end > capacity) growTo(end)
    bump = end.toInt()
    return start
}

fun resetHeap() {
    epoch++
    if (bump != 0) bump = base
}

private var epoch = 0

/** Counts the arena resets; memory handed out before a reset is not valid after it. */
internal fun arenaEpoch(): Int = epoch

/**
 * Runs [block] and then releases everything it allocated. For scratch memory in loops and per-call buffers: nothing allocated inside
 * may be referenced after [block] returns, so the result must already be lifted into Kotlin values. Not for calls that leave an
 * async host operation pending on the memory.
 */
inline fun <T> withArenaMark(block: () -> T): T {
    val mark = arenaMark()
    try {
        return block()
    } finally {
        arenaRelease(mark)
    }
}

@PublishedApi
internal fun arenaMark(): Int {
    ensureInit()
    return bump
}

@PublishedApi
internal fun arenaRelease(mark: Int) {
    check(mark >= base && mark <= maxOf(capacity, base)) { "arenaRelease: mark $mark is outside the arena" }
    bump = mark
}

// NOT @WasmExport here: a Kotlin/Wasm library dependency's @WasmExport declarations do not
// survive into a consuming executable's final linked module (verified empirically -- the
// same class of cross-module DCE limitation documented on Guest.kt). The actual
// `cabi_realloc` export is generated by KSP (NativeRegistrationEmitter) in the agent's own
// module, delegating to this function.
fun cabiRealloc(oldPtr: Int, oldSize: Int, align: Int, newSize: Int): Int {
    val fresh = alloc(newSize, if (align == 0) 1 else align)
    // The canonical ABI lets the host grow or shrink a block it already filled: the kept prefix must survive the move.
    if (oldPtr != 0) {
        check(oldSize >= 0) { "cabiRealloc: negative old size $oldSize" }
        for (i in 0 until minOf(oldSize, newSize)) storeByte(fresh + i, loadByte(oldPtr + i))
    }
    return fresh
}

fun storeByte(ptr: Int, v: Byte) = Pointer(ptr.toUInt()).storeByte(v)
fun loadByte(ptr: Int): Byte = Pointer(ptr.toUInt()).loadByte()
fun storeInt(ptr: Int, v: Int) = Pointer(ptr.toUInt()).storeInt(v)
fun loadInt(ptr: Int): Int = Pointer(ptr.toUInt()).loadInt()
fun storeLong(ptr: Int, v: Long) = Pointer(ptr.toUInt()).storeLong(v)
fun loadLong(ptr: Int): Long = Pointer(ptr.toUInt()).loadLong()
fun storeShort(ptr: Int, v: Short) = Pointer(ptr.toUInt()).storeShort(v)
fun loadShort(ptr: Int): Short = Pointer(ptr.toUInt()).loadShort()

// kotlin.wasm.unsafe.Pointer has no store/loadFloat/Double -- reinterpret bits via the
// existing store/loadInt (Float, 32 bits) and store/loadLong (Double, 64 bits).
fun storeFloat(ptr: Int, v: Float) = Pointer(ptr.toUInt()).storeInt(v.toRawBits())
fun loadFloat(ptr: Int): Float = Float.fromBits(Pointer(ptr.toUInt()).loadInt())
fun storeDouble(ptr: Int, v: Double) = Pointer(ptr.toUInt()).storeLong(v.toRawBits())
fun loadDouble(ptr: Int): Double = Double.fromBits(Pointer(ptr.toUInt()).loadLong())

// The wasmWasi target is declared as an executable (binaries.executable()); it requires a `main`.
// The real reactor entrypoint (agent registration) arrives with the native runtime.
fun main() {}
