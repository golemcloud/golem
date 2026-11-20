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
use golem_common::model::{AgentInvocation, InvocationId};
use golem_common::schema::SchemaValue;
use golem_worker_executor::services::events::Event;
use golem_worker_executor::worker::ResultOrSubscription;
use golem_worker_executor_test_utils::take_agent_oplog_over_at_epoch;
use test_r::{test, timeout};

inherit_test_dep!(Tracing);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);

fn commit_barrier_overrides() -> TestExecutorOverrides {
    TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            // These tests hold the explicit acceptance/completion commit, not a threshold flush.
            config.oplog.max_operations_before_commit = u64::MAX;
            config.agent_status_flush.enabled = false;
        })),
        ..Default::default()
    }
}

fn increment(idempotency_key: InvocationId) -> AgentInvocation {
    AgentInvocation::AgentMethod {
        idempotency_key,
        method_name: "increment".into(),
        input: data_value!().value().clone(),
        invocation_context: InvocationContextStack::fresh(),
        caller: callers::anonymous(),
        scope_card: None,
    }
}

#[test]
#[timeout("2m")]
async fn same_key_callers_are_not_accepted_before_a_pending_commit_that_is_fenced(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    // Phase 1: establish a live Worker with counter value 1.
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, commit_barrier_overrides()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let name = agent_key!("Counter", "fenced-pending-receipt");
    let id = executor
        .start_agent(&component.id, name.clone())
        .await?
        .agent_id;
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &name, "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        1
    );
    let owned = OwnedAgentId::new(context.default_environment_id, &id);
    let worker = executor.active_agent(&owned).await.unwrap().primary();

    // Phase 2: pause admission after Pending is buffered, before its commit reaches storage.
    let key = InvocationId(uuid::Uuid::new_v4());
    let invocation = increment(key);
    let mut pending_commit_gate = executor
        .gate_before_next_oplog_commit(&id, |entry| {
            matches!(entry, OplogEntry::PendingAgentInvocation { .. })
        })
        .await;
    let mut accepting = Box::pin(worker.clone().invoke(invocation.clone()));
    tokio::select! {
        _ = pending_commit_gate.entered() => {}
        _ = &mut accepting => panic!("acceptance escaped the uncommitted Pending entry"),
    }
    assert!(
        matches!(pending_commit_gate.entry(), OplogEntry::PendingAgentInvocation { invocation_id, .. } if invocation_id == key)
    );

    // Phase 3: race two same-key callers against the held acceptance commit.
    // Poll the production Worker entry points ourselves. Each same-key caller has entered
    // ingress while the original commit is held; none may acknowledge acceptance or success.
    let mut same_key_accepting = Box::pin(worker.clone().invoke(invocation.clone()));
    let mut another_same_key_accepting = Box::pin(worker.clone().invoke(invocation));
    assert!(poll!(&mut accepting).is_pending());
    assert!(poll!(&mut same_key_accepting).is_pending());
    assert!(poll!(&mut another_same_key_accepting).is_pending());
    let before = executor.stored_oplog(&id).await;
    assert!(!before.iter().any(|entry| matches!(entry,
        OplogEntry::PendingAgentInvocation { invocation_id, .. } if *invocation_id == key)));

    // Phase 4: arm an unexpected-start alarm, fence the oplog, then release the commit.
    // This append gate must never fire: a fenced Started write would not appear in storage.
    // Registering it does not release the commit, which already owns its original gate.
    let mut unexpected_start_alarm = executor.gate_next_invocation_started_append(&id).await;
    take_agent_oplog_over_at_epoch(deps, &context, &owned, 1).await?;
    pending_commit_gate.release();

    // Phase 5: every caller is rejected, the Worker retires, and no execution was attempted.
    let (first, follower, another_follower) =
        tokio::time::timeout(Duration::from_secs(20), async {
            tokio::join!(accepting, same_key_accepting, another_same_key_accepting)
        })
        .await?;
    for error in [first.err(), follower.err(), another_follower.err()] {
        let error = error.expect("a refused Pending commit must reject every same-key caller");
        assert!(
            matches!(
                error,
                WorkerExecutorError::OplogFenced { .. } | WorkerExecutorError::ShardingNotReady
            ),
            "expected a reroutable shard-loss error, got {error:?}"
        );
    }
    tokio::time::timeout(Duration::from_secs(20), async {
        while executor.worker_is_cached(&owned).await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    assert!(
        poll!(Box::pin(unexpected_start_alarm.entered())).is_pending(),
        "the refused invocation attempted to start"
    );
    let after = executor.stored_oplog(&id).await;
    assert!(
        !after.iter().any(|entry| match entry {
            OplogEntry::PendingAgentInvocation { invocation_id, .. } => *invocation_id == key,
            OplogEntry::AgentInvocationStarted { invocation_id, .. }
            | OplogEntry::AgentInvocationFinished { invocation_id, .. } => *invocation_id == key,
            _ => false,
        }),
        "the refused invocation must neither be accepted nor execute"
    );
    // No method (including a duplicate under a different key) ran after the baseline increment.
    assert_eq!(
        after
            .iter()
            .filter(|entry| matches!(entry, OplogEntry::AgentInvocationStarted { .. }))
            .count(),
        before
            .iter()
            .filter(|entry| matches!(entry, OplogEntry::AgentInvocationStarted { .. }))
            .count()
    );
    Ok(())
}

#[test]
#[timeout("2m")]
async fn finished_commit_receipt_gates_success_and_same_key_result_survives_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    // Phase 1: establish a live Worker with counter value 1.
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, commit_barrier_overrides()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let name = agent_key!("Counter", "finished-receipt");
    let id = executor
        .start_agent(&component.id, name.clone())
        .await?
        .agent_id;
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &name, "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        1
    );
    let owned = OwnedAgentId::new(context.default_environment_id, &id);
    let worker = executor.active_agent(&owned).await.unwrap().primary();

    // Phase 2: let increment run, but pause before its Finished commit reaches storage.
    let key = InvocationId(uuid::Uuid::new_v4());
    let invocation = increment(key);
    let mut finished_commit_gate = executor
        .gate_before_next_oplog_commit(&id, |entry| {
            matches!(entry, OplogEntry::AgentInvocationFinished { .. })
        })
        .await;
    let mut original = Box::pin(worker.clone().invoke_and_await(invocation.clone()));
    tokio::select! {
        _ = finished_commit_gate.entered() => {}
        result = &mut original => panic!("success escaped the uncommitted Finished entry: {result:?}"),
    }
    assert!(
        matches!(finished_commit_gate.entry(), OplogEntry::AgentInvocationFinished { invocation_id, .. } if invocation_id == key)
    );

    // Phase 3: neither the original caller, a same-key caller, nor lookup may see success.
    // A same-key invoke may return a subscription immediately; drive that subscription too,
    // so only delivery of the result (not acceptance of the existing invocation) is forbidden.
    let mut follower = Box::pin(async {
        match worker.clone().invoke(invocation.clone()).await? {
            ResultOrSubscription::Finished(result) => Ok::<_, anyhow::Error>(result?),
            ResultOrSubscription::Pending(mut subscription) => Ok(subscription
                .wait_for(|event| match event {
                    Event::InvocationCompleted {
                        agent_id,
                        idempotency_key,
                        result,
                    } if *agent_id == id && *idempotency_key == key => Some(*result.clone()),
                    _ => None,
                })
                .await??),
        }
    });
    // The gate establishes the uncommitted window. Drive all readers during that window,
    // rather than checking a single poll that may stop at an unrelated actor round trip.
    let observation_window = Duration::from_millis(200);
    let (original_while_held, follower_while_held, lookup_while_held) = tokio::join!(
        tokio::time::timeout(observation_window, &mut original),
        tokio::time::timeout(observation_window, &mut follower),
        tokio::time::timeout(observation_window, worker.lookup_invocation_result(&key)),
    );
    assert!(
        original_while_held.is_err(),
        "the original caller completed before commit"
    );
    assert!(
        follower_while_held.is_err(),
        "the same-key caller received a result before commit"
    );
    assert!(
        matches!(lookup_while_held, Err(_) | Ok(LookupResult::Pending)),
        "lookup must block or report Pending before commit, got {lookup_while_held:?}"
    );
    let held = executor.stored_oplog(&id).await;
    assert_eq!(held.iter().filter(|entry| matches!(entry, OplogEntry::AgentInvocationStarted { invocation_id, .. } if *invocation_id == key)).count(), 1);
    assert!(!held.iter().any(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { invocation_id, .. } if *invocation_id == key)));

    // Phase 4: release the Finished commit; only now may all three readers receive value 2.
    finished_commit_gate.release();
    let (original, follower) = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(original, follower)
    })
    .await?;
    let original = original?;
    assert_eq!(
        original.result,
        AgentInvocationResult::AgentMethod {
            output: SchemaValue::U32(2)
        }
    );
    assert_eq!(follower?, original);
    let lookup = worker.lookup_invocation_result(&key).await;
    assert!(matches!(lookup, LookupResult::Complete(Ok(ref result)) if *result == original));

    // Phase 5: discard the resident Worker and executor after completion is delivered.
    drop(worker);
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);

    // Phase 6: reconstruct over the same storage and recover the same-key result.
    // This is a new executor and Worker, not a resident resume or cursor reset.
    let executor = start_with_overrides(deps, &context, commit_barrier_overrides()).await?;
    executor.resume(&owned, false).await?;
    let worker = executor.active_agent(&owned).await.unwrap().primary();
    Worker::start_if_needed(worker.clone()).await?;
    let recovered = worker.clone().invoke_and_await(invocation).await?;
    assert_eq!(recovered, original);

    // Phase 7: a fresh increment returns 3, proving no same-key attempt repeated the effect.
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &name, "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        3,
        "same-key callers and completed replay must not increment the counter again"
    );
    let history = executor.stored_oplog(&id).await;
    assert_eq!(history.iter().filter(|entry| matches!(entry, OplogEntry::AgentInvocationStarted { invocation_id, .. } if *invocation_id == key)).count(), 1);
    assert_eq!(history.iter().filter(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { invocation_id, .. } if *invocation_id == key)).count(), 1);
    Ok(())
}

