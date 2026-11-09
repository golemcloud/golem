/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.bridge.runtime

import golem.bridge.runtime.json.Json

/** A resolved agent: everything a generated remote needs to invoke its methods. The [configuration] is captured when the agent is resolved. */
data class ResolvedAgent(
    val configuration: Configuration,
    val agentTypeName: String,
    val parameters: SchemaValue,
    val phantomId: String?,
    val agentId: AgentId?,
)

/** The response of an awaited invocation. */
data class AgentInvocationResult(val agentId: AgentId, val idempotencyKey: String, val result: SchemaValue?, val componentRevision: String?)

/** The outcome of an awaited call on an ephemeral agent, which has a new identity per call. */
data class InvocationResult<out A>(val agentId: AgentId, val idempotencyKey: String, val value: A)

/** The acknowledgement of a call that was scheduled or triggered on an ephemeral agent. */
data class InvocationReceipt(val agentId: AgentId, val idempotencyKey: String)

/**
 * JSON form of the bridge REST protocol: the bodies of `create-agent` and `invoke-agent` and their responses. The shapes mirror the
 * server's OpenAPI types (camelCase members), the contract the other language bridges use.
 */
internal object BridgeProtocol {
    fun renderCreateAgentRequest(configuration: Configuration, agentTypeName: String, parameters: SchemaValue, phantomId: String?): String {
        val fields = ArrayList<Pair<String, Json>>()
        fields.add("appName" to Json.string(configuration.appName))
        fields.add("envName" to Json.string(configuration.envName))
        fields.add("agentTypeName" to Json.string(agentTypeName))
        fields.add("parameters" to SchemaValueJson.toJson(parameters))
        phantomId?.let { fields.add("phantomId" to Json.string(it)) }
        fields.add("config" to Json.arr(emptyList()))
        return Json.obj(fields).render()
    }

    fun renderInvocationRequest(resolved: ResolvedAgent, methodName: String, methodParameters: SchemaValue, mode: String, scheduleAt: String?): String {
        val fields = ArrayList<Pair<String, Json>>()
        fields.add("appName" to Json.string(resolved.configuration.appName))
        fields.add("envName" to Json.string(resolved.configuration.envName))
        fields.add("agentTypeName" to Json.string(resolved.agentTypeName))
        fields.add("parameters" to SchemaValueJson.toJson(resolved.parameters))
        fields.add("config" to Json.arr(emptyList()))
        fields.add("methodName" to Json.string(methodName))
        fields.add("methodParameters" to SchemaValueJson.toJson(methodParameters))
        fields.add("mode" to Json.string(mode))
        resolved.phantomId?.let { fields.add("phantomId" to Json.string(it)) }
        scheduleAt?.let { fields.add("scheduleAt" to Json.string(it)) }
        return Json.obj(fields).render()
    }

    fun decodeAgentId(json: Json): AgentId =
        AgentId(Json.asString(Json.requireField(json, "componentId")), Json.asString(Json.requireField(json, "agentId")))

    fun decodeCreateAgentResponse(json: Json): AgentId = decodeAgentId(Json.requireField(json, "agentId"))

    fun decodeInvocationResult(json: Json): AgentInvocationResult {
        val agentId = decodeAgentId(Json.requireField(json, "agentId"))
        val key = Json.asString(Json.requireField(json, "idempotencyKey"))
        val result = Json.field(json, "result")?.let { typed ->
            Json.asObject(typed)
            SchemaValueJson.fromJson(Json.requireField(typed, "value"))
        }
        return AgentInvocationResult(agentId, key, result, Json.field(json, "componentRevision")?.let(Json::asNumberLiteral))
    }
}
