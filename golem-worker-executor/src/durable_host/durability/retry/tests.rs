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

use super::*;
use crate::durable_host::durability::{find_semantic_trap_retry_override, is_write_side_effect};
use golem_common::model::oplog::HostPayloadPair;
use golem_common::model::oplog::host_functions::KeyvalueEventualGet;
use golem_common::model::{NamedRetryPolicy, Predicate, PredicateValue, RetryPolicy};
use golem_service_base::error::worker_executor::{GolemSpecificWasmTrap, WorkerExecutorError};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use test_r::test;

/// A mock DurabilityHost for testing `try_trigger_retry_or_loop` in isolation.
struct MockDurabilityHost {
    max_in_function_retry_delay: Duration,
    named_retry_policies: Vec<NamedRetryPolicy>,
    oplog_retry_count: u32,
    /// Tracks how many times `append_retry_error_entry` was called.
    retry_entries_appended: u32,
    /// Tracks how many times `try_trigger_retry` was called (fallback path).
    trap_triggered_count: u32,
    /// If set, `create_interrupt_signal` resolves immediately with this kind.
    interrupt_signal: Option<InterruptKind>,
    /// Controls whether the interrupt fires before the sleep completes.
    /// When true, the interrupt resolves immediately.
    interrupt_armed: Arc<AtomicBool>,
    /// Tracks the latest retry policy state written via `append_retry_error_entry`.
    current_retry_policy_state: Option<RetryPolicyState>,
    /// When `Some`, `check_read_only_allows` fails with a `WorkerReadOnlyViolation`
    /// using the contained method name. Mirrors the real
    /// `DurableWorkerCtx::check_read_only_allows` behaviour.
    read_only_method: Option<String>,
}

impl MockDurabilityHost {
    fn new() -> Self {
        Self {
            max_in_function_retry_delay: Duration::from_secs(20),
            named_retry_policies: Vec::new(),
            oplog_retry_count: 0,
            retry_entries_appended: 0,
            trap_triggered_count: 0,
            interrupt_signal: None,
            interrupt_armed: Arc::new(AtomicBool::new(false)),
            current_retry_policy_state: None,
            read_only_method: None,
        }
    }

    fn durable_execution_state(&self) -> DurableExecutionState {
        DurableExecutionState {
            is_live: true,
            snapshotting_mode: false,
            max_in_function_retry_delay: self.max_in_function_retry_delay,
        }
    }
}

#[async_trait]
impl InFunctionRetryHost for MockDurabilityHost {
    fn current_retry_point(&self) -> OplogIndex {
        OplogIndex::INITIAL
    }

    async fn named_retry_policies(&mut self) -> Vec<NamedRetryPolicy> {
        self.named_retry_policies.clone()
    }

    async fn current_retry_state_for(&self, _retry_from: OplogIndex) -> Option<RetryPolicyState> {
        if let Some(state) = &self.current_retry_policy_state {
            Some(state.clone())
        } else if self.oplog_retry_count > 0 {
            Some(RetryPolicyState::CountBox {
                attempts: self.oplog_retry_count,
                inner: Box::new(RetryPolicyState::Counter(self.oplog_retry_count)),
            })
        } else {
            None
        }
    }

    fn durable_execution_state(&self) -> DurableExecutionState {
        MockDurabilityHost::durable_execution_state(self)
    }

    async fn append_retry_error_entry(
        &mut self,
        _retry_from: OplogIndex,
        retry_policy_state: Option<RetryPolicyState>,
    ) -> Result<(), OplogError> {
        self.retry_entries_appended += 1;
        self.current_retry_policy_state = retry_policy_state;
        Ok(())
    }
}

#[async_trait]
impl DurabilityHost for MockDurabilityHost {
    fn observe_function_call(&self, _interface: &str, _function: &str) {}

    async fn begin_durable_function(
        &mut self,
        function_type: &DurableFunctionType,
        host_function: &str,
    ) -> Result<(), WorkerExecutorError> {
        // Mirror the production read-only side-effect trap so tests can exercise the
        // single central enforcement point.
        if is_write_side_effect(function_type)
            && let Err(GolemSpecificWasmTrap::WorkerReadOnlyViolation {
                method,
                host_function,
            }) = self.check_read_only_allows(host_function)
        {
            return Err(WorkerExecutorError::ReadOnlyViolation {
                method,
                host_function,
            });
        }
        Ok(())
    }

    async fn end_durable_function(
        &mut self,
        _function_type: &DurableFunctionType,
        _forced_commit: bool,
    ) -> Result<(), WorkerExecutorError> {
        Ok(())
    }

    async fn try_trigger_retry(
        &mut self,
        _failure: Error,
        _properties: RetryProperties,
    ) -> anyhow::Result<()> {
        self.trap_triggered_count += 1;
        // Simulate: retries exhausted, persist the failure
        Ok(())
    }

