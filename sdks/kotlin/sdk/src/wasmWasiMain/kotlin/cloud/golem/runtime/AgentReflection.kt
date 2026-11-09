@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.runtime

import cloud.golem.wasm.liftParamRecord
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt

// Agent reflection: read the registered agent types (their constructors, methods and parameter / result types) and call any agent dynamically,
// without a generated client. The `agent-type` record is lifted from the host registry; every parameter and result type is reconstructed as the SDK's
// witType string (the same one the typed clients use), so values are plain [SchemaValue]s. Offsets are those of `AgentTypeModel.kt` (computed
// from the 1.6.0 WIT): agent-type 232 B (type-name@0, kind@8, description@12, source-language@20, schema@28, constructor@48, methods@92,
// mode@108); agent-constructor input-schema@32; agent-method 88 B (name@0, description@8, input-schema@36, output-schema@48, read-only@56);
// input-schema = variant { parameters(list<named-field 72 B { name@0, source@8, schema@12 }>) } (tag@0, list@4); output-schema = variant { unit,
// single(type-node-index) } (tag@0, index@4).
private const val AT_TYPE_NAME = 0
private const val AT_KIND = 8
private const val AT_DESCRIPTION = 12
private const val AT_SOURCE_LANGUAGE = 20
private const val AT_SCHEMA = 28 // schema-graph: type-nodes ptr@0 / len@4, defs ptr@8 / len@12, root@16
private const val AT_CONSTRUCTOR = 48
private const val AT_METHODS = 92
private const val AT_MODE = 108
private const val AC_INPUT_SCHEMA = 32
private const val AM_SIZE = 88
private const val AM_NAME = 0
private const val AM_DESCRIPTION = 8
private const val AM_INPUT_SCHEMA = 36
private const val AM_OUTPUT_SCHEMA = 48
private const val AM_READ_ONLY = 56
private const val NF_SIZE = 72
private const val NF_NAME = 0
private const val NF_SOURCE = 8
private const val NF_SCHEMA = 12
private const val REGISTERED_IMPLEMENTED_BY = 232

/** One declared parameter of an agent constructor or method. [autoInjected] ones (the caller's principal) are filled in by the runtime, not passed. */
data class AgentParameterInfo(val name: String, val witType: String, val autoInjected: Boolean)

/** A method of an agent type: its parameters and result type ([resultWitType] is null for no result). */
data class AgentMethodInfo(
    val name: String,
    val description: String,
    val parameters: List<AgentParameterInfo>,
    val resultWitType: String?,
    val readOnly: Boolean,
) {
    /** The parameters a caller supplies, in order. */
    val userParameters: List<AgentParameterInfo> get() = parameters.filterNot { it.autoInjected }
}

/** A registered agent type as the host reports it. */
data class AgentTypeInfo(
    val typeName: String,
    val description: String,
    /** `regular` or `http-router`. */
    val kind: String,
    /** `durable` or `ephemeral`. */
    val mode: String,
    val sourceLanguage: String,
    val constructorParameters: List<AgentParameterInfo>,
    val methods: List<AgentMethodInfo>,
    val implementedBy: ComponentId,
) {
    fun method(name: String): AgentMethodInfo? = methods.firstOrNull { it.name == name }
}

private fun liftParameters(agentType: Int, inputSchema: Int): List<AgentParameterInfo> {
    // input-schema: tag@0 (parameters), list<named-field> ptr@4 / len@8
    val nodesPtr = loadInt(agentType + AT_SCHEMA)
    val defsPtr = loadInt(agentType + AT_SCHEMA + 8)
    val ptr = loadInt(inputSchema + 4)
    return List(loadInt(inputSchema + 8)) { i ->
        val field = ptr + i * NF_SIZE
        AgentParameterInfo(
            liftString(loadInt(field + NF_NAME), loadInt(field + NF_NAME + 4)),
            schemaNodeToWitType(nodesPtr, defsPtr, loadInt(field + NF_SCHEMA)),
            (loadByte(field + NF_SOURCE).toInt() and 0xFF) != 0,
        )
    }
}

