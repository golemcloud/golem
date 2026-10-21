// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
use indoc::indoc;
use test_r::test;

use super::*;

#[test]
fn continuation_rejects_untrusted_structures_and_lengths() {
    let policy = RetryPolicy::Immediate;
    let mut prefix = Vec::new();
    desert_rust::serialize(&policy, &mut prefix).unwrap();
    for payload in [
        [vec![b'['; 10000], vec![b']'; 10000]].concat(),
        vec![255; 32],
        b"[0,0]".to_vec(),
        b"[0,0,1".to_vec(),
        b"[0,0,1]false".to_vec(),
        b"[0,0,1,2]".to_vec(),
        b"[0,0,4294967296]".to_vec(),
        b"[0,1000000000,1]".to_vec(),
        format!("[{}0]", "0,".repeat(10000)).into_bytes(),
        vec![b' '; 100000],
    ] {
        let mut token = prefix.clone();
        token.extend(payload);
        assert!(
            policy
                .next_retry_delay(
                    &token,
                    Duration::ZERO,
                    &RetryProperties::new(),
                    &mut FixedRng(0.0)
                )
                .is_err()
        );
    }
}

#[test]
fn continuation_roundtrips_maximum_policy_depth() {
    let mut policy = RetryPolicy::Immediate;
    for _ in 0..63 {
        policy = RetryPolicy::TimeBox {
            limit: Duration::from_secs(10),
            inner: Box::new(policy),
        };
    }
    let props = RetryProperties::new();
    let mut rng = FixedRng(0.0);
    let (token, _) = policy
        .next_retry_delay(&[], Duration::ZERO, &props, &mut rng)
        .unwrap();
    assert_eq!(
        policy
            .next_retry_delay(&token, Duration::from_secs(1), &props, &mut rng)
            .unwrap()
            .1,
        Some(Duration::ZERO)
    );
}

#[test]
fn continuation_roundtrips_maximum_policy_nodes() {
    let mut policy = RetryPolicy::Immediate;
    for _ in 0..11 {
        policy = RetryPolicy::Union(Box::new(policy.clone()), Box::new(policy));
    }
    let policy = RetryPolicy::CountBox {
        max_retries: 3,
        inner: Box::new(policy),
    };
    let props = RetryProperties::new();
    let mut rng = FixedRng(0.0);
    let (token, _) = policy
        .next_retry_delay(&[], Duration::ZERO, &props, &mut rng)
        .unwrap();
    assert_eq!(
        policy
            .next_retry_delay(&token, Duration::ZERO, &props, &mut rng)
            .unwrap()
            .1,
        Some(Duration::ZERO)
    );
}

#[test]
fn continuation_preserves_exact_float_factors() {
    let policy_with_factor = |factor| RetryPolicy::CountBox {
        max_retries: 3,
        inner: Box::new(RetryPolicy::Jitter {
            factor,
            inner: Box::new(RetryPolicy::Exponential {
                base_delay: Duration::from_millis(1),
                factor,
            }),
        }),
    };
    let policy = policy_with_factor(51.248178375505404);
    let props = RetryProperties::new();
    let mut rng = FixedRng(0.5);
    let (state, _) = policy
        .next_retry_delay(&[], Duration::ZERO, &props, &mut rng)
        .unwrap();
    let (state, delay) = policy
        .next_retry_delay(&state, Duration::ZERO, &props, &mut rng)
        .unwrap();
    assert!(delay.is_some());
    assert!(
        policy_with_factor(51.24817837550541)
            .next_retry_delay(&state, Duration::ZERO, &props, &mut rng)
            .unwrap_err()
            .contains("changed")
    );
}

#[test]
fn continuation_preserves_nested_policy_and_elapsed_budget() {
    let policy = RetryPolicy::AndThen(
        Box::new(RetryPolicy::TimeBox {
            limit: Duration::from_secs(5),
            inner: Box::new(RetryPolicy::Immediate),
        }),
        Box::new(RetryPolicy::CountBox {
            max_retries: 2,
            inner: Box::new(RetryPolicy::Periodic(Duration::from_secs(1))),
        }),
    );
    let props = RetryProperties::new();
    let mut rng = FixedRng(0.0);
    let (state, delay) = policy
        .next_retry_delay(&[], Duration::from_secs(4), &props, &mut rng)
        .unwrap();
    assert_eq!(delay, Some(Duration::ZERO));
    let (state, delay) = policy
        .next_retry_delay(&state, Duration::from_secs(5), &props, &mut rng)
        .unwrap();
    assert_eq!(delay, Some(Duration::from_secs(1)));
    let (state, delay) = policy
        .next_retry_delay(&state, Duration::ZERO, &props, &mut rng)
        .unwrap();
    assert_eq!(delay, Some(Duration::from_secs(1)));
    assert_eq!(
        policy
            .next_retry_delay(&state, Duration::ZERO, &props, &mut rng)
            .unwrap()
            .1,
        None
    );
    let changed = RetryPolicy::AndThen(
        Box::new(RetryPolicy::TimeBox {
            limit: Duration::from_secs(6),
            inner: Box::new(RetryPolicy::Immediate),
        }),
        Box::new(RetryPolicy::CountBox {
            max_retries: 2,
            inner: Box::new(RetryPolicy::Periodic(Duration::from_secs(1))),
        }),
    );
    assert!(
        changed
            .next_retry_delay(&state, Duration::ZERO, &props, &mut rng)
            .unwrap_err()
            .contains("changed")
    );
    for changed in [
        RetryPolicy::CountBox {
            max_retries: 3,
            inner: Box::new(RetryPolicy::Immediate),
        },
        RetryPolicy::CountBox {
            max_retries: 2,
            inner: Box::new(RetryPolicy::Periodic(Duration::from_secs(2))),
        },
    ] {
        let original = RetryPolicy::CountBox {
            max_retries: 2,
            inner: Box::new(RetryPolicy::Immediate),
        };
        let (token, _) = original
            .next_retry_delay(&[], Duration::ZERO, &props, &mut rng)
            .unwrap();
        assert!(
            changed
                .next_retry_delay(&token, Duration::ZERO, &props, &mut rng)
                .unwrap_err()
                .contains("changed")
        );
    }
    assert!(policy.is_bounded());
    assert!(
        !RetryPolicy::Union(Box::new(policy.clone()), Box::new(RetryPolicy::Immediate))
            .is_bounded()
    );
    assert!(
        RetryPolicy::Intersect(Box::new(policy), Box::new(RetryPolicy::Immediate)).is_bounded()
    );
}