    fn check_read_only_allows(&self, host_function: &str) -> Result<(), GolemSpecificWasmTrap> {
        if let Some(method) = &self.read_only_method {
            Err(GolemSpecificWasmTrap::WorkerReadOnlyViolation {
                method: method.clone(),
                host_function: host_function.to_string(),
            })
        } else {
            Ok(())
        }
    }

    fn create_interrupt_signal(&self) -> Pin<Box<dyn Future<Output = InterruptKind> + Send>> {
        if let Some(kind) = self.interrupt_signal {
            // Resolve immediately
            Box::pin(async move { kind })
        } else {
            let armed = self.interrupt_armed.clone();
            Box::pin(async move {
                loop {
                    if armed.load(Ordering::Acquire) {
                        return InterruptKind::Interrupt(Timestamp::now_utc());
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
        }
    }
}

/// Helper: creates an `InFunctionRetryController` for the given function type, using the
/// fully-qualified host-function name of an arbitrary host-call pair as the retry label.
async fn make_retry_controller(
    ctx: &mut MockDurabilityHost,
    function_type: DurableFunctionType,
) -> InFunctionRetryController {
    InFunctionRetryController::new(
        function_type,
        ctx.durable_execution_state(),
        <KeyvalueEventualGet as HostPayloadPair>::FQFN,
    )
}

/// Helper: opens a durable function the same way the host-call entry path does — observe the
/// call, then run the central read-only side-effect check in `begin_durable_function`.
async fn begin_durable_call(
    ctx: &mut MockDurabilityHost,
    function_type: DurableFunctionType,
) -> Result<(), WorkerExecutorError> {
    DurabilityHost::observe_function_call(
        ctx,
        <KeyvalueEventualGet as HostPayloadPair>::INTERFACE,
        <KeyvalueEventualGet as HostPayloadPair>::FUNCTION,
    );
    ctx.begin_durable_function(
        &function_type,
        <KeyvalueEventualGet as HostPayloadPair>::FQFN,
    )
    .await
}

// Test 1: In-function retry works for eligible operations
#[test]
async fn in_function_retry_returns_retry_internally_on_transient_error() {
    let mut ctx = MockDurabilityHost::new();
    ctx.named_retry_policies = vec![NamedRetryPolicy {
        name: "default".to_string(),
        priority: 0,
        predicate: Predicate::True,
        policy: RetryPolicy::CountBox {
            max_retries: 5,
            inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(1))),
        },
    }];
    let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;

    let result: Result<String, String> = Err("connection refused".to_string());
    let action = controller
        .try_trigger_retry_or_loop_with_properties(
            &mut ctx,
            &result,
            |_| HostFailureKind::Transient,
            RetryProperties::new(),
        )
        .await
        .expect("should not propagate error");

    assert_eq!(action, InternalRetryResult::RetryInternally);
    assert_eq!(ctx.retry_entries_appended, 1);
    assert_eq!(controller.retry_state.retry_count(), 1);
    assert_eq!(ctx.trap_triggered_count, 0, "should NOT fall back to trap");
}

// Test 1b: Success returns Persist immediately
#[test]
async fn in_function_retry_returns_persist_on_success() {
    let mut ctx = MockDurabilityHost::new();
    let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;

    let result: Result<String, String> = Ok("value".to_string());
    let action = controller
        .try_trigger_retry_or_loop_with_properties(
            &mut ctx,
            &result,
            |_| HostFailureKind::Transient,
            RetryProperties::new(),
        )
        .await
        .expect("should not propagate error");

    assert_eq!(action, InternalRetryResult::Persist);
    assert_eq!(ctx.retry_entries_appended, 0);
    assert_eq!(controller.retry_state.retry_count(), 0);
}

// Test 1c: Permanent errors return Persist (no retry)
#[test]
async fn in_function_retry_returns_persist_on_permanent_error() {
    let mut ctx = MockDurabilityHost::new();
    let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;

    let result: Result<String, String> = Err("invalid key format".to_string());
    let action = controller
        .try_trigger_retry_or_loop_with_properties(
            &mut ctx,
            &result,
            |_| HostFailureKind::Permanent,
            RetryProperties::new(),
        )
        .await
        .expect("should not propagate error");

    assert_eq!(action, InternalRetryResult::Persist);
    assert_eq!(ctx.retry_entries_appended, 0);
    assert_eq!(ctx.trap_triggered_count, 0);
}

// Test 1d: Multiple retries exhaust budget, then returns Persist
#[test]
async fn in_function_retry_exhausts_budget_then_persists() {
    let mut ctx = MockDurabilityHost::new();
    ctx.named_retry_policies = vec![NamedRetryPolicy {
        name: "default".to_string(),
        priority: 0,
        predicate: Predicate::True,
        policy: RetryPolicy::CountBox {
            max_retries: 3,
            inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(1))),
        },
    }];
    let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;

    let result: Result<String, String> = Err("timeout".to_string());

