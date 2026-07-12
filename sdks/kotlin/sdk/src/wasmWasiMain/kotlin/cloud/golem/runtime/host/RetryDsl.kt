package cloud.golem.runtime.host

import kotlin.time.Duration
import kotlin.time.Duration.Companion.nanoseconds

/**
 * An idiomatic Kotlin DSL for building `golem:api/retry` policies and predicates, layered on top
 * of [RetryApi]'s flat node-list model. Ported from the Scala SDK's `Retry` object, but recast in
 * Kotlin idiom:
 *
 *  - **`kotlin.time.Duration`** everywhere a delay/limit is taken (`100.milliseconds`, `2.seconds`).
 *  - **infix / operator combinators**: `p1 and p2`, `p1 or p2`, `!p`; `policy andThen other`.
 *  - **value-class [Prop]** with infix comparisons: `Props.statusCode eq 503`, `Props.errorType eq "timeout"`.
 *  - **fail-fast `require`** validation (finite/positive factors, `min <= max`, uint32 ranges,
 *    non-negative durations) instead of Scala's `Either[ValidationError, _]` plumbing.
 *  - **round-trips**: [Policy.toRetryPolicy]/[Predicate.toRetryPredicate] flatten the recursive
 *    tree into the index-referenced node lists; [RetryPolicy.toPolicy]/[RetryPredicate.toPredicate]
 *    rebuild the tree (with cycle detection), so [RetryApi] results read back ergonomically.
 *
 * Example:
 * ```
 * val np = NamedPolicy(
 *     name = "flaky-http",
 *     policy = Policy.exponential(100.milliseconds, factor = 2.0)
 *         .withJitter(0.2)
 *         .maxRetries(5)
 *         .onlyWhen(Props.statusCode eq 503 or (Props.errorType eq "timeout")),
 *     priority = 10,
 * )
 * RetryApi.setRetryPolicy(np)                 // DSL overload -> flattens + calls the host
 * val active = RetryApi.namedPolicies()       // List<NamedPolicy>, rebuilt from the host
 * ```
 */

// ── Predicate tree ───────────────────────────────────────────────────────────────────────────

/** A composable retry predicate. Build leaves via [Props]/[Prop]; combine with `and`/`or`/`!`. */
sealed class Predicate {
    infix fun and(that: Predicate): Predicate = And(this, that)
    infix fun or(that: Predicate): Predicate = Or(this, that)

    /** Enables `!predicate`. */
    operator fun not(): Predicate = Not(this)

    data class Eq(val property: String, val value: PredicateValue) : Predicate()
    data class Neq(val property: String, val value: PredicateValue) : Predicate()
    data class Gt(val property: String, val value: PredicateValue) : Predicate()
    data class Gte(val property: String, val value: PredicateValue) : Predicate()
    data class Lt(val property: String, val value: PredicateValue) : Predicate()
    data class Lte(val property: String, val value: PredicateValue) : Predicate()
    data class Exists(val property: String) : Predicate()
    data class OneOf(val property: String, val values: List<PredicateValue>) : Predicate()
    data class MatchesGlob(val property: String, val pattern: String) : Predicate()
    data class StartsWith(val property: String, val prefix: String) : Predicate()
    data class Contains(val property: String, val substring: String) : Predicate()
    data class And(val left: Predicate, val right: Predicate) : Predicate()
    data class Or(val left: Predicate, val right: Predicate) : Predicate()
    data class Not(val inner: Predicate) : Predicate()

    /** Always matches (`pred-true`). */
    object Always : Predicate()

    /** Never matches (`pred-false`). */
    object Never : Predicate()

    companion object {
        val always: Predicate get() = Always
        val never: Predicate get() = Never
    }
}