/// Counts how many `Retry` verdicts the policy issues before it gives up.
fn count_retries(policy: &RetryPolicy) -> usize {
    let mut state = policy.initial_state();
    let mut rng = FixedRng(0.0);
    let props = RetryProperties::new();
    let mut retries = 0;
    loop {
        let (new_state, verdict) = policy.step(&state, Duration::ZERO, &props, &mut rng);
        state = new_state;
        match verdict {
            RetryVerdict::Retry(_) => retries += 1,
            RetryVerdict::GiveUp => return retries,
            RetryVerdict::Error(err) => panic!("unexpected policy error: {err:?}"),
        }
        assert!(retries < 100, "policy never gave up");
    }
}

/// Legacy `RetryConfig::max_attempts` counts *total* physical attempts (`get_delay` returns
/// `None` once `attempts >= max_attempts`), so the synthesized default policy must issue
/// exactly `max_attempts - 1` retries. `max_attempts: 1` is the boundary: one attempt, no
/// retries at all.
#[test]
fn synthesized_default_policy_matches_legacy_attempt_count() {
    for (max_attempts, expected_retries) in [(1u32, 0usize), (3, 2)] {
        let config = RetryConfig {
            max_attempts,
            min_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(2),
            multiplier: 2.0,
            max_jitter_factor: None,
        };

        let legacy_retries = (1..)
            .map_while(|attempts| crate::retries::get_delay(&config, attempts))
            .count();
        assert_eq!(
            legacy_retries, expected_retries,
            "legacy get_delay loop for max_attempts={max_attempts}"
        );

        let policy = RetryPolicy::from(config);
        assert_eq!(
            count_retries(&policy),
            expected_retries,
            "synthesized policy for max_attempts={max_attempts}"
        );
    }
}

#[test]
fn periodic_strategy_retries_with_constant_delay() {
    let policy = RetryPolicy::Periodic(Duration::from_millis(10));
    let mut state = policy.initial_state();
    let mut rng = FixedRng(0.0);
    let props = RetryProperties::new();

    for _ in 0..3 {
        let (new_state, verdict) = policy.step(&state, Duration::ZERO, &props, &mut rng);
        state = new_state;
        assert_eq!(verdict, RetryVerdict::Retry(Duration::from_millis(10)));
    }
}

#[test]
fn exponential_strategy_scales_delay() {
    let policy = RetryPolicy::Exponential {
        base_delay: Duration::from_millis(10),
        factor: 2.0,
    };
    let mut state = policy.initial_state();
    let mut rng = FixedRng(0.0);
    let props = RetryProperties::new();

    let mut delays = Vec::new();
    for _ in 0..4 {
        let (new_state, verdict) = policy.step(&state, Duration::ZERO, &props, &mut rng);
        state = new_state;
        match verdict {
            RetryVerdict::Retry(delay) => delays.push(delay),
            other => panic!("expected retry verdict, got {other:?}"),
        }
    }

    assert_eq!(
        delays,
        vec![
            Duration::from_millis(10),
            Duration::from_millis(20),
            Duration::from_millis(40),
            Duration::from_millis(80)
        ]
    );
}

#[test]
fn exponential_strategy_saturates_for_large_counters() {
    // Regression test: with `Policy::exponential(1s, 2.0).clamp(500ms, 10s)` retried
    // forever, after ~64 attempts the unclamped exponential delay overflowed
    // `Duration::MAX.as_secs_f64()` and `Duration::from_secs_f64` panicked before
    // the surrounding clamp could be applied.
    let policy = RetryPolicy::Clamp {
        min_delay: Duration::from_millis(500),
        max_delay: Duration::from_secs(10),
        inner: Box::new(RetryPolicy::Exponential {
            base_delay: Duration::from_secs(1),
            factor: 2.0,
        }),
    };
    let mut state = policy.initial_state();
    let mut rng = FixedRng(0.0);
    let props = RetryProperties::new();

    for _ in 0..2_000 {
        let (new_state, verdict) = policy.step(&state, Duration::ZERO, &props, &mut rng);
        state = new_state;
        match verdict {
            RetryVerdict::Retry(delay) => {
                assert!(delay >= Duration::from_millis(500));
                assert!(delay <= Duration::from_secs(10));
            }
            other => panic!("expected retry verdict, got {other:?}"),
        }
    }
}

