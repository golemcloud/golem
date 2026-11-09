package cloud.golem.runtime.host

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue
import kotlin.time.Duration
import kotlin.time.Duration.Companion.days
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Duration.Companion.seconds

/**
 * Pure-logic tests for the RetryDsl flatten/unflatten round-trips + validation. These don't touch
 * any host WIT import, so they run under the plain wasmWasi nodejs runner.
 */
class RetryDslTest {

    @Test
    fun predicate_roundtrips_through_the_flat_node_list() {
        val predicate = (Props.statusCode eq 503) or
            (Props.errorType eq "timeout") and
            !(Props.function startsWith "internal.")
        // Flatten then rebuild; the tree must survive intact.
        assertEquals(predicate, predicate.toRetryPredicate().toPredicate())
    }

    @Test
    fun policy_roundtrips_with_nested_modifiers_and_a_filter() {
        val policy = Policy.exponential(100.milliseconds, factor = 2.0)
            .withJitter(0.2)
            .clamp(50.milliseconds, 5.seconds)
            .maxRetries(5)
            .onlyWhen(Props.statusCode gte 500 and (Props.dbType neq "sqlite"))
            .andThen(Policy.periodic(1.seconds).maxRetries(3))
        assertEquals(policy, policy.toRetryPolicy().toPolicy())
    }

    @Test
    fun named_policy_roundtrips() {
        val named = NamedPolicy(
            name = "flaky-http",
            policy = Policy.fibonacci(100.milliseconds, 200.milliseconds).maxRetries(10),
            priority = 42,
            predicate = Props.uriScheme eq "https",
        ).withPriority(7).appliesWhen(Props.statusCode.oneOf(502, 503, 504))
        assertEquals(named, named.toNamedRetryPolicy().toNamedPolicy())
    }

    @Test
    fun flattened_root_is_node_zero() {
        val flat = Policy.periodic(1.seconds).maxRetries(3).toRetryPolicy()
        // The outermost combinator (count-box) is the root and must sit at index 0.
        assertTrue(flat.nodes[0] is PolicyNode.CountBox)
    }

    @Test
    fun predicate_values_are_typed_by_the_infix_overloads() {
        assertEquals(Predicate.Eq("status-code", PredicateValue.Integer(500)), Props.statusCode eq 500)
        assertEquals(Predicate.Eq("error-type", PredicateValue.Text("x")), Props.errorType eq "x")
        assertEquals(Predicate.Eq("k", PredicateValue.Bool(true)), Props("k") eq true)
    }

    @Test
    fun validation_rejects_bad_inputs() {
        assertFailsWith<IllegalArgumentException> { Policy.exponential(1.seconds, factor = 0.0) } // factor must be > 0
        assertFailsWith<IllegalArgumentException> { Policy.exponential(1.seconds, factor = Double.NaN) } // must be finite
        assertFailsWith<IllegalArgumentException> { Policy.immediate.clamp(5.seconds, 1.seconds) } // min > max
        assertFailsWith<IllegalArgumentException> { Policy.immediate.maxRetries(-1) } // uint32 range
        assertFailsWith<IllegalArgumentException> { Policy.immediate.maxRetries(0x1_0000_0000L) } // uint32 range
    }

    @Test
    fun unflatten_detects_a_cycle() {
        // A hand-built policy whose only node references itself.
        val cyclic = RetryPolicy(listOf(PolicyNode.CountBox(CountBoxConfig(1u, inner = 0))))
        assertFailsWith<IllegalArgumentException> { cyclic.toPolicy() }
    }

    @Test
    fun named_policy_flattens_to_the_wit_node_layout() {
        val named = NamedPolicy(
            "transient-http",
            Policy.exponential(200.milliseconds, 2.0)
                .clamp(100.milliseconds, 5.seconds)
                .onlyWhen(Props.statusCode.oneOf(502, 503, 504))
                .maxRetries(3),
            priority = 10,
        ).appliesWhen((Props.verb eq "GET") and (Props.uriHost matchesGlob "*.example.com"))

        val raw = named.toNamedRetryPolicy()
        assertEquals("transient-http", raw.name)
        assertEquals(10u, raw.priority)
        assertEquals(
            listOf(
                PredicateNode.PredAnd(1, 2),
                PredicateNode.PropEq(PropertyComparison("verb", PredicateValue.Text("GET"))),
                PredicateNode.PropMatches(PropertyPattern("uri-host", "*.example.com")),
            ),
            raw.predicate.nodes,
        )
        assertEquals(
            listOf(
                PolicyNode.CountBox(CountBoxConfig(3u, 1)),
                PolicyNode.FilteredOn(
                    FilteredConfig(
                        RetryPredicate(
                            listOf(
                                PredicateNode.PropIn(
                                    PropertySetCheck("status-code", listOf(502L, 503L, 504L).map { PredicateValue.Integer(it) }),
                                ),
                            ),
                        ),
                        2,
                    ),
                ),
                PolicyNode.ClampDelay(ClampConfig(100_000_000, 5_000_000_000, 3)),
                PolicyNode.Exponential(ExponentialConfig(200_000_000, 2.0)),
            ),
            raw.policy.nodes,
        )
    }

    @Test
    fun exponential_factors_must_be_positive_and_finite() {
        for (factor in listOf(0.0, -1.0, Double.NaN, Double.POSITIVE_INFINITY, Double.NEGATIVE_INFINITY)) {
            assertFailsWith<IllegalArgumentException>("factor $factor") { Policy.exponential(100.milliseconds, factor) }
        }
    }

    @Test
    fun jitter_factors_must_be_non_negative_and_finite() {
        for (factor in listOf(-0.1, Double.NaN, Double.POSITIVE_INFINITY, Double.NEGATIVE_INFINITY)) {
            assertFailsWith<IllegalArgumentException>("factor $factor") { Policy.periodic(100.milliseconds).withJitter(factor) }
        }
        Policy.periodic(100.milliseconds).withJitter(0.0).toRetryPolicy()
    }

    @Test
    fun clamp_requires_min_not_above_max() {
        assertFailsWith<IllegalArgumentException> { Policy.periodic(100.milliseconds).clamp(2.seconds, 1.seconds) }
        Policy.periodic(100.milliseconds).clamp(1.seconds, 1.seconds).toRetryPolicy()
    }

    @Test
    fun durations_that_do_not_fit_the_wit_nanosecond_range_are_rejected() {
        assertFailsWith<IllegalArgumentException>("infinite periodic") { Policy.periodic(Duration.INFINITE) }
        assertFailsWith<IllegalArgumentException>("200000 days") { Policy.periodic(200_000.days) }
        assertFailsWith<IllegalArgumentException>("infinite within") { Policy.immediate.within(Duration.INFINITE) }
    }
}