/** A retry context property (e.g. `status-code`). Build predicate leaves with its infix operators. */
class Prop(val name: String) {
    infix fun eq(value: PredicateValue): Predicate = Predicate.Eq(name, value)
    infix fun eq(value: String): Predicate = eq(PredicateValue.Text(value))
    infix fun eq(value: Long): Predicate = eq(PredicateValue.Integer(value))
    infix fun eq(value: Int): Predicate = eq(PredicateValue.Integer(value.toLong()))
    infix fun eq(value: Boolean): Predicate = eq(PredicateValue.Bool(value))

    infix fun neq(value: PredicateValue): Predicate = Predicate.Neq(name, value)
    infix fun neq(value: String): Predicate = neq(PredicateValue.Text(value))
    infix fun neq(value: Long): Predicate = neq(PredicateValue.Integer(value))
    infix fun neq(value: Int): Predicate = neq(PredicateValue.Integer(value.toLong()))
    infix fun neq(value: Boolean): Predicate = neq(PredicateValue.Bool(value))

    infix fun gt(value: Long): Predicate = Predicate.Gt(name, PredicateValue.Integer(value))
    infix fun gt(value: Int): Predicate = gt(value.toLong())
    infix fun gte(value: Long): Predicate = Predicate.Gte(name, PredicateValue.Integer(value))
    infix fun gte(value: Int): Predicate = gte(value.toLong())
    infix fun lt(value: Long): Predicate = Predicate.Lt(name, PredicateValue.Integer(value))
    infix fun lt(value: Int): Predicate = lt(value.toLong())
    infix fun lte(value: Long): Predicate = Predicate.Lte(name, PredicateValue.Integer(value))
    infix fun lte(value: Int): Predicate = lte(value.toLong())

    infix fun matchesGlob(pattern: String): Predicate = Predicate.MatchesGlob(name, pattern)
    infix fun startsWith(prefix: String): Predicate = Predicate.StartsWith(name, prefix)
    infix fun contains(substring: String): Predicate = Predicate.Contains(name, substring)

    fun oneOf(vararg values: String): Predicate = Predicate.OneOf(name, values.map { PredicateValue.Text(it) })
    fun oneOf(vararg values: Long): Predicate = Predicate.OneOf(name, values.map { PredicateValue.Integer(it) })
    fun oneOf(vararg values: Int): Predicate = Predicate.OneOf(name, values.map { PredicateValue.Integer(it.toLong()) })

    /** `prop exists` -> the property is present in the context. */
    val exists: Predicate get() = Predicate.Exists(name)
}

/** The retry context properties the host exposes, plus [custom] for anything else. */
object Props {
    val verb = Prop("verb")
    val nounUri = Prop("noun-uri")
    val uriScheme = Prop("uri-scheme")
    val uriHost = Prop("uri-host")
    val uriPort = Prop("uri-port")
    val uriPath = Prop("uri-path")
    val statusCode = Prop("status-code")
    val errorType = Prop("error-type")
    val function = Prop("function")
    val targetComponentId = Prop("target-component-id")
    val targetAgentType = Prop("target-agent-type")
    val dbType = Prop("db-type")
    val trapType = Prop("trap-type")

    fun custom(name: String): Prop = Prop(name)
    operator fun invoke(name: String): Prop = Prop(name)
}

// ── Policy tree ──────────────────────────────────────────────────────────────────────────────

/**
 * A composable retry policy. Start from a base ([Policy.exponential], [Policy.periodic], …) and
 * layer modifiers fluently ([maxRetries], [within], [clamp], [addDelay], [withJitter],
 * [onlyWhen]) or combine whole policies ([andThen], [union], [intersect]).
 */
sealed class Policy {
    /** Cap the total number of retries (`count-box`). [maxRetries] must fit an unsigned 32-bit int. */
    fun maxRetries(maxRetries: Long): Policy = CountBox(requireUint32(maxRetries, "maxRetries"), this)

    /** Give up once [limit] has elapsed (`time-box`). */
    fun within(limit: Duration): Policy = TimeBox(requireNonNegative(limit, "within.limit"), this)