#[test]
fn fibonacci_strategy_scales_delay() {
    let policy = RetryPolicy::Fibonacci {
        first: Duration::from_millis(5),
        second: Duration::from_millis(10),
    };
    let mut state = policy.initial_state();
    let mut rng = FixedRng(0.0);
    let props = RetryProperties::new();

    let mut delays = Vec::new();
    for _ in 0..6 {
        let (new_state, verdict) = policy.step(&state, Duration::ZERO, &props, &mut rng);
        state = new_state;
        match verdict {
            RetryVerdict::Retry(delay) => delays.push(delay),
            other => panic!("expected retry verdict, got {other:?}"),
        }
    }

    assert_eq!(
        delays,
        vec![
            Duration::from_millis(5),
            Duration::from_millis(10),
            Duration::from_millis(15),
            Duration::from_millis(25),
            Duration::from_millis(40),
            Duration::from_millis(65)
        ]
    );
}

#[test]
fn count_box_exhausts_after_max_retries() {
    let policy = RetryPolicy::CountBox {
        max_retries: 2,
        inner: Box::new(RetryPolicy::Immediate),
    };
    let mut state = policy.initial_state();
    let mut rng = FixedRng(0.0);
    let props = RetryProperties::new();

    let (state_1, verdict_1) = policy.step(&state, Duration::ZERO, &props, &mut rng);
    assert_eq!(verdict_1, RetryVerdict::Retry(Duration::ZERO));

    let (state_2, verdict_2) = policy.step(&state_1, Duration::ZERO, &props, &mut rng);
    assert_eq!(verdict_2, RetryVerdict::Retry(Duration::ZERO));

    let (_, verdict_3) = policy.step(&state_2, Duration::ZERO, &props, &mut rng);
    assert_eq!(verdict_3, RetryVerdict::GiveUp);

    state = state_2;
    let _ = state;
}

#[test]
fn time_box_exhausts_after_elapsed_limit() {
    let policy = RetryPolicy::TimeBox {
        limit: Duration::from_secs(5),
        inner: Box::new(RetryPolicy::Immediate),
    };
    let state = policy.initial_state();
    let mut rng = FixedRng(0.0);
    let props = RetryProperties::new();

    let (_, verdict_before_limit) = policy.step(&state, Duration::from_secs(4), &props, &mut rng);
    assert_eq!(verdict_before_limit, RetryVerdict::Retry(Duration::ZERO));

    let (_, verdict_after_limit) = policy.step(&state, Duration::from_secs(5), &props, &mut rng);
    assert_eq!(verdict_after_limit, RetryVerdict::GiveUp);
}

#[test]
fn clamp_add_delay_and_jitter_transform_retry_delay() {
    let policy = RetryPolicy::Jitter {
        factor: 0.1,
        inner: Box::new(RetryPolicy::AddDelay {
            delay: Duration::from_millis(5),
            inner: Box::new(RetryPolicy::Clamp {
                min_delay: Duration::from_millis(10),
                max_delay: Duration::from_millis(30),
                inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(50))),
            }),
        }),
    };

    let mut rng = FixedRng(0.1);
    let props = RetryProperties::new();
    let state = policy.initial_state();
    let (_, verdict) = policy.step(&state, Duration::ZERO, &props, &mut rng);

    assert_eq!(verdict, RetryVerdict::Retry(Duration::from_micros(38_500)));
}

#[test]
fn filtered_on_uses_predicate_and_propagates_errors() {
    let policy = RetryPolicy::FilteredOn {
        predicate: Predicate::PropEq {
            property: "status-code".to_string(),
            value: PredicateValue::Integer(503),
        },
        inner: Box::new(RetryPolicy::Immediate),
    };

    let mut rng = FixedRng(0.0);
    let mut props = RetryProperties::new();
    props.set("status-code", PredicateValue::Integer(503));

    let state = policy.initial_state();
    let (_, verdict_ok) = policy.step(&state, Duration::ZERO, &props, &mut rng);
    assert_eq!(verdict_ok, RetryVerdict::Retry(Duration::ZERO));

    props.set("status-code", PredicateValue::Integer(404));
    let (_, verdict_give_up) = policy.step(&state, Duration::ZERO, &props, &mut rng);
    assert_eq!(verdict_give_up, RetryVerdict::GiveUp);

    let empty_props = RetryProperties::new();
    let (_, verdict_err) = policy.step(&state, Duration::ZERO, &empty_props, &mut rng);
    match verdict_err {
        RetryVerdict::Error(RetryEvaluationError::PropertyNotFound { property }) => {
            assert_eq!(property, "status-code");
        }
        other => panic!("expected property-not-found error, got {other:?}"),
    }
}

#[test]
fn and_then_hands_off_to_right_immediately() {
    let left = RetryPolicy::CountBox {
        max_retries: 1,
        inner: Box::new(RetryPolicy::Immediate),
    };
    let right = RetryPolicy::Periodic(Duration::from_millis(20));
    let policy = RetryPolicy::AndThen(Box::new(left), Box::new(right));

    let mut rng = FixedRng(0.0);
    let props = RetryProperties::new();
    let state = policy.initial_state();

    let (state_1, verdict_1) = policy.step(&state, Duration::ZERO, &props, &mut rng);
    assert_eq!(verdict_1, RetryVerdict::Retry(Duration::ZERO));

    let (_, verdict_2) = policy.step(&state_1, Duration::ZERO, &props, &mut rng);
    assert_eq!(verdict_2, RetryVerdict::Retry(Duration::from_millis(20)));
}

