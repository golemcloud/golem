package counter

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Description
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.Prompt

/**
 * CounterAgent: a durable counter scoped to an agent instance.
 *
 * Mounted at /counters/{name}. Each unique {name} gets its own counter with independent
 * persistent state managed by Golem. Compiled natively to Wasm (WasmGC) -- no JS, no QuickJS.
 * The KSP processor reads these annotations at compile time and generates the registration +
 * the real `@WasmExport("golem:agent/guest@2.0.0#...")` functions.
 */
@Agent(mount = "/counters/{name}", description = "A durable counter agent")
class CounterAgent(val name: String) : BaseAgent() {

    private var value: Int = 0

    @Prompt("Increase the count by one")
    @Description("Increments the counter and returns the new value")
    @Endpoint(post = "/increment")
    fun increment(): Int {
        value++
        return value
    }

    @Prompt("Get the current counter value")
    @Description("Returns the current value without modifying it")
    @Endpoint(get = "/value")
    fun getValue(): Int = value
}