    // Attempts 0, 1, 2 should return RetryInternally
    for i in 0..3 {
        let action = controller
            .try_trigger_retry_or_loop_with_properties(
                &mut ctx,
                &result,
                |_| HostFailureKind::Transient,
                RetryProperties::new(),
            )
            .await
            .expect("should not propagate error");
        assert_eq!(
            action,
            InternalRetryResult::RetryInternally,
            "attempt {i} should retry"
        );
    }

    // Attempt 3 should return Persist (budget exhausted)
    let action = controller
        .try_trigger_retry_or_loop_with_properties(
            &mut ctx,
            &result,
            |_| HostFailureKind::Transient,
            RetryProperties::new(),
        )
        .await
        .expect("should not propagate error");
    assert_eq!(action, InternalRetryResult::Persist);
    assert_eq!(ctx.retry_entries_appended, 3);
    assert_eq!(controller.retry_state.retry_count(), 3);
}

#[test]
async fn named_retry_policy_match_overrides_legacy_retry_budget() {
    let mut ctx = MockDurabilityHost::new();
    ctx.named_retry_policies = vec![NamedRetryPolicy {
        name: "rpc-transient".to_string(),
        priority: 100,
        predicate: Predicate::PropEq {
            property: "verb".to_string(),
            value: PredicateValue::Text("invoke".to_string()),
        },
        policy: RetryPolicy::CountBox {
            max_retries: 1,
            inner: Box::new(RetryPolicy::Immediate),
        },
    }];

    let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;
    let mut props = RetryProperties::new();
    props.set("verb", PredicateValue::Text("invoke".to_string()));

    let result: Result<String, String> = Err("timeout".to_string());
    let action = controller
        .try_trigger_retry_or_loop_with_properties(
            &mut ctx,
            &result,
            |_| HostFailureKind::Transient,
            props,
        )
        .await
        .expect("should not propagate error");

    assert_eq!(action, InternalRetryResult::RetryInternally);
    assert_eq!(ctx.retry_entries_appended, 1);
}

#[test]
async fn named_retry_policy_no_match_returns_exhausted() {
    let mut ctx = MockDurabilityHost::new();
    ctx.named_retry_policies = vec![NamedRetryPolicy {
        name: "rpc-transient".to_string(),
        priority: 100,
        predicate: Predicate::PropEq {
            property: "verb".to_string(),
            value: PredicateValue::Text("invoke".to_string()),
        },
        policy: RetryPolicy::Immediate,
    }];

    let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;
    let mut props = RetryProperties::new();
    props.set("verb", PredicateValue::Text("query".to_string()));

    let result: Result<String, String> = Err("timeout".to_string());
    let action = controller
        .try_trigger_retry_or_loop_with_properties(
            &mut ctx,
            &result,
            |_| HostFailureKind::Transient,
            props,
        )
        .await
        .expect("should not propagate error");

    assert_eq!(action, InternalRetryResult::Persist);
    assert_eq!(ctx.retry_entries_appended, 0);
}

#[test]
async fn semantic_policy_resolution_error_returns_exhausted() {
    let mut ctx = MockDurabilityHost::new();
    ctx.named_retry_policies = vec![NamedRetryPolicy {
        name: "invalid-for-props".to_string(),
        priority: 100,
        predicate: Predicate::PropEq {
            property: "attempt".to_string(),
            value: PredicateValue::Integer(1),
        },
        policy: RetryPolicy::Never,
    }];

    let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;
    let mut props = RetryProperties::new();
    // This type mismatch forces a coercion error during predicate evaluation.
    props.set("attempt", PredicateValue::Boolean(true));

    let result: Result<String, String> = Err("timeout".to_string());
    let action = controller
        .try_trigger_retry_or_loop_with_properties(
            &mut ctx,
            &result,
            |_| HostFailureKind::Transient,
            props,
        )
        .await
        .expect("should not propagate error");

    assert_eq!(action, InternalRetryResult::Persist);
    assert_eq!(ctx.retry_entries_appended, 0);
}

#[test]
async fn elapsed_time_policy_returns_exhausted() {
    let mut ctx = MockDurabilityHost::new();
    ctx.named_retry_policies = vec![NamedRetryPolicy {
        name: "time-boxed".to_string(),
        priority: 100,
        predicate: Predicate::PropEq {
            property: "verb".to_string(),
            value: PredicateValue::Text("invoke".to_string()),
        },
        policy: RetryPolicy::TimeBox {
            limit: Duration::ZERO,
            inner: Box::new(RetryPolicy::Immediate),
        },
    }];

    let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;
    let mut props = RetryProperties::new();
    props.set("verb", PredicateValue::Text("invoke".to_string()));

    let result: Result<String, String> = Err("timeout".to_string());
    let action = controller
        .try_trigger_retry_or_loop_with_properties(
            &mut ctx,
            &result,
            |_| HostFailureKind::Transient,
            props,
        )
        .await
        .expect("should not propagate error");

    assert_eq!(action, InternalRetryResult::Persist);
    assert_eq!(ctx.retry_entries_appended, 0);
}