#[test]
fn union_and_intersect_combine_delays() {
    let left = RetryPolicy::CountBox {
        max_retries: 1,
        inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(10))),
    };
    let right = RetryPolicy::Periodic(Duration::from_millis(20));

    let union = RetryPolicy::Union(Box::new(left.clone()), Box::new(right.clone()));
    let intersect = RetryPolicy::Intersect(Box::new(left), Box::new(right));

    let mut rng = FixedRng(0.0);
    let props = RetryProperties::new();

    let (_, union_first) = union.step(&union.initial_state(), Duration::ZERO, &props, &mut rng);
    assert_eq!(union_first, RetryVerdict::Retry(Duration::from_millis(10)));

    let (_, intersect_first) =
        intersect.step(&intersect.initial_state(), Duration::ZERO, &props, &mut rng);
    assert_eq!(
        intersect_first,
        RetryVerdict::Retry(Duration::from_millis(20))
    );
}

#[test]
fn predicate_comparisons_and_text_operators_work() {
    let mut props = RetryProperties::new();
    props
        .set("status", PredicateValue::Integer(503))
        .set("service", PredicateValue::Text("billing-api".to_string()));

    assert!(
        Predicate::PropGt {
            property: "status".to_string(),
            value: PredicateValue::Integer(500),
        }
        .matches(&props)
        .unwrap()
    );

    assert!(
        Predicate::PropMatches {
            property: "service".to_string(),
            pattern: "billing-*".to_string(),
        }
        .matches(&props)
        .unwrap()
    );

    assert!(
        Predicate::PropStartsWith {
            property: "service".to_string(),
            prefix: "bill".to_string(),
        }
        .matches(&props)
        .unwrap()
    );

    assert!(
        Predicate::PropContains {
            property: "service".to_string(),
            substring: "api".to_string(),
        }
        .matches(&props)
        .unwrap()
    );
}

#[test]
fn predicate_references_property_detects_explicit_reference() {
    // Direct leaf reference
    assert!(
        Predicate::PropEq {
            property: "status-code".to_string(),
            value: PredicateValue::Integer(500),
        }
        .references_property("status-code")
    );
    assert!(Predicate::PropExists("status-code".to_string()).references_property("status-code"));
    assert!(
        Predicate::PropIn {
            property: "status-code".to_string(),
            values: vec![PredicateValue::Integer(500), PredicateValue::Integer(503)],
        }
        .references_property("status-code")
    );

    // Different property — does not reference
    assert!(
        !Predicate::PropEq {
            property: "error-type".to_string(),
            value: PredicateValue::Text("transient".to_string()),
        }
        .references_property("status-code")
    );

    // Catch-all predicates do not reference any property
    assert!(!Predicate::True.references_property("status-code"));
    assert!(!Predicate::False.references_property("status-code"));
}

#[test]
fn predicate_references_property_traverses_composites() {
    let status_eq = Predicate::PropEq {
        property: "status-code".to_string(),
        value: PredicateValue::Integer(500),
    };
    let error_eq = Predicate::PropEq {
        property: "error-type".to_string(),
        value: PredicateValue::Text("transient".to_string()),
    };

    // And/Or detect references in either side
    assert!(
        Predicate::And(Box::new(status_eq.clone()), Box::new(error_eq.clone()))
            .references_property("status-code")
    );
    assert!(
        Predicate::Or(Box::new(error_eq.clone()), Box::new(status_eq.clone()))
            .references_property("status-code")
    );

    // Not detects reference inside negated predicate
    assert!(Predicate::Not(Box::new(status_eq.clone())).references_property("status-code"));

    // Composite of catch-all + unrelated property does not reference
    assert!(
        !Predicate::And(Box::new(Predicate::True), Box::new(error_eq.clone()))
            .references_property("status-code")
    );
    assert!(
        !Predicate::Or(Box::new(error_eq.clone()), Box::new(Predicate::True))
            .references_property("status-code")
    );
}

#[test]
fn retry_policy_references_property_recurses_through_policy_combinators() {
    let status_eq = Predicate::PropEq {
        property: "status-code".to_string(),
        value: PredicateValue::Integer(503),
    };
    let inner = RetryPolicy::Periodic(Duration::from_millis(100));

    // FilteredOn carries a predicate
    let filtered = RetryPolicy::FilteredOn {
        predicate: status_eq.clone(),
        inner: Box::new(inner.clone()),
    };
    assert!(filtered.references_property("status-code"));
    assert!(!filtered.references_property("error-type"));

    // Wrapping combinators recurse into inner
    let wrapped = RetryPolicy::CountBox {
        max_retries: 3,
        inner: Box::new(filtered.clone()),
    };
    assert!(wrapped.references_property("status-code"));

    let wrapped2 = RetryPolicy::TimeBox {
        limit: Duration::from_secs(1),
        inner: Box::new(filtered.clone()),
    };
    assert!(wrapped2.references_property("status-code"));

    // Binary combinators recurse into both sides
    let union = RetryPolicy::Union(
        Box::new(RetryPolicy::Periodic(Duration::from_millis(50))),
        Box::new(filtered.clone()),
    );
    assert!(union.references_property("status-code"));

    // Pure delay/timing policies have no predicate references
    assert!(!RetryPolicy::Periodic(Duration::from_millis(100)).references_property("status-code"));
    assert!(
        !RetryPolicy::Exponential {
            base_delay: Duration::from_millis(100),
            factor: 2.0,
        }
        .references_property("status-code")
    );
    assert!(!RetryPolicy::Never.references_property("status-code"));
}