/** Lifts an `agent-type` record at [base] (with its component, `implemented-by`, at [implementedByBase]). */
internal fun liftAgentTypeInfo(base: Int, implementedByBase: Int): AgentTypeInfo {
    val nodesPtr = loadInt(base + AT_SCHEMA)
    val defsPtr = loadInt(base + AT_SCHEMA + 8)
    val methodsPtr = loadInt(base + AT_METHODS)
    val methods = List(loadInt(base + AT_METHODS + 4)) { i ->
        val m = methodsPtr + i * AM_SIZE
        val output = m + AM_OUTPUT_SCHEMA
        AgentMethodInfo(
            liftString(loadInt(m + AM_NAME), loadInt(m + AM_NAME + 4)),
            liftString(loadInt(m + AM_DESCRIPTION), loadInt(m + AM_DESCRIPTION + 4)),
            liftParameters(base, m + AM_INPUT_SCHEMA),
            if (loadByte(output).toInt() and 0xFF == 0) null else schemaNodeToWitType(nodesPtr, defsPtr, loadInt(output + 4)),
            loadByte(m + AM_READ_ONLY).toInt() and 0xFF != 0,
        )
    }
    return AgentTypeInfo(
        typeName = liftString(loadInt(base + AT_TYPE_NAME), loadInt(base + AT_TYPE_NAME + 4)),
        description = liftString(loadInt(base + AT_DESCRIPTION), loadInt(base + AT_DESCRIPTION + 4)),
        kind = if (loadByte(base + AT_KIND).toInt() and 0xFF == 0) "regular" else "http-router",
        mode = if (loadByte(base + AT_MODE).toInt() and 0xFF == 0) "durable" else "ephemeral",
        sourceLanguage = liftString(loadInt(base + AT_SOURCE_LANGUAGE), loadInt(base + AT_SOURCE_LANGUAGE + 4)),
        constructorParameters = liftParameters(base, base + AT_CONSTRUCTOR + AC_INPUT_SCHEMA),
        methods = methods,
        implementedBy = liftComponentId(implementedByBase),
    )
}

/** Reading the agent types registered with the Golem host. */
object AgentReflection {
    /** Every agent type registered in the environment. */
    fun agentTypes(): List<AgentTypeInfo> = HostApi.registeredAgentTypePointers().map { liftAgentTypeInfo(it, it + REGISTERED_IMPLEMENTED_BY) }

    /** One registered agent type by name, or null. */
    fun agentType(typeName: String): AgentTypeInfo? = HostApi.registeredAgentTypePointer(typeName)?.let { liftAgentTypeInfo(it, it + REGISTERED_IMPLEMENTED_BY) }

    /**
     * A dynamic client for the agent with the given agent-id string (for example `Counter("c1")`): looks up its type and reads its constructor
     * arguments from the id. Null if the id does not parse or its type is not registered.
     * @throws RpcException if the host refuses to create the client.
     */
    fun dynamicClient(agentId: String): DynamicAgentClient? {
        val ref = HostApi.parseAgentIdConstructorParams(agentId) ?: return null
        val type = agentType(ref.agentTypeName) ?: return null
        val declared = type.constructorParameters.filterNot { it.autoInjected }
        val values = liftParamRecord(ref.paramsValueTreePtr, declared.map { it.witType })
        return DynamicAgentClient(type, WasmRpc(type.typeName, SchemaValue.Record(values)))
    }
}

/** Thrown when a dynamic call does not match the agent type's declaration. */
class DynamicCallException(message: String) : RuntimeException(message)

/**
 * Calls an agent by method name with plain [SchemaValue] arguments, checked against the agent type's declaration. [close] when done. Obtain one
 * from [AgentReflection.dynamicClient].
 */
class DynamicAgentClient internal constructor(val type: AgentTypeInfo, private val rpc: WasmRpc) : AutoCloseable {
    /**
     * Invokes [method] with [arguments] (the user-supplied parameters, in declaration order) and waits for the result. The result value is decoded
     * with the declared result type (`null` for a method without a result).
     * @throws DynamicCallException if the method is unknown or the argument count differs.
     */
    fun invoke(method: String, arguments: List<SchemaValue> = emptyList()): RpcResult {
        val info = type.method(method) ?: throw DynamicCallException("agent type ${type.typeName} has no method '$method' (methods: ${type.methods.joinToString { it.name }})")
        val expected = info.userParameters
        if (arguments.size != expected.size) {
            throw DynamicCallException("$method takes ${expected.size} argument(s) (${expected.joinToString { "${it.name}: ${it.witType}" }}), got ${arguments.size}")
        }
        return rpc.invokeAndAwait(method, SchemaValue.Record(arguments), info.resultWitType ?: "()")
    }

    /** Like [invoke] with arguments by parameter name. */
    fun invoke(method: String, arguments: Map<String, SchemaValue>): RpcResult {
        val info = type.method(method) ?: throw DynamicCallException("agent type ${type.typeName} has no method '$method'")
        val ordered = info.userParameters.map { p ->
            arguments[p.name] ?: throw DynamicCallException("$method: missing argument '${p.name}' (${p.witType})")
        }
        val unknown = arguments.keys - info.userParameters.map { it.name }.toSet()
        if (unknown.isNotEmpty()) throw DynamicCallException("$method: unknown argument(s) ${unknown.joinToString()}")
        return invoke(method, ordered)
    }

    /** Releases the underlying RPC handle. */
    override fun close() = rpc.close()
}