// WriteRemote is always eligible for in-function retry.
#[test]
async fn in_function_retry_eligible_for_write_remote() {
    let mut ctx = MockDurabilityHost::new();
    ctx.named_retry_policies = vec![NamedRetryPolicy {
        name: "default".to_string(),
        priority: 0,
        predicate: Predicate::True,
        policy: RetryPolicy::CountBox {
            max_retries: 5,
            inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(1))),
        },
    }];
    let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::WriteRemote).await;

    let result: Result<String, String> = Err("timeout".to_string());
    let action = controller
        .try_trigger_retry_or_loop_with_properties(
            &mut ctx,
            &result,
            |_| HostFailureKind::Transient,
            RetryProperties::new(),
        )
        .await
        .expect("should not propagate error");

    assert_eq!(action, InternalRetryResult::RetryInternally);
    assert_eq!(ctx.trap_triggered_count, 0);
}

// Test 2: Fallback to trap when delay exceeds max_in_function_retry_delay
#[test]
async fn fallback_to_trap_when_delay_exceeds_threshold() {
    let mut ctx = MockDurabilityHost::new();
    ctx.named_retry_policies = vec![NamedRetryPolicy {
        name: "default".to_string(),
        priority: 0,
        predicate: Predicate::True,
        policy: RetryPolicy::Periodic(Duration::from_secs(30)),
    }];
    ctx.max_in_function_retry_delay = Duration::from_secs(20);
    let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;

    let result: Result<String, String> = Err("timeout".to_string());
    let action = controller
        .try_trigger_retry_or_loop_with_properties(
            &mut ctx,
            &result,
            |_| HostFailureKind::Transient,
            RetryProperties::new(),
        )
        .await
        .expect("should not propagate error (mock try_trigger_retry returns Ok)");

    // Should have fallen back to trap+replay path
    assert_eq!(action, InternalRetryResult::Persist);
    assert_eq!(
        ctx.trap_triggered_count, 1,
        "should fall back to trap+replay"
    );
    assert_eq!(
        ctx.retry_entries_appended, 0,
        "should NOT append in-function retry entry"
    );
}

#[test]
async fn time_box_fallback_carries_the_evaluated_state_to_trap_recovery() {
    let mut ctx = MockDurabilityHost::new();
    ctx.named_retry_policies = vec![NamedRetryPolicy {
        name: "time-box".to_string(),
        priority: 0,
        predicate: Predicate::True,
        policy: RetryPolicy::TimeBox {
            limit: Duration::from_secs(60),
            inner: Box::new(RetryPolicy::Periodic(Duration::from_secs(30))),
        },
    }];
    ctx.max_in_function_retry_delay = Duration::from_secs(20);
    let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;

    let result: Result<String, String> = Err("timeout".to_string());
    let error = controller
        .try_trigger_retry_or_loop_with_properties(
            &mut ctx,
            &result,
            |_| HostFailureKind::Transient,
            RetryProperties::new(),
        )
        .await
        .expect_err("TimeBox fallback must carry the first decision into trap recovery");
    let semantic_override = find_semantic_trap_retry_override(&error)
        .expect("fallback must carry a semantic trap retry override");

    assert_eq!(
        semantic_override.verdict,
        SemanticTrapRetryVerdict::Retry(Duration::from_secs(30))
    );
    assert!(matches!(
        semantic_override.retry_policy_state,
        RetryPolicyState::TimeBox {
            elapsed_millis: 0,
            inner,
            ..
        } if *inner == RetryPolicyState::Wrapper(Box::new(RetryPolicyState::Counter(1)))
    ));
    assert_eq!(
        ctx.trap_triggered_count, 0,
        "policy must not be stepped twice"
    );
    assert_eq!(ctx.retry_entries_appended, 0);
}

#[test]
fn child_repair_requires_owner_authorization_for_every_function_type() {
    for function_type in [
        DurableFunctionType::ReadLocal,
        DurableFunctionType::WriteLocal,
        DurableFunctionType::ReadRemote,
        DurableFunctionType::WriteRemote,
    ] {
        let ctx = MockDurabilityHost::new();
        let controller = InFunctionRetryController::new(
            function_type,
            ctx.durable_execution_state(),
            KeyvalueEventualGet::FQFN,
        );
        assert!(controller.can_reexecute_on_incomplete_replay());
        let controller = controller.with_parent_start_index(Some(OplogIndex::from_u64(17)));
        assert!(!controller.can_reexecute_on_incomplete_replay());
        assert!(
            controller
                .with_incomplete_repair_authorized_by_scope(true)
                .can_reexecute_on_incomplete_replay()
        );
    }
}