#[test]
fn coercion_supports_text_integer_and_rejects_boolean_to_integer() {
    let mut props = RetryProperties::new();
    props.set("attempt", PredicateValue::Text("42".to_string()));

    assert!(
        Predicate::PropEq {
            property: "attempt".to_string(),
            value: PredicateValue::Integer(42),
        }
        .matches(&props)
        .unwrap()
    );

    props.set("attempt", PredicateValue::Integer(42));
    assert!(
        Predicate::PropEq {
            property: "attempt".to_string(),
            value: PredicateValue::Text("42".to_string()),
        }
        .matches(&props)
        .unwrap()
    );

    props.set("attempt", PredicateValue::Boolean(true));
    let error = Predicate::PropEq {
        property: "attempt".to_string(),
        value: PredicateValue::Integer(42),
    }
    .matches(&props)
    .unwrap_err();

    match error {
        RetryEvaluationError::CoercionFailed {
            property,
            target_type,
            ..
        } => {
            assert_eq!(property, "attempt");
            assert_eq!(target_type, PredicateValueType::Integer);
        }
        other => panic!("expected coercion error, got {other:?}"),
    }
}

#[test]
fn prop_in_propagates_coercion_error() {
    let mut props = RetryProperties::new();
    props.set("attempt", PredicateValue::Boolean(true));

    let error = Predicate::PropIn {
        property: "attempt".to_string(),
        values: vec![PredicateValue::Integer(1), PredicateValue::Integer(2)],
    }
    .matches(&props)
    .unwrap_err();

    assert!(matches!(
        error,
        RetryEvaluationError::CoercionFailed {
            target_type: PredicateValueType::Integer,
            ..
        }
    ));
}

#[test]
fn errors_propagate_through_filtered_union_and_intersect() {
    let missing_predicate = Predicate::PropEq {
        property: "missing".to_string(),
        value: PredicateValue::Integer(1),
    };
    let filtered = RetryPolicy::FilteredOn {
        predicate: missing_predicate,
        inner: Box::new(RetryPolicy::Immediate),
    };
    let union = RetryPolicy::Union(Box::new(filtered.clone()), Box::new(RetryPolicy::Immediate));
    let intersect = RetryPolicy::Intersect(Box::new(filtered), Box::new(RetryPolicy::Immediate));

    let props = RetryProperties::new();
    let mut rng = FixedRng(0.0);

    let (_, union_verdict) = union.step(&union.initial_state(), Duration::ZERO, &props, &mut rng);
    assert!(matches!(
        union_verdict,
        RetryVerdict::Error(RetryEvaluationError::PropertyNotFound { .. })
    ));

    let (_, intersect_verdict) =
        intersect.step(&intersect.initial_state(), Duration::ZERO, &props, &mut rng);
    assert!(matches!(
        intersect_verdict,
        RetryVerdict::Error(RetryEvaluationError::PropertyNotFound { .. })
    ));
}

#[test]
fn named_policy_resolution_respects_priority_order() {
    let high = NamedRetryPolicy {
        name: "high".to_string(),
        priority: 100,
        predicate: Predicate::PropEq {
            property: "verb".to_string(),
            value: PredicateValue::Text("invoke".to_string()),
        },
        policy: RetryPolicy::Immediate,
    };
    let low = NamedRetryPolicy {
        name: "low".to_string(),
        priority: 10,
        predicate: Predicate::True,
        policy: RetryPolicy::Never,
    };

    let mut props = RetryProperties::new();
    props.set("verb", PredicateValue::Text("invoke".to_string()));

    let policies = vec![high.clone(), low.clone()];
    let resolved = NamedRetryPolicy::resolve(&policies, &props)
        .expect("resolution should succeed")
        .expect("matching policy is expected");
    assert_eq!(resolved.name, "high");

    props.set("verb", PredicateValue::Text("query".to_string()));
    let resolved = NamedRetryPolicy::resolve(&policies, &props)
        .expect("resolution should succeed")
        .expect("fallback policy is expected");
    assert_eq!(resolved.name, "low");

    let no_match_policies = vec![NamedRetryPolicy {
        name: "none".to_string(),
        priority: 0,
        predicate: Predicate::False,
        policy: RetryPolicy::Never,
    }];
    let no_match =
        NamedRetryPolicy::resolve(&no_match_policies, &props).expect("resolution should succeed");
    assert!(no_match.is_none());
}

