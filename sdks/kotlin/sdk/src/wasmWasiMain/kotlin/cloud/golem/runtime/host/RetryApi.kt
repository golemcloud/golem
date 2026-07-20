@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime.host

import cloud.golem.runtime.lowerStringToPtrLen
import cloud.golem.wasm.alloc
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadDouble
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.loadLong
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeDouble
import cloud.golem.wasm.storeInt
import cloud.golem.wasm.storeLong
import cloud.golem.wasm.writeStringField

// Native SDK access to golem:api/retry@1.5.0 -- the semantic retry-policy API. Unlike the Scala
// SDK (which keeps the policy/predicate trees OPAQUE, passing JS objects straight through), the
// native path has no opaque-JS escape hatch: the flattened `retry-policy`/`retry-predicate`
// node-list trees (structurally like schema-value-tree: a list of nodes with s32 index
// cross-references, root = nodes[0]) are marshalled field-for-field here.
//
// Full surface: DECODE (get-retry-policies / get-retry-policy-by-name / remove) and ENCODE
// (set-retry-policy / resolve-retry-policy). Every layout below (variant tags/payload offsets,
// config record field offsets, named-retry-policy field offsets, the resolve properties tuple)
// was verified via abi-dump against wit-native/deps/golem-1.x/golem-retry.wit, not hand-derived.

@kotlin.wasm.WasmImport("golem:api/retry@1.5.0", "get-retry-policies")
private external fun hostGetRetryPolicies(retPtr: Int)

