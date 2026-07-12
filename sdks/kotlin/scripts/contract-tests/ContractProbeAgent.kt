package contractprobe

import cloud.golem.BaseAgent
import cloud.golem.Datetime
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint
import cloud.golem.runtime.Checkpoint
import cloud.golem.runtime.Either
import cloud.golem.runtime.Guards
import cloud.golem.runtime.HostApi
import cloud.golem.runtime.Transactions
import cloud.golem.runtime.host.AttributeValue
import cloud.golem.runtime.host.ContextApi
import cloud.golem.runtime.host.DurabilityApi
import cloud.golem.runtime.host.DurableFunctionType
import cloud.golem.runtime.host.GetOplog
import cloud.golem.runtime.host.NamedPolicy
import cloud.golem.runtime.host.Policy
import cloud.golem.runtime.host.RetryApi
import cloud.golem.runtime.host.SecretApi
import cloud.golem.runtime.host.SecretRevealException
import cloud.golem.runtime.host.namedPolicy
import cloud.golem.runtime.host.setRetryPolicy
import kotlin.time.Duration.Companion.milliseconds

/**
 * Contract-test probe agent. Each @Endpoint exercises one SDK capability's host boundary and
 * returns a String verdict: "OK <detail>" on a clean round-trip, "FAIL <detail>" on a handled
 * mismatch. An unhandled host trap aborts the invoke (the driver classifies that separately).
 * Scope is contract-only: prove the call crossed the boundary and returned the expected shape,
 * NOT that the value is functionally correct.
 */
@Agent(mount = "/probe/{id}", description = "Contract-test probe agent")
class ContractProbeAgent(val id: String) : BaseAgent() {

    // --- Capability 1: Agent model / BaseAgent identity ---
    @Endpoint(get = "/agent-model")
    fun probeAgentModel(): String {
        val aid = agentId
        return if (aid.isNotEmpty()) "OK agentId=$aid type=$agentType name=$agentName"
        else "FAIL empty agentId"
    }

    // --- HTTP gateway coverage (one endpoint reachable over the HTTP API) ---
    @Endpoint(post = "/http-echo")
    fun httpEcho(): String = "OK http id=$id"

    // --- Capability 2: Type mapping (Kotlin <-> WIT / schema values) ---

    // Return-direction (lower): one value touching every mapped type family.
    @Endpoint(get = "/return-all-types")
    fun returnAllTypes(): AllTypes = AllTypes(
        i8 = -8, i16 = -16, i32 = -32, i64 = -64L,
        u8 = 8u, u16 = 16u, u32 = 32u, u64 = 64uL,
        f32 = 1.5f, f64 = 2.5, flag = true, text = "hello",
        opt = 7, nums = listOf(1, 2, 3), color = Color.GREEN,
        shape = Shape.Circle(3.0), pair = Pair(9, "nine"),
        dict = mapOf("k" to 1), res = Either.Right(42), ts = Datetime(1000L, 0),
    )

    // Lift-direction (host -> guest) for the types with unambiguous CLI literals.
    @Endpoint(post = "/echo-record")
    fun echoRecord(p: Pt): String = "OK ${p.x},${p.y}"

    @Endpoint(post = "/echo-list")
    fun echoList(xs: List<Int>): String = "OK size=${xs.size}"

    @Endpoint(post = "/echo-opt")
    fun echoOpt(o: Int?): String = "OK ${o ?: -1}"

    // --- Capability 3: Host API (read-only host imports) ---
    @Endpoint(get = "/host-api")
    fun probeHostApi(): String {
        val meta = HostApi.getSelfMetadata()
        val parsed = HostApi.parseAgentId(meta.agentId.agentId)
        val types = HostApi.getAllAgentTypes() // may be empty -> still OK
        val idem = HostApi.getIdempotenceMode()
        return "OK metaId=${meta.agentId.agentId} parsed=${parsed::class.simpleName} " +
            "types=${types.size} idem=$idem"
    }