    /** Clamp each computed delay to `[minDelay, maxDelay]`. */
    fun clamp(minDelay: Duration, maxDelay: Duration): Policy {
        requireNonNegative(minDelay, "clamp.minDelay")
        requireNonNegative(maxDelay, "clamp.maxDelay")
        require(minDelay <= maxDelay) { "clamp requires minDelay <= maxDelay, got $minDelay > $maxDelay" }
        return Clamp(minDelay, maxDelay, this)
    }

    /** Add a fixed [delay] on top of each computed delay. */
    fun addDelay(delay: Duration): Policy = AddDelay(requireNonNegative(delay, "addDelay.delay"), this)

    /** Randomize each delay by up to [factor] (0.0..; e.g. 0.2 = ±20%). */
    fun withJitter(factor: Double): Policy = Jitter(requireFactor(factor, "withJitter.factor", allowZero = true), this)

    /** Only apply this policy when [predicate] matches the retry context (`filtered-on`). */
    fun onlyWhen(predicate: Predicate): Policy = FilteredOn(predicate, this)

    /** Fall back to [that] once this policy is exhausted (`and-then`). */
    infix fun andThen(that: Policy): Policy = AndThen(this, that)

    /** Retry if *either* policy would (`policy-union`). */
    infix fun union(that: Policy): Policy = Union(this, that)

    /** Retry only if *both* policies would (`policy-intersect`). */
    infix fun intersect(that: Policy): Policy = Intersect(this, that)

    data class Periodic(val delay: Duration) : Policy()
    data class Exponential(val baseDelay: Duration, val factor: Double) : Policy()
    data class Fibonacci(val first: Duration, val second: Duration) : Policy()
    object Immediate : Policy()
    object Never : Policy()
    data class CountBox(val maxRetries: UInt, val inner: Policy) : Policy()
    data class TimeBox(val limit: Duration, val inner: Policy) : Policy()
    data class Clamp(val minDelay: Duration, val maxDelay: Duration, val inner: Policy) : Policy()
    data class AddDelay(val delay: Duration, val inner: Policy) : Policy()
    data class Jitter(val factor: Double, val inner: Policy) : Policy()
    data class FilteredOn(val predicate: Predicate, val inner: Policy) : Policy()
    data class AndThen(val left: Policy, val right: Policy) : Policy()
    data class Union(val left: Policy, val right: Policy) : Policy()
    data class Intersect(val left: Policy, val right: Policy) : Policy()

    companion object {
        /** Retry immediately with no delay. */
        val immediate: Policy get() = Immediate

        /** Never retry. */
        val never: Policy get() = Never

        /** A fixed [delay] between attempts. */
        fun periodic(delay: Duration): Policy = Periodic(requireNonNegative(delay, "periodic.delay"))

        /** Exponential backoff: `baseDelay * factor^attempt`. [factor] must be finite and > 0. */
        fun exponential(baseDelay: Duration, factor: Double): Policy = Exponential(requireNonNegative(baseDelay, "exponential.baseDelay"), requireFactor(factor, "exponential.factor", allowZero = false))

        /** Fibonacci backoff seeded by [first] and [second]. */
        fun fibonacci(first: Duration, second: Duration): Policy = Fibonacci(requireNonNegative(first, "fibonacci.first"), requireNonNegative(second, "fibonacci.second"))
    }
}

/**
 * A named retry rule: [predicate] selects when it applies, [policy] is the strategy, [priority]
 * controls evaluation order (higher = checked first). Defaults mirror the Scala SDK
 * (`priority = 0`, `predicate = always`).
 */
data class NamedPolicy(
    val name: String,
    val policy: Policy,
    val priority: Long = 0,
    val predicate: Predicate = Predicate.always,
) {
    fun withPriority(value: Long): NamedPolicy = copy(priority = value)
    fun appliesWhen(value: Predicate): NamedPolicy = copy(predicate = value)
}

