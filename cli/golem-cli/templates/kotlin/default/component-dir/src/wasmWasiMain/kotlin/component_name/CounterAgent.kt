package component_name

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.Prompt

/**
 * A durable counter agent.
 *
 * Mounted at /counters/{name}: each unique {name} is a separate agent instance with its own
 * persistent state, managed durably by Golem. Compiled directly to Wasm (WasmGC) via
 * Kotlin/Wasm -- no JavaScript, no QuickJS. The KSP processor reads these annotations at
 * compile time and generates the actual `@WasmExport` guest functions + agent metadata.
 */
@Agent(mount = "/counters/{name}", description = "A durable counter agent")
class CounterAgent(val name: String) : BaseAgent() {

    private var count: Int = 0

    /** Increases the count by one and returns the new value. */
    @Prompt("Increase the count by one")
    @Endpoint(post = "/increment")
    fun increment(): Int {
        count++
        return count
    }

    /** Returns the current value without modifying it. */
    @Prompt("Get the current counter value")
    @Endpoint(get = "/value")
    fun value(): Int = count

    /** Adds the given amount to the counter and returns the new value. */
    @Endpoint(post = "/add")
    fun add(by: Int): Int {
        count += by
        return count
    }

    /** Resets the counter to zero. */
    @Endpoint(post = "/reset")
    fun reset() {
        count = 0
    }
}
