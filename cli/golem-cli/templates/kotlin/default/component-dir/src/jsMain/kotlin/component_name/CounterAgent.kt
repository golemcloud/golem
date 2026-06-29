package component_name

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Description
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.Prompt

/**
 * A durable counter agent.
 *
 * Mounted at /counters/{name}: each unique {name} is a separate agent instance with its own
 * persistent state, managed durably by Golem. The KSP processor reads these annotations at
 * compile time and generates the agent registration + metadata automatically.
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