// ── Flatten (DSL tree -> RetryApi node lists) ─────────────────────────────────────────────────

/** Flattens this predicate tree into a [RetryPredicate] (root = `nodes[0]`, children by index). */
fun Predicate.toRetryPredicate(): RetryPredicate {
    val nodes = ArrayList<PredicateNode>()
    fun append(p: Predicate): Int {
        val index = nodes.size
        nodes.add(PredicateNode.PredFalse) // reserve; overwritten below after children are appended
        nodes[index] = when (p) {
            is Predicate.Eq -> PredicateNode.PropEq(PropertyComparison(p.property, p.value))
            is Predicate.Neq -> PredicateNode.PropNeq(PropertyComparison(p.property, p.value))
            is Predicate.Gt -> PredicateNode.PropGt(PropertyComparison(p.property, p.value))
            is Predicate.Gte -> PredicateNode.PropGte(PropertyComparison(p.property, p.value))
            is Predicate.Lt -> PredicateNode.PropLt(PropertyComparison(p.property, p.value))
            is Predicate.Lte -> PredicateNode.PropLte(PropertyComparison(p.property, p.value))
            is Predicate.Exists -> PredicateNode.PropExists(p.property)
            is Predicate.OneOf -> PredicateNode.PropIn(PropertySetCheck(p.property, p.values))
            is Predicate.MatchesGlob -> PredicateNode.PropMatches(PropertyPattern(p.property, p.pattern))
            is Predicate.StartsWith -> PredicateNode.PropStartsWith(PropertyPattern(p.property, p.prefix))
            is Predicate.Contains -> PredicateNode.PropContains(PropertyPattern(p.property, p.substring))
            is Predicate.And -> PredicateNode.PredAnd(append(p.left), append(p.right))
            is Predicate.Or -> PredicateNode.PredOr(append(p.left), append(p.right))
            is Predicate.Not -> PredicateNode.PredNot(append(p.inner))
            Predicate.Always -> PredicateNode.PredTrue
            Predicate.Never -> PredicateNode.PredFalse
        }
        return index
    }
    append(this)
    return RetryPredicate(nodes)
}

/** Flattens this policy tree into a [RetryPolicy] (root = `nodes[0]`, children by index). */
fun Policy.toRetryPolicy(): RetryPolicy {
    val nodes = ArrayList<PolicyNode>()
    fun append(p: Policy): Int {
        val index = nodes.size
        nodes.add(PolicyNode.Immediate) // reserve; overwritten below after children are appended
        nodes[index] = when (p) {
            is Policy.Periodic -> PolicyNode.Periodic(p.delay.inWholeNanoseconds)
            is Policy.Exponential -> PolicyNode.Exponential(ExponentialConfig(p.baseDelay.inWholeNanoseconds, p.factor))
            is Policy.Fibonacci -> PolicyNode.Fibonacci(FibonacciConfig(p.first.inWholeNanoseconds, p.second.inWholeNanoseconds))
            Policy.Immediate -> PolicyNode.Immediate
            Policy.Never -> PolicyNode.Never
            is Policy.CountBox -> PolicyNode.CountBox(CountBoxConfig(p.maxRetries, append(p.inner)))
            is Policy.TimeBox -> PolicyNode.TimeBox(TimeBoxConfig(p.limit.inWholeNanoseconds, append(p.inner)))
            is Policy.Clamp -> PolicyNode.ClampDelay(ClampConfig(p.minDelay.inWholeNanoseconds, p.maxDelay.inWholeNanoseconds, append(p.inner)))
            is Policy.AddDelay -> PolicyNode.AddDelay(AddDelayConfig(p.delay.inWholeNanoseconds, append(p.inner)))
            is Policy.Jitter -> PolicyNode.Jitter(JitterConfig(p.factor, append(p.inner)))
            is Policy.FilteredOn -> PolicyNode.FilteredOn(FilteredConfig(p.predicate.toRetryPredicate(), append(p.inner)))
            is Policy.AndThen -> PolicyNode.AndThen(append(p.left), append(p.right))
            is Policy.Union -> PolicyNode.PolicyUnion(append(p.left), append(p.right))
            is Policy.Intersect -> PolicyNode.PolicyIntersect(append(p.left), append(p.right))
        }
        return index
    }
    append(this)
    return RetryPolicy(nodes)
}