// Test 4: Interrupt during in-function retry sleep
#[test]
async fn interrupt_during_in_function_retry_sleep_propagates_error() {
    let mut ctx = MockDurabilityHost::new();
    ctx.named_retry_policies = vec![NamedRetryPolicy {
        name: "default".to_string(),
        priority: 0,
        predicate: Predicate::True,
        policy: RetryPolicy::Periodic(Duration::from_secs(10)),
    }];
    // But still within the threshold so we don't fall back to trap
    ctx.max_in_function_retry_delay = Duration::from_secs(20);
    // Interrupt fires immediately
    ctx.interrupt_signal = Some(InterruptKind::Interrupt(Timestamp::now_utc()));

    let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;

    let result: Result<String, String> = Err("timeout".to_string());
    let err = controller
        .try_trigger_retry_or_loop_with_properties(
            &mut ctx,
            &result,
            |_| HostFailureKind::Transient,
            RetryProperties::new(),
        )
        .await
        .expect_err("should propagate interrupt as error");

    // The error should be an InterruptKind
    assert!(
        err.downcast_ref::<InterruptKind>().is_some(),
        "error should be InterruptKind, got: {err}"
    );
    // The retry entry should still have been appended before the sleep
    assert_eq!(ctx.retry_entries_appended, 1);
}

// Test 4b: Interrupt via armed flag during sleep
#[test]
async fn interrupt_armed_during_sleep_propagates() {
    let mut ctx = MockDurabilityHost::new();
    ctx.named_retry_policies = vec![NamedRetryPolicy {
        name: "default".to_string(),
        priority: 0,
        predicate: Predicate::True,
        policy: RetryPolicy::Periodic(Duration::from_millis(500)),
    }];
    ctx.max_in_function_retry_delay = Duration::from_secs(20);
    // Arm the interrupt to fire via the polling loop
    ctx.interrupt_armed.store(true, Ordering::Release);

    let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;

    let result: Result<String, String> = Err("timeout".to_string());
    let err = controller
        .try_trigger_retry_or_loop_with_properties(
            &mut ctx,
            &result,
            |_| HostFailureKind::Transient,
            RetryProperties::new(),
        )
        .await
        .expect_err("should propagate interrupt as error");

    assert!(err.downcast_ref::<InterruptKind>().is_some());
}

// Test: oplog retry count is combined with in-function retry count
#[test]
async fn oplog_retry_count_combined_with_in_function_retry_count() {
    let mut ctx = MockDurabilityHost::new();
    ctx.named_retry_policies = vec![NamedRetryPolicy {
        name: "default".to_string(),
        priority: 0,
        predicate: Predicate::True,
        policy: RetryPolicy::CountBox {
            max_retries: 5,
            inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(1))),
        },
    }];
    // Simulate 3 prior oplog-level retries
    ctx.oplog_retry_count = 3;
    let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;

    let result: Result<String, String> = Err("timeout".to_string());

    // With 3 oplog retries, total=3. max_retries=5.
    // Attempt at total=3 → ok (within budget)
    let action = controller
        .try_trigger_retry_or_loop_with_properties(
            &mut ctx,
            &result,
            |_| HostFailureKind::Transient,
            RetryProperties::new(),
        )
        .await
        .expect("should not propagate error");
    assert_eq!(action, InternalRetryResult::RetryInternally);

    // Attempt at total=4 → ok (within budget)
    let action = controller
        .try_trigger_retry_or_loop_with_properties(
            &mut ctx,
            &result,
            |_| HostFailureKind::Transient,
            RetryProperties::new(),
        )
        .await
        .expect("should not propagate error");
    assert_eq!(action, InternalRetryResult::RetryInternally);

    // Attempt at total=5 → exhausted (5 >= max_retries)
    let action = controller
        .try_trigger_retry_or_loop_with_properties(
            &mut ctx,
            &result,
            |_| HostFailureKind::Transient,
            RetryProperties::new(),
        )
        .await
        .expect("should not propagate error");
    assert_eq!(action, InternalRetryResult::Persist);
}

// Test: decide_retry_for_named_policy uses the supplied policy directly,
// bypassing the resolver. Even if `named_retry_policies` would have selected a
// different (e.g. synthesized default) policy for the same properties,
// `decide_retry_for_named_policy` honors the explicit `named_policy`.
#[test]
async fn decide_retry_for_named_policy_uses_supplied_policy() {
    let mut ctx = MockDurabilityHost::new();
    // Resolver-visible policies include only a "would-be-default" that exhausts
    // immediately. If the helper re-resolved, no retry would happen.
    ctx.named_retry_policies = vec![NamedRetryPolicy {
        name: "default".to_string(),
        priority: 0,
        predicate: Predicate::True,
        policy: RetryPolicy::CountBox {
            max_retries: 0,
            inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(1))),
        },
    }];

    // The explicitly supplied policy permits one retry.
    let explicit = NamedRetryPolicy {
        name: "explicit".to_string(),
        priority: 0,
        predicate: Predicate::True,
        policy: RetryPolicy::CountBox {
            max_retries: 1,
            inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(1))),
        },
    };

    let mut state = InFunctionRetryState::new();
    let decision = state
        .decide_retry_for_named_policy(&mut ctx, "test-fn", &RetryProperties::new(), &explicit)
        .await;

    match decision {
        AsyncRetryDecision::RetryAfterDelay(_) => {}
        other => panic!("expected RetryAfterDelay, got {other:?}"),
    }
    assert_eq!(ctx.retry_entries_appended, 1);
    assert_eq!(state.retry_count(), 1);
}

