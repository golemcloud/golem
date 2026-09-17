use super::*;
use crate::retry::builder::{Policy, Predicate, Props};
use std::cell::{Cell, RefCell};
use std::future::ready;
use std::rc::Rc;
use test_r::test;

fn schedule(policy: Policy) -> RetrySchedule {
    policy.try_to_schedule().unwrap()
}

fn step_delays(
    schedule: &RetrySchedule,
    count: usize,
    properties: &[(String, retry_api::PredicateValue)],
) -> Vec<Step> {
    let mut policy = schedule.policy.clone();
    let mut random = || 0.0;
    (0..count)
        .map(|_| policy.step(Duration::ZERO, properties, &mut random))
        .collect()
}

#[test]
fn every_delay_strategy_and_transform_is_interpreted() {
    assert_eq!(
        step_delays(
            &schedule(Policy::exponential(Duration::from_millis(3), 2.0)),
            3,
            &[]
        ),
        vec![
            Step::Retry(Duration::from_millis(3)),
            Step::Retry(Duration::from_millis(6)),
            Step::Retry(Duration::from_millis(12)),
        ]
    );
    assert_eq!(
        step_delays(
            &schedule(Policy::fibonacci(
                Duration::from_millis(2),
                Duration::from_millis(5)
            )),
            4,
            &[]
        ),
        vec![
            Step::Retry(Duration::from_millis(2)),
            Step::Retry(Duration::from_millis(5)),
            Step::Retry(Duration::from_millis(7)),
            Step::Retry(Duration::from_millis(12)),
        ]
    );

    let transformed = Policy::periodic(Duration::from_millis(80))
        .clamp(Duration::from_millis(20), Duration::from_millis(50))
        .add_delay(Duration::from_millis(7));
    assert_eq!(
        step_delays(&schedule(transformed), 1, &[]),
        vec![Step::Retry(Duration::from_millis(57))]
    );
    assert_eq!(
        step_delays(&schedule(Policy::never()), 1, &[]),
        vec![Step::GiveUp]
    );
}

#[test]
fn sequence_union_and_intersection_keep_independent_asymmetric_state() {
    let left = Policy::periodic(Duration::from_millis(10)).max_retries(1);
    let right = Policy::exponential(Duration::from_millis(20), 2.0);

    assert_eq!(
        step_delays(&schedule(left.clone().and_then(right.clone())), 3, &[]),
        vec![
            Step::Retry(Duration::from_millis(10)),
            Step::Retry(Duration::from_millis(20)),
            Step::Retry(Duration::from_millis(40)),
        ]
    );
    assert_eq!(
        step_delays(&schedule(left.clone().union(right.clone())), 3, &[]),
        vec![
            Step::Retry(Duration::from_millis(10)),
            Step::Retry(Duration::from_millis(40)),
            Step::Retry(Duration::from_millis(80)),
        ]
    );
    assert_eq!(
        step_delays(&schedule(left.intersect(right)), 2, &[]),
        vec![Step::Retry(Duration::from_millis(20)), Step::GiveUp]
    );
}

#[test]
fn time_box_uses_elapsed_boundary_without_rejecting_crossing_delay() {
    let schedule =
        schedule(Policy::periodic(Duration::from_secs(10)).within(Duration::from_secs(5)));
    let mut policy = schedule.policy.clone();
    let mut random = || 0.0;

    assert_eq!(
        policy.step(Duration::from_nanos(4_999_999_999), &[], &mut random),
        Step::Retry(Duration::from_secs(10))
    );
    assert_eq!(
        policy.step(Duration::from_secs(5), &[], &mut random),
        Step::GiveUp
    );
}

#[test]
fn deterministic_jitter_is_positive_and_uses_base_delay() {
    let schedule = schedule(
        Policy::periodic(Duration::from_millis(100))
            .with_jitter(0.4)
            .max_retries(1),
    );
    let mut policy = schedule.policy.clone();
    let mut random = || 0.25;

    assert_eq!(
        policy.step(Duration::ZERO, &[], &mut random),
        Step::Retry(Duration::from_millis(110))
    );
}