#[test]
fn resolve_treating_missing_properties_as_no_match_skips_property_not_found() {
    // A status-code-keyed policy that would normally apply to outgoing HTTP
    // responses, plus a fallback policy keyed on the trap context.
    let status_code_policy = NamedRetryPolicy {
        name: "http-5xx".to_string(),
        priority: 100,
        predicate: Predicate::PropEq {
            property: "status-code".to_string(),
            value: PredicateValue::Integer(500),
        },
        policy: RetryPolicy::Immediate,
    };
    let trap_policy = NamedRetryPolicy {
        name: "trap-fallback".to_string(),
        priority: 10,
        predicate: Predicate::PropEq {
            property: "trap-type".to_string(),
            value: PredicateValue::Text("transient-error".to_string()),
        },
        policy: RetryPolicy::Immediate,
    };

    let policies = vec![status_code_policy.clone(), trap_policy.clone()];

    // Trap context: only `trap-type` is populated, no `status-code`.
    let mut trap_props = RetryProperties::new();
    trap_props.set(
        "trap-type",
        PredicateValue::Text("transient-error".to_string()),
    );

    // The strict resolver errors because `status-code` is referenced but missing.
    let strict = NamedRetryPolicy::resolve(&policies, &trap_props);
    assert!(matches!(
        strict,
        Err(RetryEvaluationError::PropertyNotFound { ref property }) if property == "status-code"
    ));

    // The lenient resolver skips the status-code policy and selects the trap one.
    let resolved =
        NamedRetryPolicy::resolve_treating_missing_properties_as_no_match(&policies, &trap_props)
            .expect("resolution should succeed")
            .expect("trap-fallback policy should be selected");
    assert_eq!(resolved.name, "trap-fallback");

    // When no policy matches and missing properties are skipped, returns None
    // instead of an error.
    let unmatched_props = RetryProperties::new();
    let resolved = NamedRetryPolicy::resolve_treating_missing_properties_as_no_match(
        &policies,
        &unmatched_props,
    )
    .expect("resolution should succeed");
    assert!(resolved.is_none());
}

#[test]
fn resolve_applicable_treating_missing_properties_skips_policy_body_property_references() {
    // Status-code retry policies can encode their status predicate inside
    // the policy body rather than in the named-policy selector. Trap
    // recovery has no `status-code` property, so this policy cannot apply
    // there and must be skipped in favour of the trap/default fallback.
    let status_code_policy = NamedRetryPolicy {
        name: "http-5xx-filtered-body".to_string(),
        priority: 100,
        predicate: Predicate::True,
        policy: RetryPolicy::FilteredOn {
            predicate: Predicate::PropEq {
                property: "status-code".to_string(),
                value: PredicateValue::Integer(500),
            },
            inner: Box::new(RetryPolicy::Immediate),
        },
    };
    let trap_policy = NamedRetryPolicy {
        name: "trap-fallback".to_string(),
        priority: 10,
        predicate: Predicate::PropEq {
            property: "trap-type".to_string(),
            value: PredicateValue::Text("transient-error".to_string()),
        },
        policy: RetryPolicy::Immediate,
    };

    let mut trap_props = RetryProperties::new();
    trap_props.set(
        "trap-type",
        PredicateValue::Text("transient-error".to_string()),
    );

    let policies = vec![status_code_policy, trap_policy];
    let resolved = NamedRetryPolicy::resolve_applicable_treating_missing_properties_as_no_match(
        &policies,
        &trap_props,
    )
    .expect("resolution should succeed")
    .expect("trap fallback should be selected when status-code is missing");

    assert_eq!(resolved.name, "trap-fallback");
}

#[test]
fn lenient_predicate_not_missing_property_does_not_match() {
    let predicate = Predicate::Not(Box::new(Predicate::PropEq {
        property: "status-code".to_string(),
        value: PredicateValue::Integer(500),
    }));

    assert!(
        !predicate
            .matches_treating_missing_properties_as_false(&RetryProperties::new())
            .expect("lenient predicate evaluation should succeed"),
        "missing properties under Not must remain a no-match"
    );

    let mut props = RetryProperties::new();
    props.set("status-code", PredicateValue::Integer(404));
    assert!(
        predicate
            .matches_treating_missing_properties_as_false(&props)
            .expect("lenient predicate evaluation should succeed"),
        "Not should still match when the referenced property exists and does not match"
    );
}

#[test]
fn resolve_treating_missing_properties_as_no_match_propagates_other_errors() {
    // A predicate that would fail with a coercion error (text vs integer),
    // not with PropertyNotFound. That kind of error must still surface.
    let bad_policy = NamedRetryPolicy {
        name: "type-mismatch".to_string(),
        priority: 100,
        predicate: Predicate::PropGt {
            property: "verb".to_string(),
            value: PredicateValue::Integer(0),
        },
        policy: RetryPolicy::Immediate,
    };

    let mut props = RetryProperties::new();
    props.set("verb", PredicateValue::Text("trap".to_string()));

    let resolved = NamedRetryPolicy::resolve_treating_missing_properties_as_no_match(
        std::slice::from_ref(&bad_policy),
        &props,
    );
    assert!(matches!(
        resolved,
        Err(RetryEvaluationError::CoercionFailed { .. })
    ));
}

