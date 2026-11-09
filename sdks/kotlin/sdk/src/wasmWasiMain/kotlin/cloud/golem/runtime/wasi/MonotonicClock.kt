@file:OptIn(kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime.wasi

// Raw canonical-ABI import bindings to wasi:clocks/monotonic-clock@0.3.0 (WASI P3). `wait-for` is
// declared `async func` in the WIT; this SDK lowers it *synchronously* (the call blocks the guest
// until the host completes it), which is exactly what a durable sleep needs -- the host records the
// wait, so a replayed or resumed agent does not wait twice.
@kotlin.wasm.WasmImport("wasi:clocks/monotonic-clock@0.3.0", "now")
private external fun hostNow(): Long

@kotlin.wasm.WasmImport("wasi:clocks/monotonic-clock@0.3.0", "wait-for")
private external fun hostWaitFor(howLongNanos: Long)

/** Native SDK access to `wasi:clocks/monotonic-clock@0.3.0`. Times are in nanoseconds. */
object MonotonicClock {
    /** The current value of the monotonic clock, in nanoseconds. */
    fun now(): Long = hostNow()

    /** Blocks the agent for [nanos] nanoseconds of (durable) monotonic time. */
    fun waitFor(nanos: Long) = hostWaitFor(nanos)
}