#[test]
fn all_predicate_nodes_and_coercions_are_supported() {
    let properties = vec![
        (
            "number".to_string(),
            retry_api::PredicateValue::Text("42".into()),
        ),
        (
            "name".to_string(),
            retry_api::PredicateValue::Text("api-503".into()),
        ),
        (
            "enabled".to_string(),
            retry_api::PredicateValue::Boolean(true),
        ),
    ];
    let predicates = [
        Predicate::eq("number", 42),
        Predicate::neq("number", 41),
        Predicate::gt("number", 40),
        Predicate::gte("number", 42),
        Predicate::lt("number", 50),
        Predicate::lte("number", 42),
        Predicate::exists("enabled"),
        Predicate::one_of("number", [1, 42]),
        Predicate::matches_glob("name", "{web,api}-[0-9][0-9][0-9]"),
        Predicate::starts_with("name", "api-"),
        Predicate::contains("name", "503"),
        Predicate::and(Predicate::always(), Predicate::not(Predicate::never())),
        Predicate::or(Predicate::never(), Predicate::always()),
    ];

    for predicate in predicates {
        let schedule = schedule(Policy::immediate().only_when(predicate));
        assert_eq!(
            step_delays(&schedule, 1, &properties),
            vec![Step::Retry(Duration::ZERO)]
        );
    }
}

#[test]
fn glob_matching_has_canonical_boundaries_and_malformed_behavior() {
    assert!(glob_match::glob_match("**/foo", "a/b/foo"));
    assert!(!glob_match::glob_match("**/foo", "xfoo"));
    assert!(!glob_match::glob_match("**/", "a"));
    assert!(!glob_match::glob_match("![", "x"));
    assert!(glob_match::glob_match(r"api-\*", "api-*"));
    assert!(glob_match::glob_match("フ*/**/*", "フォルダ/aaa.js"));
    assert!(!glob_match::glob_match("**/x", &"a".repeat(100_000)));
}

#[test]
fn predicate_errors_propagate_without_activating_composed_fallbacks() {
    let filtered = Policy::immediate().only_when(Predicate::eq("status", 503));
    let policies = [
        filtered.clone().and_then(Policy::immediate()),
        filtered.clone().union(Policy::immediate()),
        Policy::immediate().union(filtered),
    ];

    for policy in policies {
        assert_eq!(step_delays(&schedule(policy), 1, &[]), vec![Step::Error]);
    }
}

#[test]
async fn retry_projects_each_failure_and_eventually_succeeds() {
    let schedule = schedule(
        Policy::immediate()
            .only_when(Predicate::gte(Props::STATUS_CODE, 500_u16))
            .max_retries(4),
    );
    let attempts = Rc::new(Cell::new(0));
    let projected = Rc::new(RefCell::new(Vec::new()));
    let sleeps = Rc::new(Cell::new(0));

    let result = schedule
        .retry_with_runtime(
            {
                let attempts = attempts.clone();
                move || {
                    let attempt = attempts.get() + 1;
                    attempts.set(attempt);
                    ready(if attempt == 3 {
                        Ok("done")
                    } else {
                        Err(500 + attempt)
                    })
                }
            },
            {
                let projected = projected.clone();
                move |error: &u32| {
                    projected.borrow_mut().push(*error);
                    vec![(
                        Props::STATUS_CODE.to_string(),
                        retry_api::PredicateValue::Integer(i64::from(*error)),
                    )]
                }
            },
            || 0,
            || 0.0,
            {
                let sleeps = sleeps.clone();
                move |_| {
                    sleeps.set(sleeps.get() + 1);
                    ready(())
                }
            },
        )
        .await;

    assert_eq!(result, Ok("done"));
    assert_eq!(attempts.get(), 3);
    assert_eq!(&*projected.borrow(), &[501, 502]);
    assert_eq!(sleeps.get(), 2);
}