/** Flattens this named policy into the [NamedRetryPolicy] [RetryApi] expects. */
fun NamedPolicy.toNamedRetryPolicy(): NamedRetryPolicy = NamedRetryPolicy(
    name = name,
    priority = requireUint32(priority, "priority"),
    predicate = predicate.toRetryPredicate(),
    policy = policy.toRetryPolicy(),
)

// ── Unflatten (RetryApi node lists -> DSL tree) ───────────────────────────────────────────────

/** Rebuilds the predicate tree from a flat [RetryPredicate], detecting cycles/out-of-range refs. */
fun RetryPredicate.toPredicate(): Predicate {
    require(nodes.isNotEmpty()) { "retry predicate must contain at least one node" }
    val cache = HashMap<Int, Predicate>()
    val inProgress = HashSet<Int>()
    fun build(i: Int): Predicate {
        require(i in nodes.indices) { "predicate node index $i out of range (${nodes.size} nodes)" }
        cache[i]?.let { return it }
        require(inProgress.add(i)) { "cycle detected at predicate node $i" }
        val p: Predicate = when (val n = nodes[i]) {
            is PredicateNode.PropEq -> Predicate.Eq(n.cmp.propertyName, n.cmp.value)
            is PredicateNode.PropNeq -> Predicate.Neq(n.cmp.propertyName, n.cmp.value)
            is PredicateNode.PropGt -> Predicate.Gt(n.cmp.propertyName, n.cmp.value)
            is PredicateNode.PropGte -> Predicate.Gte(n.cmp.propertyName, n.cmp.value)
            is PredicateNode.PropLt -> Predicate.Lt(n.cmp.propertyName, n.cmp.value)
            is PredicateNode.PropLte -> Predicate.Lte(n.cmp.propertyName, n.cmp.value)
            is PredicateNode.PropExists -> Predicate.Exists(n.propertyName)
            is PredicateNode.PropIn -> Predicate.OneOf(n.check.propertyName, n.check.values)
            is PredicateNode.PropMatches -> Predicate.MatchesGlob(n.pattern.propertyName, n.pattern.pattern)
            is PredicateNode.PropStartsWith -> Predicate.StartsWith(n.pattern.propertyName, n.pattern.pattern)
            is PredicateNode.PropContains -> Predicate.Contains(n.pattern.propertyName, n.pattern.pattern)
            is PredicateNode.PredAnd -> Predicate.And(build(n.left), build(n.right))
            is PredicateNode.PredOr -> Predicate.Or(build(n.left), build(n.right))
            is PredicateNode.PredNot -> Predicate.Not(build(n.inner))
            PredicateNode.PredTrue -> Predicate.Always
            PredicateNode.PredFalse -> Predicate.Never
        }
        inProgress.remove(i)
        cache[i] = p
        return p
    }
    return build(0)
}