// Test: decide_retry_for_named_policy returns Exhausted when the supplied
// policy has no remaining attempts.
#[test]
async fn decide_retry_for_named_policy_exhausted_when_supplied_policy_gives_up() {
    let mut ctx = MockDurabilityHost::new();

    let explicit = NamedRetryPolicy {
        name: "explicit".to_string(),
        priority: 0,
        predicate: Predicate::True,
        policy: RetryPolicy::CountBox {
            max_retries: 0,
            inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(1))),
        },
    };

    let mut state = InFunctionRetryState::new();
    let decision = state
        .decide_retry_for_named_policy(&mut ctx, "test-fn", &RetryProperties::new(), &explicit)
        .await;

    assert!(matches!(decision, AsyncRetryDecision::Exhausted));
    assert_eq!(ctx.retry_entries_appended, 0);
}

// Regression reproducer: status-code retry decisions are keyed on the HTTP
// request's begin index, which can already contain retry state written by a
// different policy shape (for example a transport-error/default policy). The
// explicit status-code policy must reset on that shape mismatch instead of
// treating `InvalidState` as exhausted.
#[test]
async fn decide_retry_for_named_policy_resets_stale_state_shape() {
    let mut ctx = MockDurabilityHost::new();
    ctx.current_retry_policy_state = Some(RetryPolicyState::CountBox {
        attempts: 1,
        inner: Box::new(RetryPolicyState::Wrapper(Box::new(
            RetryPolicyState::Counter(1),
        ))),
    });

    let status_policy = NamedRetryPolicy {
        name: "http-5xx".to_string(),
        priority: 100,
        predicate: Predicate::True,
        policy: RetryPolicy::CountBox {
            max_retries: 5,
            inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(1))),
        },
    };

    let mut state = InFunctionRetryState::new();
    let decision = state
        .decide_retry_for_named_policy(
            &mut ctx,
            "http-status-retry",
            &RetryProperties::new(),
            &status_policy,
        )
        .await;

    assert!(
        matches!(decision, AsyncRetryDecision::RetryAfterDelay(_)),
        "stale retry state from a different policy shape must not exhaust the status policy; got {decision:?}"
    );
    assert_eq!(ctx.retry_entries_appended, 1);
}

// Test: is_eligible_for_internal_retry covers all DurableFunctionType variants
#[test]
fn eligibility_matrix() {
    let cases = vec![
        (DurableFunctionType::ReadRemote, true),
        (DurableFunctionType::ReadLocal, true),
        (DurableFunctionType::WriteLocal, true),
        (DurableFunctionType::WriteRemote, true),
    ];

    for (ft, expected) in cases {
        let ctx = MockDurabilityHost::new();
        let controller = InFunctionRetryController::new(
            ft.clone(),
            ctx.durable_execution_state(),
            KeyvalueEventualGet::FQFN,
        );
        assert_eq!(
            controller.is_eligible_for_internal_retry(),
            expected,
            "ft={ft:?}"
        );
        assert_eq!(
            controller.can_reexecute_on_incomplete_replay(),
            expected,
            "ft={ft:?}"
        );
    }
}