#[test]
async fn retry_count_and_final_error_are_preserved() {
    #[derive(Debug, PartialEq)]
    struct UserError(&'static str, u32);

    let schedule = schedule(Policy::immediate().max_retries(2));
    let attempts = Rc::new(Cell::new(0));
    let result: Result<(), UserError> = schedule
        .retry_with_runtime(
            {
                let attempts = attempts.clone();
                move || {
                    let attempt = attempts.get() + 1;
                    attempts.set(attempt);
                    ready(Err(UserError("original type", attempt)))
                }
            },
            |_| Vec::new(),
            || 0,
            || 0.0,
            |_| ready(()),
        )
        .await;

    assert_eq!(attempts.get(), 3);
    assert_eq!(result, Err(UserError("original type", 3)));
}

#[test]
async fn retry_stops_when_dynamic_filtered_predicate_changes() {
    let schedule = schedule(
        Policy::immediate()
            .only_when(Predicate::eq(Props::STATUS_CODE, 503_u16))
            .max_retries(5),
    );
    let attempts = Rc::new(Cell::new(0));
    let result: Result<(), u16> = schedule
        .retry_with_runtime(
            {
                let attempts = attempts.clone();
                move || {
                    let attempt = attempts.get() + 1;
                    attempts.set(attempt);
                    ready(Err(if attempt == 1 { 503 } else { 404 }))
                }
            },
            |status| {
                [(
                    Props::STATUS_CODE.to_string(),
                    retry_api::PredicateValue::Integer(i64::from(*status)),
                )]
            },
            || 0,
            || 0.0,
            |_| ready(()),
        )
        .await;

    assert_eq!(attempts.get(), 2);
    assert_eq!(result, Err(404));
}

#[test]
async fn elapsed_time_is_measured_from_before_the_first_attempt() {
    let schedule = schedule(Policy::immediate().within(Duration::from_nanos(10)));
    let times = Rc::new(RefCell::new(vec![0_u64, 9, 10].into_iter()));
    let attempts = Rc::new(Cell::new(0));
    let result: Result<(), u32> = schedule
        .retry_with_runtime(
            {
                let attempts = attempts.clone();
                move || {
                    let attempt = attempts.get() + 1;
                    attempts.set(attempt);
                    ready(Err(attempt))
                }
            },
            |_| Vec::new(),
            {
                let times = times.clone();
                move || times.borrow_mut().next().unwrap()
            },
            || 0.0,
            |_| ready(()),
        )
        .await;

    assert_eq!(attempts.get(), 2);
    assert_eq!(result, Err(2));
}

#[test]
async fn operation_can_borrow_mutable_state_across_await() {
    let schedule = schedule(Policy::immediate().max_retries(1));
    let mut attempts = 0;
    let result = schedule
        .retry_with_runtime(
            async || {
                attempts += 1;
                ready(()).await;
                if attempts == 2 {
                    Ok(attempts)
                } else {
                    Err(attempts)
                }
            },
            |_| Vec::new(),
            || 0,
            || 0.0,
            |_| ready(()),
        )
        .await;

    assert_eq!(result, Ok(2));
}

#[test]
async fn zero_retry_budget_still_runs_the_initial_attempt() {
    let schedule = schedule(Policy::immediate().max_retries(0));
    let attempts = Rc::new(Cell::new(0));
    let result: Result<(), &str> = schedule
        .retry_with_runtime(
            {
                let attempts = attempts.clone();
                async move || {
                    attempts.set(attempts.get() + 1);
                    Err("first failure")
                }
            },
            |_| Vec::new(),
            || 0,
            || 0.0,
            |_| ready(()),
        )
        .await;

    assert_eq!(attempts.get(), 1);
    assert_eq!(result, Err("first failure"));
}

#[test]
fn malformed_and_cyclic_raw_asts_are_rejected() {
    assert_eq!(
        RetrySchedule::try_from(&retry_api::RetryPolicy { nodes: vec![] }).unwrap_err(),
        RetryPolicyError::EmptyPolicy
    );
    let invalid = retry_api::RetryPolicy {
        nodes: vec![retry_api::PolicyNode::CountBox(retry_api::CountBoxConfig {
            max_retries: 1,
            inner: -1,
        })],
    };
    assert_eq!(
        RetrySchedule::try_from(&invalid).unwrap_err(),
        RetryPolicyError::InvalidPolicyNodeIndex(-1)
    );
    let cyclic = retry_api::RetryPolicy {
        nodes: vec![retry_api::PolicyNode::AddDelay(retry_api::AddDelayConfig {
            delay: 1,
            inner: 0,
        })],
    };
    assert_eq!(
        RetrySchedule::try_from(&cyclic).unwrap_err(),
        RetryPolicyError::CyclicPolicyNode(0)
    );

    let cyclic_predicate = retry_api::RetryPolicy {
        nodes: vec![
            retry_api::PolicyNode::FilteredOn(retry_api::FilteredConfig {
                predicate: retry_api::RetryPredicate {
                    nodes: vec![retry_api::PredicateNode::PredNot(0)],
                },
                inner: 1,
            }),
            retry_api::PolicyNode::Immediate,
        ],
    };
    assert_eq!(
        RetrySchedule::try_from(&cyclic_predicate).unwrap_err(),
        RetryPolicyError::CyclicPredicateNode(0)
    );
}

#[test]
fn malformed_raw_values_are_rejected_before_execution() {
    let invalid_clamp = retry_api::RetryPolicy {
        nodes: vec![
            retry_api::PolicyNode::ClampDelay(retry_api::ClampConfig {
                min_delay: 2,
                max_delay: 1,
                inner: 1,
            }),
            retry_api::PolicyNode::Immediate,
        ],
    };
    assert!(matches!(
        RetrySchedule::try_from(&invalid_clamp),
        Err(RetryPolicyError::InvalidClampRange { .. })
    ));

    let invalid_jitter = retry_api::RetryPolicy {
        nodes: vec![
            retry_api::PolicyNode::Jitter(retry_api::JitterConfig {
                factor: f64::NAN,
                inner: 1,
            }),
            retry_api::PolicyNode::Immediate,
        ],
    };
    assert!(matches!(
        RetrySchedule::try_from(&invalid_jitter),
        Err(RetryPolicyError::InvalidJitterFactor(factor)) if factor.is_nan()
    ));
}

#[test]
fn raw_ast_depth_and_expansion_are_bounded_but_shared_state_is_independent() {
    let oversized = retry_api::RetryPolicy {
        nodes: vec![retry_api::PolicyNode::Never; MAX_COMPILED_NODES + 1],
    };
    assert_eq!(
        RetrySchedule::try_from(&oversized).unwrap_err(),
        RetryPolicyError::PolicyTooComplex
    );

    let oversized_set = retry_api::RetryPolicy {
        nodes: vec![
            retry_api::PolicyNode::FilteredOn(retry_api::FilteredConfig {
                predicate: retry_api::RetryPredicate {
                    nodes: vec![retry_api::PredicateNode::PropIn(
                        retry_api::PropertySetCheck {
                            property_name: "value".to_string(),
                            values: vec![
                                retry_api::PredicateValue::Integer(0);
                                MAX_COMPILED_PAYLOAD_BYTES
                                    / std::mem::size_of::<retry_api::PredicateValue>()
                                    + 1
                            ],
                        },
                    )],
                },
                inner: 1,
            }),
            retry_api::PolicyNode::Immediate,
        ],
    };
    assert_eq!(
        RetrySchedule::try_from(&oversized_set).unwrap_err(),
        RetryPolicyError::PolicyTooComplex
    );

    let mut deep_nodes = vec![retry_api::PolicyNode::Never; MAX_POLICY_DEPTH + 2];
    for (index, node) in deep_nodes.iter_mut().enumerate().take(MAX_POLICY_DEPTH + 1) {
        *node = retry_api::PolicyNode::AddDelay(retry_api::AddDelayConfig {
            delay: 0,
            inner: (index + 1) as i32,
        });
    }
    assert_eq!(
        RetrySchedule::try_from(&retry_api::RetryPolicy { nodes: deep_nodes }).unwrap_err(),
        RetryPolicyError::PolicyTooComplex
    );

    let mut expansive_nodes = Vec::new();
    for index in 0..13 {
        expansive_nodes.push(retry_api::PolicyNode::PolicyUnion((index + 1, index + 1)));
    }
    expansive_nodes.push(retry_api::PolicyNode::Immediate);
    assert_eq!(
        RetrySchedule::try_from(&retry_api::RetryPolicy {
            nodes: expansive_nodes
        })
        .unwrap_err(),
        RetryPolicyError::PolicyTooComplex
    );

    let shared = retry_api::RetryPolicy {
        nodes: vec![
            retry_api::PolicyNode::PolicyUnion((1, 1)),
            retry_api::PolicyNode::Exponential(retry_api::ExponentialConfig {
                base_delay: 20_000_000,
                factor: 2.0,
            }),
        ],
    };
    let shared = RetrySchedule::try_from(&shared).unwrap();
    assert_eq!(
        step_delays(&shared, 2, &[]),
        vec![
            Step::Retry(Duration::from_millis(20)),
            Step::Retry(Duration::from_millis(40))
        ]
    );
}