#[test]
fn retry_policy_accepts_humantime_duration_strings() {
    // Periodic — Duration is the bare payload (not inside a struct).
    let json = r#"{ "periodic": "200ms" }"#;
    let parsed: RetryPolicy = serde_json::from_str(json).expect("should parse humantime");
    assert_eq!(parsed, RetryPolicy::Periodic(Duration::from_millis(200)));

    // Exponential — Duration is a struct field with humantime form.
    let json = r#"{ "exponential": { "baseDelay": "500ms", "factor": 2.0 } }"#;
    let parsed: RetryPolicy = serde_json::from_str(json).expect("should parse humantime");
    assert_eq!(
        parsed,
        RetryPolicy::Exponential {
            base_delay: Duration::from_millis(500),
            factor: 2.0,
        }
    );

    // Clamp + nested exponential.
    let json = r#"{
            "clamp": {
                "minDelay": "100ms",
                "maxDelay": "5s",
                "inner": { "exponential": { "baseDelay": "200ms", "factor": 2.0 } }
            }
        }"#;
    let parsed: RetryPolicy = serde_json::from_str(json).expect("should parse humantime");
    assert_eq!(
        parsed,
        RetryPolicy::Clamp {
            min_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(5),
            inner: Box::new(RetryPolicy::Exponential {
                base_delay: Duration::from_millis(200),
                factor: 2.0,
            }),
        }
    );

    // Fibonacci.
    let json = r#"{ "fibonacci": { "first": "1s", "second": "2s" } }"#;
    let parsed: RetryPolicy = serde_json::from_str(json).expect("should parse humantime");
    assert_eq!(
        parsed,
        RetryPolicy::Fibonacci {
            first: Duration::from_secs(1),
            second: Duration::from_secs(2),
        }
    );

    // TimeBox.
    let json = r#"{ "timeBox": { "limit": "30s", "inner": "immediate" } }"#;
    let parsed: RetryPolicy = serde_json::from_str(json).expect("should parse humantime");
    assert_eq!(
        parsed,
        RetryPolicy::TimeBox {
            limit: Duration::from_secs(30),
            inner: Box::new(RetryPolicy::Immediate),
        }
    );

    // AddDelay.
    let json = r#"{ "addDelay": { "delay": "750ms", "inner": "immediate" } }"#;
    let parsed: RetryPolicy = serde_json::from_str(json).expect("should parse humantime");
    assert_eq!(
        parsed,
        RetryPolicy::AddDelay {
            delay: Duration::from_millis(750),
            inner: Box::new(RetryPolicy::Immediate),
        }
    );
}

#[test]
fn retry_policy_still_accepts_struct_duration_form() {
    // Both string and struct forms must be accepted to preserve backward
    // compatibility with the Rust `Duration` JSON serialization.
    let json = r#"{
            "exponential": {
                "baseDelay": { "secs": 1, "nanos": 500000000 },
                "factor": 2.0
            }
        }"#;
    let parsed: RetryPolicy = serde_json::from_str(json).expect("should parse struct form");
    assert_eq!(
        parsed,
        RetryPolicy::Exponential {
            base_delay: Duration::from_millis(1500),
            factor: 2.0,
        }
    );

    // Periodic struct form.
    let json = r#"{ "periodic": { "secs": 1, "nanos": 0 } }"#;
    let parsed: RetryPolicy = serde_json::from_str(json).expect("should parse struct form");
    assert_eq!(parsed, RetryPolicy::Periodic(Duration::from_secs(1)));
}

#[test]
fn retry_policy_rejects_invalid_duration_string() {
    let json = r#"{ "periodic": "definitely not a duration" }"#;
    let result: Result<RetryPolicy, _> = serde_json::from_str(json);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("invalid duration string"),
        "expected duration error, got: {err}"
    );
}

#[test]
fn fixed_rng_makes_jitter_deterministic() {
    let policy = RetryPolicy::Jitter {
        factor: 0.5,
        inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(100))),
    };

    let mut rng = FixedRng(0.25);
    let props = RetryProperties::new();
    let (_, verdict) = policy.step(&policy.initial_state(), Duration::ZERO, &props, &mut rng);
    assert_eq!(verdict, RetryVerdict::Retry(Duration::from_millis(125)));
}

#[test]
fn retry_context_populates_expected_http_fields() {
    let props = RetryContext::http_with_response(
        "GET",
        "https://api.example.com:8443/v1/items",
        Some(503),
        "transient",
    );

    assert_eq!(
        props.get("verb"),
        Some(&PredicateValue::Text("GET".to_string()))
    );
    assert_eq!(
        props.get("uri-scheme"),
        Some(&PredicateValue::Text("https".to_string()))
    );
    assert_eq!(
        props.get("uri-host"),
        Some(&PredicateValue::Text("api.example.com".to_string()))
    );
    assert_eq!(props.get("uri-port"), Some(&PredicateValue::Integer(8443)));
    assert_eq!(
        props.get("uri-path"),
        Some(&PredicateValue::Text("/v1/items".to_string()))
    );
    assert_eq!(
        props.get("status-code"),
        Some(&PredicateValue::Integer(503))
    );
    assert_eq!(
        props.get("error-type"),
        Some(&PredicateValue::Text("transient".to_string()))
    );
}