/// Bug B reproducer: when the in-function HTTP status retry exhausts
/// (so the guest finally sees the 500 and traps), the leftover
/// `CountBox{Counter}` retry state at the request's begin index must
/// not poison the trap-retry path's evaluation of a *different*
/// policy (the catch-all `default` policy synthesised from
/// `RetryConfig`, which has shape `CountBox{Jitter{Clamp{Exp}}}`).
///
/// Without the `evaluate_named_policy_step_resetting_on_invalid_state`
/// guard, evaluation produces `RetryVerdict::Error(InvalidState ..)`
/// and the trap path silently falls back to the legacy retry config
/// instead of using the user-configured (or default) named policy.
#[test]
fn evaluate_named_policy_step_resets_on_state_shape_mismatch() {
    use golem_common::model::RetryConfig;

    let default_policy = NamedRetryPolicy::default_from_config(&RetryConfig::max_attempts_5());

    // Simulate the leftover state from an exhausted inline status
    // retry: its inner state is a bare `Counter`, while the default
    // policy's inner is a `Wrapper` chain.
    let stale_state = RetryPolicyState::CountBox {
        attempts: 2,
        inner: Box::new(RetryPolicyState::Counter(2)),
    };
    let properties = RetryProperties::new();

    // Without the guard: evaluation surfaces InvalidState as a
    // verdict (which the trap-retry call site treats as a fall-back
    // signal to the legacy retry config).
    let baseline = evaluate_named_policy_step(&default_policy, &properties, Some(&stale_state))
        .expect("evaluate must not error at the Result layer");
    assert!(
        matches!(
            baseline.1,
            RetryVerdict::Error(RetryEvaluationError::InvalidState { .. })
        ),
        "baseline must produce InvalidState verdict, got {:?}",
        baseline.1
    );

    // With the guard: evaluation transparently re-evaluates from an
    // empty state, producing a usable Retry verdict.
    let guarded = evaluate_named_policy_step_resetting_on_invalid_state(
        &default_policy,
        &properties,
        Some(&stale_state),
    )
    .expect("evaluate must not error at the Result layer");
    assert!(
        matches!(guarded.1, RetryVerdict::Retry(_)),
        "guarded evaluation must produce a Retry verdict, got {:?}",
        guarded.1
    );

    // The guard MUST NOT change behaviour when the state shape
    // matches: the existing accumulator must be preserved (otherwise
    // it would be a regression in normal retry accounting).
    let valid_state = default_policy.policy.initial_state();
    let direct = evaluate_named_policy_step(&default_policy, &properties, Some(&valid_state))
        .expect("evaluate must not error");
    let via_guard = evaluate_named_policy_step_resetting_on_invalid_state(
        &default_policy,
        &properties,
        Some(&valid_state),
    )
    .expect("evaluate must not error");
    assert!(matches!(direct.1, RetryVerdict::Retry(_)));
    assert!(matches!(via_guard.1, RetryVerdict::Retry(_)));
    assert_eq!(
        format!("{:?}", direct.0),
        format!("{:?}", via_guard.0),
        "guard must not perturb state when shape matches"
    );
}

#[test]
fn host_managed_time_box_uses_inclusive_monotonic_elapsed_time() {
    let policy = NamedRetryPolicy {
        name: "nested-time-box".to_string(),
        priority: 1,
        predicate: Predicate::True,
        policy: RetryPolicy::CountBox {
            max_retries: 10,
            inner: Box::new(RetryPolicy::AddDelay {
                delay: Duration::from_millis(1),
                inner: Box::new(RetryPolicy::TimeBox {
                    limit: Duration::from_millis(500),
                    inner: Box::new(RetryPolicy::Immediate),
                }),
            }),
        },
    };
    let properties = RetryProperties::new();

    let (state_1, verdict_1) =
        evaluate_named_policy_step_at(&policy, &properties, None, 1_000).unwrap();
    assert_eq!(verdict_1, RetryVerdict::Retry(Duration::from_millis(1)));

    let (state_2, verdict_2) =
        evaluate_named_policy_step_at(&policy, &properties, Some(&state_1), 1_499).unwrap();
    assert_eq!(verdict_2, RetryVerdict::Retry(Duration::from_millis(1)));
    assert_eq!(state_2.retry_count(), 2);

    let (state_after_clock_regression, verdict_after_clock_regression) =
        evaluate_named_policy_step_at(&policy, &properties, Some(&state_2), 1_200).unwrap();
    assert_eq!(
        verdict_after_clock_regression,
        RetryVerdict::Retry(Duration::from_millis(1))
    );
    assert!(matches!(
        state_after_clock_regression,
        RetryPolicyState::TimeBox {
            started_at_millis: 1_000,
            elapsed_millis: 499,
            ..
        }
    ));

    let (_, verdict_at_limit) = evaluate_named_policy_step_at(
        &policy,
        &properties,
        Some(&state_after_clock_regression),
        1_500,
    )
    .unwrap();
    assert_eq!(verdict_at_limit, RetryVerdict::GiveUp);
}

#[test]
fn host_managed_policy_without_time_box_keeps_existing_state_shape() {
    let policy = NamedRetryPolicy {
        name: "count-only".to_string(),
        priority: 1,
        predicate: Predicate::True,
        policy: RetryPolicy::CountBox {
            max_retries: 2,
            inner: Box::new(RetryPolicy::Immediate),
        },
    };

    let (state, verdict) =
        evaluate_named_policy_step_at(&policy, &RetryProperties::new(), None, 10_000).unwrap();

    assert_eq!(verdict, RetryVerdict::Retry(Duration::ZERO));
    assert_eq!(
        state,
        RetryPolicyState::CountBox {
            attempts: 1,
            inner: Box::new(RetryPolicyState::Counter(1)),
        }
    );
}
// ---------------------------------------------------------------------
// Generic read-only side-effect trap (begin_durable_function)
// ---------------------------------------------------------------------

