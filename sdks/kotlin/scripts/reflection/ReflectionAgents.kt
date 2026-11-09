package p26

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Description
import cloud.golem.annotations.Endpoint
import cloud.golem.runtime.AgentReflection
import cloud.golem.runtime.DynamicCallException
import cloud.golem.runtime.HostApi
import cloud.golem.runtime.MakeAgentIdResult
import cloud.golem.runtime.RpcResult
import cloud.golem.runtime.SchemaValue

data class Point(val x: Int, val y: Int)

@Agent(mount = "/counter/{id}", description = "A counter")
class Counter(val id: String) : BaseAgent() {
    private var total = 0

    @Endpoint(post = "/inc")
    @Description("Adds to the counter")
    fun inc(by: Int): Int {
        total += by
        return total
    }

    @Endpoint(post = "/move")
    fun move(from: Point, steps: List<Int>): Point = Point(from.x + steps.sum(), from.y)

    @Endpoint(post = "/label")
    fun label(): String = "counter:$id"

    @Endpoint(post = "/reset")
    fun reset() {
        total = 0
    }
}

@Agent(mount = "/reflector/{id}", description = "Reflects over other agents")
class Reflector(val id: String) : BaseAgent() {
    @Endpoint(post = "/describe")
    fun describe(): String {
        val type = AgentReflection.agentType("Counter") ?: return "no Counter"
        val methods = type.methods.joinToString("; ") { m ->
            "${m.name}(${m.parameters.joinToString { "${it.name}: ${it.witType}" }}) -> ${m.resultWitType ?: "()"}"
        }
        val all = AgentReflection.agentTypes().map { it.typeName }.sorted()
        return "type=${type.typeName} kind=${type.kind} mode=${type.mode} desc='${type.description}' ctor=${type.constructorParameters.map { it.name to it.witType }} methods=[$methods] all=$all"
    }

    @Endpoint(post = "/call")
    fun call(): String {
        val agentId = (HostApi.makeAgentId("Counter", SchemaValue.Record(listOf(SchemaValue.Str("c1")))) as MakeAgentIdResult.Ok).agentId
        val client = AgentReflection.dynamicClient(agentId) ?: return "no client for $agentId"
        try {
            val out = mutableListOf<String>("id=$agentId")
            out += "inc=" + (client.invoke("inc", listOf(SchemaValue.S32(5))) as RpcResult.Ok).value
            out += "inc-by-name=" + (client.invoke("inc", mapOf("by" to SchemaValue.S32(7))) as RpcResult.Ok).value
            out += "move=" + (
                client.invoke(
                    "move",
                    listOf(SchemaValue.Record(listOf(SchemaValue.S32(1), SchemaValue.S32(2))), SchemaValue.ListVal(listOf(SchemaValue.S32(3), SchemaValue.S32(4)))),
                ) as RpcResult.Ok
                ).value
            out += "label=" + (client.invoke("label") as RpcResult.Ok).value
            out += "reset=" + (client.invoke("reset") as RpcResult.Ok).value
            for ((what, f) in listOf<Pair<String, () -> Any>>(
                "unknown" to { client.invoke("nope") },
                "arity" to { client.invoke("inc", emptyList()) },
                "missing" to { client.invoke("inc", mapOf<String, SchemaValue>()) },
            )) {
                out += try {
                    "$what=" + f()
                } catch (e: DynamicCallException) {
                    "$what=${e.message}"
                }
            }
            return out.joinToString(" | ")
        } finally {
            client.close()
        }
    }
}
