package cloud.golem.runtime.host

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue
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
}