    // --- Capability 4: Oplog (get-oplog resource + entry decode) ---
    @Endpoint(get = "/oplog")
    fun probeOplog(): String {
        val meta = HostApi.getSelfMetadata()
        val oplog = GetOplog(meta.agentId, 0L)
        var count = 0
        val kinds = mutableSetOf<String>()
        try {
            while (true) {
                val batch = oplog.getNext() ?: break
                count += batch.size
                batch.forEach { kinds.add(it::class.simpleName ?: "?") }
            }
        } finally {
            oplog.close()
        }
        // A live agent always has at least a Create entry; if decode traps this never returns.
        return if (count > 0) "OK entries=$count kinds=${kinds.size}" else "FAIL empty oplog"
    }

    // --- Capability 5: Retry + Retry DSL (policy-tree marshalling) ---
    @Endpoint(get = "/retry")
    fun probeRetry(): String {
        val policy = NamedPolicy("probe-policy", Policy.exponential(100.milliseconds, 2.0).maxRetries(5))
        RetryApi.setRetryPolicy(policy) // lower the tree to the host
        val readBack = RetryApi.namedPolicy("probe-policy") // lift it back
        return if (readBack != null && readBack.name == "probe-policy") {
            "OK policy round-tripped: ${readBack.name}"
        } else {
            "FAIL policy not found after set"
        }
    }

    // --- Capability 6: Transactions (begin/commit machinery) ---
    @Endpoint(get = "/transactions")
    fun probeTransactions(): String {
        // Minimal body that ignores the tx handle: exercises begin + commit without needing an Operation.
        val result = Transactions.infallibleTransaction { 42 }
        return if (result == 42) "OK transaction committed, result=$result" else "FAIL unexpected result=$result"
    }

    // --- Capability 7: Guards & Checkpoint (scoped host-state guards) ---
    @Endpoint(get = "/guards")
    fun probeGuards(): String {
        val a = Guards.withPersistenceLevel(HostApi.PersistenceLevel.PERSIST_NOTHING) { 1 }
        val b = Guards.withIdempotenceMode(true) { 2 }
        val c = Guards.atomically { 3 }
        val cp = Checkpoint() // captures current oplog index
        cp.assertOrRevert(true) // true -> no revert; false would trap-revert
        return "OK guards ran: $a$b$c checkpoint-ok"
    }

    // --- Capability 8: Secrets (reveal error-path; no provisioning available) ---
    @Endpoint(get = "/secrets")
    fun probeSecrets(): String = try {
        // 0 is not a live secret handle. A wired boundary either lifts a SecretError (caught here)
        // or the host rejects the handle as a trap (invoke exits non-zero -> driver classifies it).
        SecretApi.reveal(0, "string")
        "FAIL reveal(0) returned a value instead of erroring"
    } catch (e: SecretRevealException) {
        "OK SecretError lifted: ${e.error::class.simpleName}"
    }

    // --- Capability 9: Context / tracing (span + invocation-context resources) ---
    @Endpoint(get = "/context")
    fun probeContext(): String {
        val span = ContextApi.startSpan("probe-span")
        span.setAttribute("probe.key", AttributeValue.StringValue("probe.value"))
        val ctx = ContextApi.currentContext()
        val trace = ctx.traceId()
        ctx.close()
        span.close()
        return "OK span+context ran, traceId=${if (trace.isNotEmpty()) "present" else "empty"}"
    }

    // --- Capability 10: Durability (durable-function marshalling; replay is out of scope) ---
    @Endpoint(get = "/durability")
    fun probeDurability(): String {
        val begin = DurabilityApi.beginDurableFunction(DurableFunctionType.ReadRemote)
        val state = DurabilityApi.currentDurableExecutionState()
        DurabilityApi.endDurableFunction(DurableFunctionType.ReadRemote, begin, false)
        return "OK durable region marshalled: begin=$begin live=${state.isLive}"
    }
}

enum class Color { RED, GREEN, BLUE }

sealed class Shape {
    data class Circle(val radius: Double) : Shape()
    data class Rect(val w: Int, val h: Int) : Shape()
    object Unknown : Shape()
}

data class Pt(val x: Int, val y: Int)

data class AllTypes(
    val i8: Byte, val i16: Short, val i32: Int, val i64: Long,
    val u8: UByte, val u16: UShort, val u32: UInt, val u64: ULong,
    val f32: Float, val f64: Double, val flag: Boolean, val text: String,
    val opt: Int?, val nums: List<Int>, val color: Color, val shape: Shape,
    val pair: Pair<Int, String>, val dict: Map<String, Int>,
    val res: Either<String, Int>, val ts: Datetime,
)
