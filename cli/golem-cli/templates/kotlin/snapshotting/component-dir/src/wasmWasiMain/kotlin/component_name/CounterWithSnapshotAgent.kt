package component_name

import cloud.golem.BaseAgent
import cloud.golem.Snapshotted
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Description
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.Prompt

/** The agent's durable state. `S` in `Snapshotted<S>` must be a WIT-mappable type (here a data class). */
data class CounterState(val value: Int)

/**
 * A durable counter agent with typed state snapshotting.
 *
 * Mixing in `Snapshotted<S>` opts the agent into snapshot-based (manual) updates. KSP derives a byte
 * codec from `S` at compile time, so a manual update calls the generated `save-snapshot` on the old
 * component revision and `load-snapshot` on the new one -- carrying typed `state` across a revision
 * bump and a restart, with the caller identity preserved. `@Agent(snapshotting = ...)` advertises the
 * cadence to the host; the mixin is what provides the state that is saved and restored. A non-mappable
 * `S` is a compile-time error, never a silent empty snapshot.
 *
 * Mounted at /snapshot-counters/{name}: each unique {name} is a separate agent instance with its own
 * persistent state. Compiled directly to Wasm (WasmGC) via Kotlin/Wasm -- no JavaScript, no QuickJS.
 */
@Agent(
    mount = "/snapshot-counters/{name}",
    description = "A durable counter agent with typed state snapshotting",
    snapshotting = "every(1)",
)
class CounterWithSnapshotAgent(val name: String) : BaseAgent(), Snapshotted<CounterState> {
    override var state = CounterState(0)

    @Prompt("Increase the count by one")
    @Description("Increments the counter and returns the new value")
    @Endpoint(post = "/increment")
    fun increment(): Int {
        state = CounterState(state.value + 1)
        return state.value
    }

    @Prompt("Get the current counter value")
    @Description("Returns the current value without modifying it")
    @Endpoint(get = "/value")
    fun getValue(): Int = state.value
}