#[test]
#[timeout("2m")]
async fn commit_gate_observes_either_entry_from_both_pair_append_apis(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::oplog::LogLevel;
    use golem_worker_executor::services::oplog::CommitLevel;

    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, commit_barrier_overrides()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let name = agent_key!("Counter", "pair-commit-gate");
    let id = executor
        .start_agent(&component.id, name.clone())
        .await?
        .agent_id;
    executor
        .invoke_and_await_agent(&component, &name, "increment", data_value!())
        .await?;
    let owned = OwnedAgentId::new(context.default_environment_id, &id);
    let worker = executor.active_agent(&owned).await.unwrap().primary();
    let oplog = worker.oplog();

    let matchers: [fn(&OplogEntry) -> bool; 2] = [
        |entry| matches!(entry, OplogEntry::Log { message, .. } if message == "first"),
        |entry| matches!(entry, OplogEntry::Log { message, .. } if message == "second"),
    ];
    for enqueue in [false, true] {
        for matches in matchers {
            // Buffer an atomic pair, targeting each position through each append API.
            let before = executor.stored_oplog(&id).await;
            let mut gate = executor.gate_before_next_oplog_commit(&id, matches).await;
            let log = |message: &str| OplogEntry::Log {
                timestamp: Timestamp::now_utc(),
                parent_start_index: None,
                level: LogLevel::Info,
                context: "pair-commit-gate".into(),
                message: message.into(),
                trace_context: None,
            };
            let first = log("first");
            let second = log("second");
            let make_second = Box::new(move |_| second);
            if enqueue {
                oplog.enqueue_add_pair(first, make_second).await?;
            } else {
                oplog.add_pair(first, make_second).await?;
            }

            // The explicit commit must stop before either entry reaches storage.
            let mut commit = Box::pin(oplog.commit(CommitLevel::Always));
            tokio::select! {
                _ = gate.entered() => {}
                result = &mut commit => panic!("pair commit bypassed its gate: {result:?}"),
            }
            assert!(matches(&gate.entry()));
            assert_eq!(executor.stored_oplog(&id).await.len(), before.len());

            // Releasing the gate commits both entries together.
            gate.release();
            commit.await?;
            let after = executor.stored_oplog(&id).await;
            assert_eq!(after.len(), before.len() + 2);
            assert!(
                matches!(&after[before.len()], OplogEntry::Log { message, .. } if message == "first")
            );
            assert!(
                matches!(&after[before.len() + 1], OplogEntry::Log { message, .. } if message == "second")
            );
        }
    }
    Ok(())
}