/// Read function types must always succeed, regardless of whether the
/// mock currently models a read-only agent method. Only `Write*`
/// variants must be gated by `check_read_only_allows`.
#[test]
async fn read_only_check_does_not_trip_for_read_function_types() {
    for ft in [
        DurableFunctionType::ReadLocal,
        DurableFunctionType::ReadRemote,
    ] {
        let mut ctx = MockDurabilityHost::new();
        ctx.read_only_method = Some("read-only-method".to_string());
        let result = begin_durable_call(&mut ctx, ft.clone()).await;
        assert!(
            result.is_ok(),
            "read-only check must not trip for {ft:?}, got {:?}",
            result.err()
        );
    }
}

/// When the worker is NOT in read-only mode, every `Write*` function
/// type must open the durable function successfully — the new generic
/// check must be a strict precondition added on top of the existing
/// behaviour, not a new constraint that always fires.
#[test]
async fn write_function_types_succeed_when_not_in_read_only_mode() {
    for ft in [
        DurableFunctionType::WriteLocal,
        DurableFunctionType::WriteRemote,
    ] {
        let mut ctx = MockDurabilityHost::new();
        // read_only_method left as None.
        let result = begin_durable_call(&mut ctx, ft.clone()).await;
        assert!(
            result.is_ok(),
            "non-read-only worker must accept {ft:?}, got {:?}",
            result.err()
        );
    }
}

/// In read-only mode, every `Write*` function type must be refused via
/// `WorkerExecutorError::ReadOnlyViolation`, carrying:
///   - the agent method name supplied by the host context, and
///   - the fully qualified host-function name (`Pair::FQFN`) of the
///     attempted host call.
/// This is the core property the generic check is supposed to enforce
/// uniformly across every side-effecting host function path.
#[test]
async fn write_function_types_trap_with_read_only_violation_in_read_only_mode() {
    for ft in [
        DurableFunctionType::WriteLocal,
        DurableFunctionType::WriteRemote,
    ] {
        let mut ctx = MockDurabilityHost::new();
        ctx.read_only_method = Some("agent::read-only-method".to_string());

        let result = begin_durable_call(&mut ctx, ft.clone()).await;
        let err = match result {
            Ok(_) => panic!("read-only worker must refuse {ft:?} via begin_durable_function"),
            Err(e) => e,
        };

        match err {
            WorkerExecutorError::ReadOnlyViolation {
                method,
                host_function,
            } => {
                assert_eq!(method, "agent::read-only-method");
                assert_eq!(
                    host_function,
                    <KeyvalueEventualGet as HostPayloadPair>::FQFN,
                    "host_function must be Pair::FQFN (got {host_function})"
                );
            }
            other => {
                panic!("expected WorkerExecutorError::ReadOnlyViolation for {ft:?}, got {other:?}")
            }
        }
    }
}

fn named_policy(name: &str, priority: u32, policy: RetryPolicy) -> NamedRetryPolicy {
    NamedRetryPolicy {
        name: name.to_string(),
        priority,
        predicate: Predicate::True,
        policy,
    }
}

/// `merge_named_retry_policy_tiers` must apply name-based overrides in tier order
/// (default < agent-config < environment < runtime mutations), honor runtime removals,
/// and sort the result by descending priority then by name.
#[test]
async fn merge_named_retry_policy_tiers_precedence_removal_and_sorting() {
    let default_policy = named_policy("default", 0, RetryPolicy::Never);

    let agent_config_policies = vec![
        // Overridden by the environment tier below.
        named_policy("shared", 10, RetryPolicy::Never),
        named_policy("agent-only", 5, RetryPolicy::Immediate),
        // Removed by a runtime mutation below.
        named_policy("removed", 99, RetryPolicy::Immediate),
    ];

    let environment_policies = vec![
        named_policy("shared", 20, RetryPolicy::Immediate),
        // Same priority as "agent-only" to exercise the name tie-break.
        named_policy("env-only", 5, RetryPolicy::Immediate),
    ];

    let mut mutations: BTreeMap<String, Option<NamedRetryPolicy>> = BTreeMap::new();
    mutations.insert(
        "runtime-added".to_string(),
        Some(named_policy("runtime-added", 30, RetryPolicy::Immediate)),
    );
    mutations.insert(
        "shared".to_string(),
        Some(named_policy("shared", 1, RetryPolicy::Never)),
    );
    mutations.insert("removed".to_string(), None);

    let merged = merge_named_retry_policy_tiers(
        default_policy,
        agent_config_policies,
        environment_policies,
        &mutations,
    );

    let names_and_priorities: Vec<(&str, u32)> = merged
        .iter()
        .map(|p| (p.name.as_str(), p.priority))
        .collect();
    assert_eq!(
        names_and_priorities,
        vec![
            ("runtime-added", 30),
            ("agent-only", 5),
            ("env-only", 5),
            ("shared", 1),
            ("default", 0),
        ]
    );
    // The runtime mutation (highest tier) determines the surviving "shared" policy.
    let shared = merged.iter().find(|p| p.name == "shared").unwrap();
    assert_eq!(shared.policy, RetryPolicy::Never);
}