#[test]
fn retry_predicate_json_and_yaml_example() {
    let model = Predicate::And(
        Box::new(Predicate::True),
        Box::new(Predicate::PropEq {
            property: "status".to_string(),
            value: PredicateValue::Text("transient".to_string()),
        }),
    );

    let json_example = serde_json::json!({
        "and": [
            true,
            {
                "propEq": {
                    "property": "status",
                    "value": "transient"
                }
            }
        ]
    });

    let yaml_example = indoc! {r#"
            and:
              - true
              - propEq:
                  property: status
                  value: transient
        "#};

    let from_json =
        serde_json::from_value::<Predicate>(json_example.clone()).expect("json should parse");
    let from_yaml = serde_yaml::from_str::<Predicate>(yaml_example).expect("yaml should parse");

    assert_eq!(from_json, model);
    assert_eq!(from_yaml, model);

    let normalized = serde_json::to_value(model).expect("predicate should serialize");
    assert_eq!(normalized, json_example);
}

#[test]
fn retry_policy_json_and_yaml_example() {
    let model = RetryPolicy::AndThen(
        Box::new(RetryPolicy::Periodic(Duration::from_secs(1))),
        Box::new(RetryPolicy::CountBox {
            max_retries: 3,
            inner: Box::new(RetryPolicy::Never),
        }),
    );

    let json_example = serde_json::json!({
        "andThen": [
            {
                "periodic": {
                    "secs": 1,
                    "nanos": 0
                }
            },
            {
                "countBox": {
                    "maxRetries": 3,
                    "inner": "never"
                }
            }
        ]
    });

    let yaml_example = indoc! {r#"
            andThen:
              - periodic:
                  secs: 1
                  nanos: 0
              - countBox:
                  maxRetries: 3
                  inner: never
        "#};

    let from_json =
        serde_json::from_value::<RetryPolicy>(json_example.clone()).expect("json should parse");
    let from_yaml = serde_yaml::from_str::<RetryPolicy>(yaml_example).expect("yaml should parse");

    assert_eq!(from_json, model);
    assert_eq!(from_yaml, model);

    let normalized = serde_json::to_value(model).expect("retry policy should serialize");
    assert_eq!(normalized, json_example);
}

#[test]
fn retry_policy_yaml_roundtrip_is_plain_yaml() {
    let model = RetryPolicy::FilteredOn {
        predicate: Predicate::False,
        inner: Box::new(RetryPolicy::Immediate),
    };

    let yaml = serde_yaml::to_string(&model).expect("retry policy yaml serialization");
    assert!(!yaml.contains('!'));

    let reparsed =
        serde_yaml::from_str::<RetryPolicy>(&yaml).expect("serialized yaml should parse");
    assert_eq!(reparsed, model);
}

#[test]
fn predicate_units_are_lenient_and_normalized() {
    let json_bool = serde_json::json!(true);
    let json_string = serde_json::json!("true");
    let yaml_bool = indoc! {"true\n"};
    let yaml_string = indoc! {"\"true\"\n"};

    let from_json_bool = serde_json::from_value::<Predicate>(json_bool)
        .expect("json bool predicate unit should deserialize");
    let from_json_string = serde_json::from_value::<Predicate>(json_string)
        .expect("json string predicate unit should deserialize");
    let from_yaml_bool =
        serde_yaml::from_str::<Predicate>(yaml_bool).expect("yaml bool should deserialize");
    let from_yaml_string =
        serde_yaml::from_str::<Predicate>(yaml_string).expect("yaml string should deserialize");

    assert_eq!(from_json_bool, Predicate::True);
    assert_eq!(from_json_string, Predicate::True);
    assert_eq!(from_yaml_bool, Predicate::True);
    assert_eq!(from_yaml_string, Predicate::True);

    let normalized = serde_json::to_value(Predicate::True).expect("predicate should serialize");
    assert_eq!(normalized, serde_json::json!(true));
}

#[cfg(feature = "full")]
#[test]
fn predicate_and_policy_roundtrip_through_protobuf() {
    let predicate = Predicate::And(
        Box::new(Predicate::PropEq {
            property: "verb".to_string(),
            value: PredicateValue::Text("invoke".to_string()),
        }),
        Box::new(Predicate::Not(Box::new(Predicate::PropExists(
            "status-code".to_string(),
        )))),
    );

    let policy = RetryPolicy::Union(
        Box::new(RetryPolicy::CountBox {
            max_retries: 3,
            inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(10))),
        }),
        Box::new(RetryPolicy::FilteredOn {
            predicate: predicate.clone(),
            inner: Box::new(RetryPolicy::Immediate),
        }),
    );

    let predicate_proto: golem_api_grpc::proto::golem::worker::retry::Predicate =
        predicate.clone().into();
    let predicate_roundtrip = Predicate::try_from(predicate_proto)
        .expect("predicate should roundtrip through protobuf conversion");
    assert_eq!(predicate_roundtrip, predicate);

    let policy_proto: golem_api_grpc::proto::golem::worker::retry::RetryPolicy =
        policy.clone().into();
    let policy_roundtrip = RetryPolicy::try_from(policy_proto)
        .expect("policy should roundtrip through protobuf conversion");
    assert_eq!(policy_roundtrip, policy);
}

#[cfg(feature = "full")]
#[test]
fn named_retry_policy_roundtrip_through_protobuf() {
    let named_policy = NamedRetryPolicy {
        name: "rpc-transient".to_string(),
        priority: 42,
        predicate: Predicate::PropEq {
            property: "error-type".to_string(),
            value: PredicateValue::Text("transient".to_string()),
        },
        policy: RetryPolicy::CountBox {
            max_retries: 5,
            inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(200))),
        },
    };

    let proto: golem_api_grpc::proto::golem::worker::retry::NamedRetryPolicy =
        named_policy.clone().into();
    let roundtrip = NamedRetryPolicy::try_from(proto)
        .expect("named retry policy should roundtrip through protobuf conversion");
    assert_eq!(roundtrip, named_policy);
}