@kotlin.wasm.WasmImport("golem:api/retry@1.5.0", "get-retry-policy-by-name")
private external fun hostGetRetryPolicyByName(namePtr: Int, nameLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:api/retry@1.5.0", "remove-retry-policy")
private external fun hostRemoveRetryPolicy(namePtr: Int, nameLen: Int)

// set-retry-policy(policy: named-retry-policy) is FULLY FLATTENED (indirect_params=false): the
// record's fields become args -- name(ptr,len), priority(i32), predicate.nodes(ptr,len),
// policy.nodes(ptr,len). Verified via abi-dump `sig`.
@kotlin.wasm.WasmImport("golem:api/retry@1.5.0", "set-retry-policy")
private external fun hostSetRetryPolicy(
    namePtr: Int,
    nameLen: Int,
    priority: Int,
    predPtr: Int,
    predLen: Int,
    polPtr: Int,
    polLen: Int,
)

// resolve-retry-policy(verb, noun-uri, properties: list<tuple<string, predicate-value>>)
//   -> option<retry-policy>. retptr=true; the properties list element is a 24-byte tuple
// {string@0, predicate-value@8} (align 8).
@kotlin.wasm.WasmImport("golem:api/retry@1.5.0", "resolve-retry-policy")
private external fun hostResolveRetryPolicy(
    verbPtr: Int,
    verbLen: Int,
    nounPtr: Int,
    nounLen: Int,
    propsPtr: Int,
    propsLen: Int,
    retPtr: Int,
)

// ── Kotlin model (mirrors the golem:api/retry WIT) ───────────────────────────────────────────

/** `predicate-value` variant: a dynamic value for property comparisons. */
sealed class PredicateValue {
    data class Text(val value: String) : PredicateValue()
    data class Integer(val value: Long) : PredicateValue()
    data class Bool(val value: Boolean) : PredicateValue()
}

data class PropertyComparison(val propertyName: String, val value: PredicateValue)
data class PropertySetCheck(val propertyName: String, val values: List<PredicateValue>)
data class PropertyPattern(val propertyName: String, val pattern: String)

/** `predicate-node` variant. Tuple/index cases carry `predicate-node-index` (s32) into the node list. */
sealed class PredicateNode {
    data class PropEq(val cmp: PropertyComparison) : PredicateNode()
    data class PropNeq(val cmp: PropertyComparison) : PredicateNode()
    data class PropGt(val cmp: PropertyComparison) : PredicateNode()
    data class PropGte(val cmp: PropertyComparison) : PredicateNode()
    data class PropLt(val cmp: PropertyComparison) : PredicateNode()
    data class PropLte(val cmp: PropertyComparison) : PredicateNode()
    data class PropExists(val propertyName: String) : PredicateNode()
    data class PropIn(val check: PropertySetCheck) : PredicateNode()
    data class PropMatches(val pattern: PropertyPattern) : PredicateNode()
    data class PropStartsWith(val pattern: PropertyPattern) : PredicateNode()
    data class PropContains(val pattern: PropertyPattern) : PredicateNode()
    data class PredAnd(val left: Int, val right: Int) : PredicateNode()
    data class PredOr(val left: Int, val right: Int) : PredicateNode()
    data class PredNot(val inner: Int) : PredicateNode()
    object PredTrue : PredicateNode()
    object PredFalse : PredicateNode()
}

/** `retry-predicate`: a flattened predicate tree. Root is `nodes[0]`; children by index. */
data class RetryPredicate(val nodes: List<PredicateNode>)

data class ExponentialConfig(val baseDelayNanos: Long, val factor: Double)
data class FibonacciConfig(val firstNanos: Long, val secondNanos: Long)
data class CountBoxConfig(val maxRetries: UInt, val inner: Int)
data class TimeBoxConfig(val limitNanos: Long, val inner: Int)
data class ClampConfig(val minDelayNanos: Long, val maxDelayNanos: Long, val inner: Int)
data class AddDelayConfig(val delayNanos: Long, val inner: Int)
data class JitterConfig(val factor: Double, val inner: Int)
data class FilteredConfig(val predicate: RetryPredicate, val inner: Int)

/** `policy-node` variant. Durations are total nanoseconds (WIT `duration`). */
sealed class PolicyNode {
    data class Periodic(val nanos: Long) : PolicyNode()
    data class Exponential(val config: ExponentialConfig) : PolicyNode()
    data class Fibonacci(val config: FibonacciConfig) : PolicyNode()
    object Immediate : PolicyNode()
    object Never : PolicyNode()
    data class CountBox(val config: CountBoxConfig) : PolicyNode()
    data class TimeBox(val config: TimeBoxConfig) : PolicyNode()
    data class ClampDelay(val config: ClampConfig) : PolicyNode()
    data class AddDelay(val config: AddDelayConfig) : PolicyNode()
    data class Jitter(val config: JitterConfig) : PolicyNode()
    data class FilteredOn(val config: FilteredConfig) : PolicyNode()
    data class AndThen(val left: Int, val right: Int) : PolicyNode()
    data class PolicyUnion(val left: Int, val right: Int) : PolicyNode()
    data class PolicyIntersect(val left: Int, val right: Int) : PolicyNode()
}

/** `retry-policy`: a flattened policy tree. Root is `nodes[0]`; children by index. */
data class RetryPolicy(val nodes: List<PolicyNode>)

/** `named-retry-policy`: a named rule (predicate selects when it applies, policy is the strategy). */
data class NamedRetryPolicy(
    val name: String,
    val priority: UInt,
    val predicate: RetryPredicate,
    val policy: RetryPolicy,
)

// ── DECODE (verified layouts) ────────────────────────────────────────────────────────────────

private const val NODE_STRIDE = 32 // policy-node / predicate-node: size=32 align=8
private const val NODE_PAYLOAD = 8 // both variants: tag@0, payload_offset=8
private const val PREDICATE_VALUE_SIZE = 16 // predicate-value: size=16, payload_offset=8
private const val NAMED_POLICY_STRIDE = 28 // named-retry-policy: size=28 align=4

private fun liftStringAt(base: Int): String = liftString(loadInt(base), loadInt(base + 4))

/** predicate-value: tag@base, payload@base+8. */
private fun liftPredicateValue(base: Int): PredicateValue {
    val pay = base + 8
    return when (loadByte(base).toInt() and 0xFF) {
        0 -> PredicateValue.Text(liftStringAt(pay))
        1 -> PredicateValue.Integer(loadLong(pay))
        else -> PredicateValue.Bool(loadByte(pay).toInt() != 0)
    }
}

private fun liftPropertyComparison(base: Int): PropertyComparison = PropertyComparison(liftStringAt(base), liftPredicateValue(base + 8)) // name@0, value@8

private fun liftPropertySetCheck(base: Int): PropertySetCheck {
    val name = liftStringAt(base) // property-name@0
    val valuesPtr = loadInt(base + 8) // values list@8 (ptr@8, len@12)
    val len = loadInt(base + 12)
    val values = (0 until len).map { liftPredicateValue(valuesPtr + it * PREDICATE_VALUE_SIZE) }
    return PropertySetCheck(name, values)
}

private fun liftPropertyPattern(base: Int): PropertyPattern = PropertyPattern(liftStringAt(base), liftStringAt(base + 8)) // property-name@0, pattern@8

private fun liftPredicateNode(nodePtr: Int): PredicateNode {
    val pay = nodePtr + NODE_PAYLOAD
    return when (loadByte(nodePtr).toInt() and 0xFF) {
        0 -> PredicateNode.PropEq(liftPropertyComparison(pay))
        1 -> PredicateNode.PropNeq(liftPropertyComparison(pay))
        2 -> PredicateNode.PropGt(liftPropertyComparison(pay))
        3 -> PredicateNode.PropGte(liftPropertyComparison(pay))
        4 -> PredicateNode.PropLt(liftPropertyComparison(pay))
        5 -> PredicateNode.PropLte(liftPropertyComparison(pay))
        6 -> PredicateNode.PropExists(liftStringAt(pay))
        7 -> PredicateNode.PropIn(liftPropertySetCheck(pay))
        8 -> PredicateNode.PropMatches(liftPropertyPattern(pay))
        9 -> PredicateNode.PropStartsWith(liftPropertyPattern(pay))
        10 -> PredicateNode.PropContains(liftPropertyPattern(pay))
        11 -> PredicateNode.PredAnd(loadInt(pay), loadInt(pay + 4)) // tuple<index,index>
        12 -> PredicateNode.PredOr(loadInt(pay), loadInt(pay + 4))
        13 -> PredicateNode.PredNot(loadInt(pay))
        14 -> PredicateNode.PredTrue
        else -> PredicateNode.PredFalse
    }
}

/** retry-predicate = {nodes: list}. [listBase] points at the list's (ptr,len). */
private fun liftRetryPredicate(listBase: Int): RetryPredicate {
    val nodesPtr = loadInt(listBase)
    val len = loadInt(listBase + 4)
    return RetryPredicate((0 until len).map { liftPredicateNode(nodesPtr + it * NODE_STRIDE) })
}

private fun liftPolicyNode(nodePtr: Int): PolicyNode {
    val pay = nodePtr + NODE_PAYLOAD
    return when (loadByte(nodePtr).toInt() and 0xFF) {
        0 -> PolicyNode.Periodic(loadLong(pay)) // duration
        1 -> PolicyNode.Exponential(ExponentialConfig(loadLong(pay), loadDouble(pay + 8)))
        2 -> PolicyNode.Fibonacci(FibonacciConfig(loadLong(pay), loadLong(pay + 8)))
        3 -> PolicyNode.Immediate
        4 -> PolicyNode.Never
        5 -> PolicyNode.CountBox(CountBoxConfig(loadInt(pay).toUInt(), loadInt(pay + 4)))
        6 -> PolicyNode.TimeBox(TimeBoxConfig(loadLong(pay), loadInt(pay + 8)))
        7 -> PolicyNode.ClampDelay(ClampConfig(loadLong(pay), loadLong(pay + 8), loadInt(pay + 16)))
        8 -> PolicyNode.AddDelay(AddDelayConfig(loadLong(pay), loadInt(pay + 8)))
        9 -> PolicyNode.Jitter(JitterConfig(loadDouble(pay), loadInt(pay + 8)))
        10 -> PolicyNode.FilteredOn(FilteredConfig(liftRetryPredicate(pay), loadInt(pay + 8))) // predicate list@0, inner@8
        11 -> PolicyNode.AndThen(loadInt(pay), loadInt(pay + 4))
        12 -> PolicyNode.PolicyUnion(loadInt(pay), loadInt(pay + 4))
        else -> PolicyNode.PolicyIntersect(loadInt(pay), loadInt(pay + 4))
    }
}

/** retry-policy = {nodes: list}. [listBase] points at the list's (ptr,len). */
internal fun liftRetryPolicy(listBase: Int): RetryPolicy {
    val nodesPtr = loadInt(listBase)
    val len = loadInt(listBase + 4)
    return RetryPolicy((0 until len).map { liftPolicyNode(nodesPtr + it * NODE_STRIDE) })
}

/** named-retry-policy: name@0, priority@8, predicate list@12, policy list@20. */
internal fun liftNamedRetryPolicy(base: Int): NamedRetryPolicy = NamedRetryPolicy(
    name = liftStringAt(base),
    priority = loadInt(base + 8).toUInt(),
    predicate = liftRetryPredicate(base + 12),
    policy = liftRetryPolicy(base + 20),
)

// ── ENCODE (mirror of the decode; same verified layouts) ─────────────────────────────────────
// Node arrays are alloc'd fresh; each variant writes only its tag + active-case payload -- unused
// payload/tail bytes are never read by the host (canonical ABI), so no zeroing is needed.

/** Lowers a predicate-value (16B) at [base]: tag@0, payload@8. */
private fun lowerPredicateValueInto(base: Int, v: PredicateValue) {
    when (v) {
        is PredicateValue.Text -> {
            storeByte(base, 0)
            writeStringField(base, 8, v.value)
        }
        is PredicateValue.Integer -> {
            storeByte(base, 1)
            storeLong(base + 8, v.value)
        }
        is PredicateValue.Bool -> {
            storeByte(base, 2)
            storeByte(base + 8, if (v.value) 1 else 0)
        }
    }
}

private fun lowerPropertyComparisonInto(base: Int, c: PropertyComparison) {
    writeStringField(base, 0, c.propertyName) // property-name@0
    lowerPredicateValueInto(base + 8, c.value) // value@8
}

private fun lowerPropertySetCheckInto(base: Int, c: PropertySetCheck) {
    writeStringField(base, 0, c.propertyName)
    val (ptr, len) = lowerPredicateValueList(c.values)
    storeInt(base + 8, ptr)
    storeInt(base + 12, len) // values list@8
}

private fun lowerPropertyPatternInto(base: Int, p: PropertyPattern) {
    writeStringField(base, 0, p.propertyName) // property-name@0
    writeStringField(base, 8, p.pattern) // pattern@8
}

/** Lowers a list<predicate-value> (element 16B, align 8); returns (dataPtr, len). */
private fun lowerPredicateValueList(values: List<PredicateValue>): Pair<Int, Int> {
    if (values.isEmpty()) return 0 to 0
    val arr = alloc(values.size * PREDICATE_VALUE_SIZE, 8)
    values.forEachIndexed { i, v -> lowerPredicateValueInto(arr + i * PREDICATE_VALUE_SIZE, v) }
    return arr to values.size
}

/** Lowers a predicate-node (32B) at [nodePtr]: tag@0, payload@8. */
private fun lowerPredicateNodeInto(nodePtr: Int, n: PredicateNode) {
    val pay = nodePtr + NODE_PAYLOAD
    when (n) {
        is PredicateNode.PropEq -> {
            storeByte(nodePtr, 0)
            lowerPropertyComparisonInto(pay, n.cmp)
        }
        is PredicateNode.PropNeq -> {
            storeByte(nodePtr, 1)
            lowerPropertyComparisonInto(pay, n.cmp)
        }
        is PredicateNode.PropGt -> {
            storeByte(nodePtr, 2)
            lowerPropertyComparisonInto(pay, n.cmp)
        }
        is PredicateNode.PropGte -> {
            storeByte(nodePtr, 3)
            lowerPropertyComparisonInto(pay, n.cmp)
        }
        is PredicateNode.PropLt -> {
            storeByte(nodePtr, 4)
            lowerPropertyComparisonInto(pay, n.cmp)
        }
        is PredicateNode.PropLte -> {
            storeByte(nodePtr, 5)
            lowerPropertyComparisonInto(pay, n.cmp)
        }
        is PredicateNode.PropExists -> {
            storeByte(nodePtr, 6)
            writeStringField(pay, 0, n.propertyName)
        }
        is PredicateNode.PropIn -> {
            storeByte(nodePtr, 7)
            lowerPropertySetCheckInto(pay, n.check)
        }
        is PredicateNode.PropMatches -> {
            storeByte(nodePtr, 8)
            lowerPropertyPatternInto(pay, n.pattern)
        }
        is PredicateNode.PropStartsWith -> {
            storeByte(nodePtr, 9)
            lowerPropertyPatternInto(pay, n.pattern)
        }
        is PredicateNode.PropContains -> {
            storeByte(nodePtr, 10)
            lowerPropertyPatternInto(pay, n.pattern)
        }
        is PredicateNode.PredAnd -> {
            storeByte(nodePtr, 11)
            storeInt(pay, n.left)
            storeInt(pay + 4, n.right)
        }
        is PredicateNode.PredOr -> {
            storeByte(nodePtr, 12)
            storeInt(pay, n.left)
            storeInt(pay + 4, n.right)
        }
        is PredicateNode.PredNot -> {
            storeByte(nodePtr, 13)
            storeInt(pay, n.inner)
        }
        PredicateNode.PredTrue -> storeByte(nodePtr, 14)
        PredicateNode.PredFalse -> storeByte(nodePtr, 15)
    }
}

/** Lowers a retry-predicate's node list (element 32B, align 8); returns (dataPtr, len). */
private fun lowerRetryPredicateNodes(pred: RetryPredicate): Pair<Int, Int> {
    if (pred.nodes.isEmpty()) return 0 to 0
    val arr = alloc(pred.nodes.size * NODE_STRIDE, 8)
    pred.nodes.forEachIndexed { i, n -> lowerPredicateNodeInto(arr + i * NODE_STRIDE, n) }
    return arr to pred.nodes.size
}

/** Lowers a policy-node (32B) at [nodePtr]: tag@0, payload@8. */
private fun lowerPolicyNodeInto(nodePtr: Int, n: PolicyNode) {
    val pay = nodePtr + NODE_PAYLOAD
    when (n) {
        is PolicyNode.Periodic -> {
            storeByte(nodePtr, 0)
            storeLong(pay, n.nanos)
        }
        is PolicyNode.Exponential -> {
            storeByte(nodePtr, 1)
            storeLong(pay, n.config.baseDelayNanos)
            storeDouble(pay + 8, n.config.factor)
        }
        is PolicyNode.Fibonacci -> {
            storeByte(nodePtr, 2)
            storeLong(pay, n.config.firstNanos)
            storeLong(pay + 8, n.config.secondNanos)
        }
        PolicyNode.Immediate -> storeByte(nodePtr, 3)
        PolicyNode.Never -> storeByte(nodePtr, 4)
        is PolicyNode.CountBox -> {
            storeByte(nodePtr, 5)
            storeInt(pay, n.config.maxRetries.toInt())
            storeInt(pay + 4, n.config.inner)
        }
        is PolicyNode.TimeBox -> {
            storeByte(nodePtr, 6)
            storeLong(pay, n.config.limitNanos)
            storeInt(pay + 8, n.config.inner)
        }
        is PolicyNode.ClampDelay -> {
            storeByte(nodePtr, 7)
            storeLong(pay, n.config.minDelayNanos)
            storeLong(pay + 8, n.config.maxDelayNanos)
            storeInt(pay + 16, n.config.inner)
        }
        is PolicyNode.AddDelay -> {
            storeByte(nodePtr, 8)
            storeLong(pay, n.config.delayNanos)
            storeInt(pay + 8, n.config.inner)
        }
        is PolicyNode.Jitter -> {
            storeByte(nodePtr, 9)
            storeDouble(pay, n.config.factor)
            storeInt(pay + 8, n.config.inner)
        }
        is PolicyNode.FilteredOn -> {
            storeByte(nodePtr, 10)
            val (ptr, len) = lowerRetryPredicateNodes(n.config.predicate) // filtered-config.predicate@0
            storeInt(pay, ptr)
            storeInt(pay + 4, len)
            storeInt(pay + 8, n.config.inner) // inner@8
        }
        is PolicyNode.AndThen -> {
            storeByte(nodePtr, 11)
            storeInt(pay, n.left)
            storeInt(pay + 4, n.right)
        }
        is PolicyNode.PolicyUnion -> {
            storeByte(nodePtr, 12)
            storeInt(pay, n.left)
            storeInt(pay + 4, n.right)
        }
        is PolicyNode.PolicyIntersect -> {
            storeByte(nodePtr, 13)
            storeInt(pay, n.left)
            storeInt(pay + 4, n.right)
        }
    }
}

/** Lowers a retry-policy's node list (element 32B, align 8); returns (dataPtr, len). */
private fun lowerRetryPolicyNodes(policy: RetryPolicy): Pair<Int, Int> {
    if (policy.nodes.isEmpty()) return 0 to 0
    val arr = alloc(policy.nodes.size * NODE_STRIDE, 8)
    policy.nodes.forEachIndexed { i, n -> lowerPolicyNodeInto(arr + i * NODE_STRIDE, n) }
    return arr to policy.nodes.size
}

object RetryApi {
    /** All retry policies active for this agent, in host-defined order. */
    fun getRetryPolicies(): List<NamedRetryPolicy> {
        val ret = alloc(8, 4) // list<named-retry-policy>: {ptr, len}
        hostGetRetryPolicies(ret)
        val dataPtr = loadInt(ret)
        val len = loadInt(ret + 4)
        return (0 until len).map { liftNamedRetryPolicy(dataPtr + it * NAMED_POLICY_STRIDE) }
    }

    /** The named retry policy with [name], or null if none is registered. */
    fun getRetryPolicyByName(name: String): NamedRetryPolicy? {
        val (namePtr, nameLen) = lowerStringToPtrLen(name)
        val ret = alloc(32, 4) // option<named-retry-policy>: tag@0, payload@4 (28B)
        hostGetRetryPolicyByName(namePtr, nameLen, ret)
        return if (loadByte(ret).toInt() == 0) null else liftNamedRetryPolicy(ret + 4)
    }

    /** Removes a named retry policy (persisted to the oplog). No-op if it doesn't exist. */
    fun removeRetryPolicy(name: String) {
        val (namePtr, nameLen) = lowerStringToPtrLen(name)
        hostRemoveRetryPolicy(namePtr, nameLen)
    }

    /** Adds or overwrites a named retry policy (persisted to the oplog). */
    fun setRetryPolicy(policy: NamedRetryPolicy) {
        val (namePtr, nameLen) = lowerStringToPtrLen(policy.name)
        val (predPtr, predLen) = lowerRetryPredicateNodes(policy.predicate)
        val (polPtr, polLen) = lowerRetryPolicyNodes(policy.policy)
        hostSetRetryPolicy(namePtr, nameLen, policy.priority.toInt(), predPtr, predLen, polPtr, polLen)
    }

    /**
     * Resolves the matching retry policy for an operation context (verb + noun URI + dynamic
     * properties), or null if no named rule's predicate matches. [properties] pairs a property
     * name with a [PredicateValue].
     */
    fun resolveRetryPolicy(
        verb: String,
        nounUri: String,
        properties: List<Pair<String, PredicateValue>>,
    ): RetryPolicy? {
        val (verbPtr, verbLen) = lowerStringToPtrLen(verb)
        val (nounPtr, nounLen) = lowerStringToPtrLen(nounUri)
        val (propsPtr, propsLen) = if (properties.isEmpty()) {
            0 to 0
        } else {
            val arr = alloc(properties.size * 24, 8) // tuple<string, predicate-value>: string@0, pv@8, 24B align8
            properties.forEachIndexed { i, (key, pv) ->
                val e = arr + i * 24
                writeStringField(e, 0, key)
                lowerPredicateValueInto(e + 8, pv)
            }
            arr to properties.size
        }
        val ret = alloc(12, 4) // option<retry-policy>: tag@0, payload@4 (retry-policy {nodes list} 8B)
        hostResolveRetryPolicy(verbPtr, verbLen, nounPtr, nounLen, propsPtr, propsLen, ret)
        return if (loadByte(ret).toInt() == 0) null else liftRetryPolicy(ret + 4)
    }
}