/** Rebuilds the policy tree from a flat [RetryPolicy], detecting cycles/out-of-range refs. */
fun RetryPolicy.toPolicy(): Policy {
    require(nodes.isNotEmpty()) { "retry policy must contain at least one node" }
    val cache = HashMap<Int, Policy>()
    val inProgress = HashSet<Int>()
    fun build(i: Int): Policy {
        require(i in nodes.indices) { "policy node index $i out of range (${nodes.size} nodes)" }
        cache[i]?.let { return it }
        require(inProgress.add(i)) { "cycle detected at policy node $i" }
        val p: Policy = when (val n = nodes[i]) {
            is PolicyNode.Periodic -> Policy.Periodic(n.nanos.nanoseconds)
            is PolicyNode.Exponential -> Policy.Exponential(n.config.baseDelayNanos.nanoseconds, n.config.factor)
            is PolicyNode.Fibonacci -> Policy.Fibonacci(n.config.firstNanos.nanoseconds, n.config.secondNanos.nanoseconds)
            PolicyNode.Immediate -> Policy.Immediate
            PolicyNode.Never -> Policy.Never
            is PolicyNode.CountBox -> Policy.CountBox(n.config.maxRetries, build(n.config.inner))
            is PolicyNode.TimeBox -> Policy.TimeBox(n.config.limitNanos.nanoseconds, build(n.config.inner))
            is PolicyNode.ClampDelay -> Policy.Clamp(n.config.minDelayNanos.nanoseconds, n.config.maxDelayNanos.nanoseconds, build(n.config.inner))
            is PolicyNode.AddDelay -> Policy.AddDelay(n.config.delayNanos.nanoseconds, build(n.config.inner))
            is PolicyNode.Jitter -> Policy.Jitter(n.config.factor, build(n.config.inner))
            is PolicyNode.FilteredOn -> Policy.FilteredOn(n.config.predicate.toPredicate(), build(n.config.inner))
            is PolicyNode.AndThen -> Policy.AndThen(build(n.left), build(n.right))
            is PolicyNode.PolicyUnion -> Policy.Union(build(n.left), build(n.right))
            is PolicyNode.PolicyIntersect -> Policy.Intersect(build(n.left), build(n.right))
        }
        inProgress.remove(i)
        cache[i] = p
        return p
    }
    return build(0)
}

/** Rebuilds a [NamedPolicy] from a flat [NamedRetryPolicy]. */
fun NamedRetryPolicy.toNamedPolicy(): NamedPolicy = NamedPolicy(
    name = name,
    policy = policy.toPolicy(),
    priority = priority.toLong(),
    predicate = predicate.toPredicate(),
)

// ── RetryApi ergonomic overloads (DSL-typed) ──────────────────────────────────────────────────

/** Adds or overwrites a named retry policy, flattening the DSL [NamedPolicy] for the host. */
fun RetryApi.setRetryPolicy(policy: NamedPolicy): Unit = setRetryPolicy(policy.toNamedRetryPolicy())

/** All active retry policies as DSL [NamedPolicy] trees. */
fun RetryApi.namedPolicies(): List<NamedPolicy> = getRetryPolicies().map { it.toNamedPolicy() }

/** The named retry policy [name] as a DSL [NamedPolicy] tree, or null. */
fun RetryApi.namedPolicy(name: String): NamedPolicy? = getRetryPolicyByName(name)?.toNamedPolicy()

/** Resolves the matching policy for a context as a DSL [Policy] tree, or null. */
fun RetryApi.resolvePolicy(
    verb: String,
    nounUri: String,
    properties: List<Pair<String, PredicateValue>> = emptyList(),
): Policy? = resolveRetryPolicy(verb, nounUri, properties)?.toPolicy()

// ── Validation helpers (fail-fast, Kotlin-idiomatic) ─────────────────────────────────────────

private fun requireNonNegative(duration: Duration, label: String): Duration {
    require(duration >= Duration.ZERO) { "$label must be a non-negative duration, got $duration" }
    return duration
}

private fun requireFactor(value: Double, label: String, allowZero: Boolean): Double {
    require(value.isFinite()) { "$label must be finite, got $value" }
    if (allowZero) {
        require(value >= 0.0) { "$label must be >= 0, got $value" }
    } else {
        require(value > 0.0) { "$label must be > 0, got $value" }
    }
    return value
}

private fun requireUint32(value: Long, label: String): UInt {
    require(value in 0..0xFFFF_FFFFL) { "$label must fit an unsigned 32-bit integer (0..4294967295), got $value" }
    return value.toUInt()
}
