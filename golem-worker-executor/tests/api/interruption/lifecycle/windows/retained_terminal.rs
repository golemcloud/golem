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
use anyhow::ensure;
use futures::FutureExt;
use golem_common::model::{
    ShardAssignment, ShardDeliveryOutcome, ShardEpoch, ShardId, ShardLeaseRevision, Timestamp,
};
use golem_worker_executor::services::HasEvents;
use golem_worker_executor::services::shard::ShardService;
use golem_worker_executor_test_utils::start_with_concurrent_agent_limit_and_overrides;
use pretty_assertions::assert_eq;
use std::collections::HashSet;
use std::sync::mpsc;
use test_r::test;

#[derive(Clone, Copy, PartialEq, Eq)]
enum NextStart {
    None,
    Waiting,
    BeforeOwner,
    RestartFollower,
}

macro_rules! retained_terminal_test {
    ($name:ident, $next:expr) => {
        #[test]
        #[timeout("60s")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            retained_terminal(last_unique_id, deps, host_api_tests, $next).await
        }
    };
}

retained_terminal_test!(
    retained_committed_suspend_settles_after_real_publication,
    NextStart::None
);
retained_terminal_test!(
    prior_committed_suspend_cannot_settle_waiting_start,
    NextStart::Waiting
);
retained_terminal_test!(
    prior_committed_suspend_cannot_settle_pre_owner_start,
    NextStart::BeforeOwner
);
retained_terminal_test!(
    published_restart_preserves_distinct_later_terminal,
    NextStart::RestartFollower
);

#[test]
#[timeout("60s")]
async fn pending_automatic_writer_blocks_lifecycle_selection_until_publication(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_concurrent_agent_limit_and_overrides(
        deps,
        &context,
        1,
        TestExecutorOverrides::default(),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let name = agent_id!("GolemHostApi", "retained-pending-writer");
    let id = executor.start_agent(&component.id, name.clone()).await?;
    let promise = executor
        .invoke_and_await_agent(&component, &name, "create_promise", data_value!())
        .await?
        .into_return_value()
        .context("create_promise returned no id")?;
    let owned = OwnedAgentId::new(context.default_environment_id, &id);
    let active = executor
        .production_active_agent(&owned)
        .await
        .context("owner not cached")?;
    let worker = active.primary();
    let generation = worker.resident_generation_for_test();
    let key = IdempotencyKey::fresh();
    let (outcome, release_outcome) = worker.pause_next_outcome_for_test(true);
    let (teardown, release_teardown) = worker.pause_next_teardown_fence_for_test();
    executor
        .invoke_agent_with_key(
            &component,
            &name,
            &key,
            "await_promise",
            crate::raw_params(vec![promise]),
        )
        .await?;
    tokio::time::timeout(Duration::from_secs(10), outcome).await??;
    let selected = worker
        .pending_terminal_for_test()
        .context("automatic terminal writer is not pending")?;
    assert_eq!(selected.0, generation);
    assert_eq!(selected.1, Some(key.clone()));
    let automatic = selected.2;
    ensure!(matches!(automatic, InterruptKind::Suspend(_)));
    assert_eq!(worker.committed_terminal_for_test(), None);
    assert_eq!(worker.pending_stop_for_test().await, None);
    assert!(worker.concurrent_agent_permit_is_held().await);
    let acquisitions = worker.permit_acquisitions_for_test();

    let mut held_permit = Box::pin(executor.acquire_account_concurrent_agent_permit(AgentId {
        component_id: component.id,
        agent_id: agent_id!("Counter", "pending-writer-permit-holder").to_string(),
    }));
    assert!((&mut held_permit).now_or_never().is_none());
    let (frozen, release_freeze) = worker.pause_next_stop_freeze_for_test();
    let (published, release_publication) = worker.pause_next_stop_publication_for_test();
    let mut published = Box::pin(published);
    let mut signal = worker.raw_interrupt_for_test();
    let mut receipt = worker
        .set_interrupting(automatic)
        .await?
        .context("accepted automatic cause has no receipt")?;
    tokio::time::timeout(Duration::from_secs(10), frozen).await??;
    assert_eq!(worker.frozen_stop_for_test(), Some(automatic));
    assert_eq!(worker.pending_stop_for_test().await, Some(automatic));
    assert!(!worker.terminal_stop_claimed_for_test().await);
    assert!(worker.terminal_interrupt_pending_for_test());

    let publication_wait = worker.observe_next_publication_wait_for_test();
    let mut selector = Box::pin(worker.claim_lifecycle_outcome_for_test(&key, automatic));
    release_freeze.send(false).unwrap();
    // Poll the real selector, not a task JoinHandle, to reach its Freezing wait.
    assert!((&mut selector).now_or_never().is_none());
    tokio::time::timeout(Duration::from_secs(10), publication_wait).await??;
    let mut joined = Box::pin(worker.join_accepted_stops_for_test());
    assert!((&mut joined).now_or_never().is_none());
    assert!((&mut published).now_or_never().is_none());
    assert!((&mut selector).now_or_never().is_none());
    assert_eq!(worker.pending_terminal_for_test(), Some(selected.clone()));
    assert_eq!(worker.committed_terminal_for_test(), None);
    assert_eq!(worker.pending_stop_for_test().await, Some(automatic));
    assert!(!worker.terminal_stop_claimed_for_test().await);
    assert!(worker.terminal_interrupt_pending_for_test());
    assert!(matches!(
        signal.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(worker.concurrent_agent_permit_is_held().await);
    assert!((&mut held_permit).now_or_never().is_none());
    let unwritten = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await;
    assert!(!unwritten.values().any(|entry| matches!(
        entry,
        OplogEntry::Suspend { .. } | OplogEntry::Interrupted { .. } | OplogEntry::Error { .. }
    )));
    assert!((&mut selector).now_or_never().is_none());
    assert!((&mut published).now_or_never().is_none());
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    let mut unloaded = Box::pin(worker.await_ready_to_process_commands());
    assert!((&mut unloaded).now_or_never().is_none());
    assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);

    release_outcome.send(false).unwrap();
    tokio::time::timeout(Duration::from_secs(10), &mut published).await??;
    assert_eq!(signal.recv().await?, automatic);
    assert_eq!(worker.owner_stop_for_test().await, Some(automatic));
    assert_eq!(worker.frozen_stop_for_test(), Some(automatic));
    assert_eq!(worker.pending_terminal_for_test(), None);
    assert_eq!(worker.committed_terminal_for_test(), Some(selected.clone()));
    assert!(worker.terminal_stop_claimed_for_test().await);
    assert_eq!(worker.pending_stop_for_test().await, None);
    assert!(!worker.terminal_interrupt_pending_for_test());
    assert!(
        tokio::time::timeout(Duration::from_secs(10), &mut selector)
            .await?
            .is_none(),
        "the same invocation already owns its only writer"
    );
    drop(selector);
    assert_eq!(worker.committed_terminal_for_test(), Some(selected));
    assert!(worker.concurrent_agent_permit_is_held().await);
    assert!((&mut held_permit).now_or_never().is_none());
    assert!((&mut joined).now_or_never().is_none());
    assert!((&mut unloaded).now_or_never().is_none());
    assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
    let snapshot = tokio::time::timeout(Duration::from_secs(10), teardown).await??;
    assert_eq!(snapshot.generation, generation);
    assert_eq!(snapshot.pending, None);
    assert_eq!(snapshot.frozen, Some(automatic));
    assert!(worker.concurrent_agent_permit_is_held().await);

    // A Store may acknowledge after typed publication, before physical cleanup.
    let mut acknowledgements = match receipt.try_recv() {
        Ok(()) => 1,
        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => 0,
        Err(error) => return Err(anyhow!("stop acknowledgement failed: {error}")),
    };
    eprintln!("[retained-pending-writer] acknowledgements at publication gate: {acknowledgements}");
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    release_teardown.send(()).unwrap();
    release_publication.send(false).unwrap();
    tokio::time::timeout(Duration::from_secs(10), &mut joined).await??;
    tokio::time::timeout(Duration::from_secs(10), &mut unloaded).await??;
    drop(unloaded);
    worker.retained_cleanup_for_test().await?;
    if acknowledgements == 0 {
        tokio::time::timeout(Duration::from_secs(10), receipt.recv()).await??;
        acknowledgements += 1;
    }
    assert_eq!(acknowledgements, 1);
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty
            | tokio::sync::broadcast::error::TryRecvError::Closed)
    ));
    let held_permit = tokio::time::timeout(Duration::from_secs(10), held_permit).await?;
    assert!(!worker.is_loaded().await);
    assert!(!worker.concurrent_agent_permit_is_held().await);
    assert!(worker.unload_succeeded_for_test());
    assert_eq!(worker.resident_generation_for_test(), generation);
    assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
    let entries = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await;
    assert_eq!(
        entries
            .values()
            .filter(|entry| matches!(entry, OplogEntry::Suspend { .. }))
            .count(),
        1
    );
    let starts = entries
        .iter()
        .filter_map(|(index, entry)| match entry {
            OplogEntry::AgentInvocationStarted {
                idempotency_key, ..
            } if idempotency_key == &key => Some(*index),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(starts.len(), 1);
    assert!(
        !entries.range(starts[0].next()..).any(|(_, entry)| matches!(
            entry,
            OplogEntry::AgentInvocationFinished { .. }
                | OplogEntry::Interrupted { .. }
                | OplogEntry::Error { .. }
        ))
    );
    eprintln!(
        "[retained-pending-writer] selector returned no writer; one Suspend and one acknowledgement; ordinary unload released the original permit"
    );
    drop(held_permit);
    Ok(())
}

async fn retained_terminal(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    next: NextStart,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let admission_pause = Arc::new(Mutex::new(None));
    let executor = start_with_concurrent_agent_limit_and_overrides(
        deps,
        &context,
        1,
        TestExecutorOverrides {
            wrap_shard_service: Some(Arc::new({
                let pause = admission_pause.clone();
                move |inner| {
                    Arc::new(StartupAdmissionGate {
                        inner,
                        pause: pause.clone(),
                    })
                }
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let name = agent_id!("GolemHostApi", "retained-terminal");
    let id = executor.start_agent(&component.id, name.clone()).await?;
    let promise = executor
        .invoke_and_await_agent(&component, &name, "create_promise", data_value!())
        .await?
        .into_return_value()
        .context("create_promise returned no id")?;
    let owned = OwnedAgentId::new(context.default_environment_id, &id);
    let active = executor
        .production_active_agent(&owned)
        .await
        .context("owner not cached")?;
    let worker = active.primary();
    let generation = worker.resident_generation_for_test();
    let key = IdempotencyKey::fresh();
    let (teardown, release_teardown) = worker.pause_next_teardown_fence_for_test();
    executor
        .invoke_agent_with_key(
            &component,
            &name,
            &key,
            "await_promise",
            crate::raw_params(vec![promise]),
        )
        .await?;
    let snapshot = tokio::time::timeout(Duration::from_secs(10), teardown).await??;
    assert_eq!(snapshot.generation, generation);
    assert_eq!(snapshot.pending, None);
    assert_eq!(snapshot.frozen, None);
    let selected = worker
        .committed_terminal_for_test()
        .context("automatic terminal writer not committed")?;
    assert_eq!(selected.0, generation);
    assert_eq!(selected.1, Some(key.clone()));
    let automatic = selected.2;
    ensure!(matches!(automatic, InterruptKind::Suspend(_)));
    assert!(worker.concurrent_agent_permit_is_held().await);
    let acquisitions = worker.permit_acquisitions_for_test();

    // Queue the real FIFO account waiter before the old resident releases its slot.
    let mut held_permit = Box::pin(executor.acquire_account_concurrent_agent_permit(AgentId {
        component_id: component.id,
        agent_id: agent_id!("Counter", "retained-terminal-permit-holder").to_string(),
    }));
    assert!((&mut held_permit).now_or_never().is_none());
    let (frozen, release_freeze) = worker.pause_next_stop_freeze_for_test();
    let (published, release_publication) = worker.pause_next_stop_publication_for_test();
    release_teardown.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), frozen).await??;
    let mut receipt = worker.accepted_stop_receipt_for_test();
    assert_eq!(worker.pending_stop_for_test().await, Some(automatic));
    assert!(!worker.terminal_stop_claimed_for_test().await);
    assert!(worker.terminal_interrupt_pending_for_test());
    assert_eq!(worker.committed_terminal_for_test(), Some(selected.clone()));
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!((&mut held_permit).now_or_never().is_none());

    // This command cannot be drained by the parked teardown. Ordinary unload resolves it.
    let mut unloaded = Box::pin(worker.await_ready_to_process_commands());
    assert!((&mut unloaded).now_or_never().is_none());
    assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
    release_freeze.send(false).unwrap();
    tokio::time::timeout(Duration::from_secs(10), published).await??;
    assert_eq!(worker.owner_stop_for_test().await, Some(automatic));
    assert_eq!(worker.frozen_stop_for_test(), Some(automatic));
    assert!(worker.terminal_stop_claimed_for_test().await);
    assert_eq!(worker.pending_stop_for_test().await, None);
    assert!(!worker.terminal_interrupt_pending_for_test());
    assert_eq!(worker.committed_terminal_for_test(), Some(selected.clone()));
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(worker.concurrent_agent_permit_is_held().await);
    assert!((&mut held_permit).now_or_never().is_none());
    assert!((&mut unloaded).now_or_never().is_none());
    assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
    release_publication.send(false).unwrap();
    worker.join_accepted_stops_for_test().await?;
    tokio::time::timeout(Duration::from_secs(10), &mut unloaded).await??;
    drop(unloaded);
    worker.retained_cleanup_for_test().await?;
    tokio::time::timeout(Duration::from_secs(10), receipt.recv()).await??;
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty
            | tokio::sync::broadcast::error::TryRecvError::Closed)
    ));
    let held_permit = tokio::time::timeout(Duration::from_secs(10), held_permit).await?;
    assert!(!worker.is_loaded().await);
    assert!(!worker.concurrent_agent_permit_is_held().await);
    assert!(worker.unload_succeeded_for_test());
    assert_eq!(worker.resident_generation_for_test(), generation);
    assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
    let entries = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await;
    assert_eq!(
        entries
            .values()
            .filter(|entry| matches!(entry, OplogEntry::Suspend { .. }))
            .count(),
        1
    );
    assert_eq!(entries.values().filter(|entry| matches!(entry, OplogEntry::AgentInvocationStarted { idempotency_key, .. } if idempotency_key == &key)).count(), 1);
    assert!(!entries.values().any(|entry| matches!(
        entry,
        OplogEntry::Interrupted { .. } | OplogEntry::Error { .. }
    )));

    if next == NextStart::None {
        drop(held_permit);
        return Ok(());
    }

    let mut events = worker.events().subscribe();
    let attempt = Worker::start_if_needed(worker.clone())
        .await?
        .context("new startup attempt missing")?;
    assert_eq!(worker.startup_attempt_for_test()?, Some(attempt));
    assert_eq!(worker.resident_generation_for_test(), generation);
    assert_eq!(worker.committed_terminal_for_test(), Some(selected.clone()));
    let first = if next == NextStart::RestartFollower {
        InterruptKind::Restart
    } else {
        automatic
    };
    let handoff = if next != NextStart::Waiting {
        let (observe, release) = pause_admission(&admission_pause, &id);
        drop(held_permit);
        tokio::time::timeout(Duration::from_secs(10), observe).await??;
        assert!(worker.is_loaded().await);
        assert!(worker.concurrent_agent_permit_is_held().await);
        assert_eq!(worker.startup_attempt_for_test()?, Some(attempt));
        assert_eq!(worker.resident_generation_for_test(), generation);
        Some(release)
    } else {
        assert!(!worker.is_loaded().await);
        assert!(!worker.concurrent_agent_permit_is_held().await);
        // Keep the account slot occupied through waiting-task finalization.
        None
    };
    let (frozen, release_freeze) = worker.pause_next_stop_freeze_for_test();
    let (published, release_publication) = worker.pause_next_stop_publication_for_test();
    let mut signal = worker.raw_interrupt_for_test();
    let mut receipt = worker
        .set_interrupting(first)
        .await?
        .context("new startup stop receipt missing")?;
    tokio::time::timeout(Duration::from_secs(10), frozen).await??;
    assert_eq!(worker.pending_stop_for_test().await, Some(first));
    assert!(!worker.terminal_stop_claimed_for_test().await);
    assert_eq!(worker.committed_terminal_for_test(), Some(selected.clone()));
    assert!(matches!(
        signal.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert_eq!(worker.startup_attempt_for_test()?, Some(attempt));
    release_freeze.send(false).unwrap();
    tokio::time::timeout(Duration::from_secs(10), published).await??;
    assert_eq!(signal.recv().await?, first);
    assert_eq!(worker.frozen_stop_for_test(), Some(first));
    assert_eq!(worker.pending_stop_for_test().await, Some(first));
    assert!(
        !worker.terminal_stop_claimed_for_test().await,
        "the previous resident writer must not consume this startup stop"
    );
    assert_eq!(
        worker.terminal_interrupt_pending_for_test(),
        next != NextStart::RestartFollower
    );
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert_eq!(worker.startup_attempt_for_test()?, Some(attempt));
    release_publication.send(false).unwrap();
    worker.join_accepted_stops_for_test().await?;

    if let Some(release_handoff) = handoff {
        // The native loop takes the published request before checking admission again.
        // An older Suspend preserves the later start; Restart acknowledges and retries.
        let (continued, release_continuation) = pause_admission(&admission_pause, &id);
        release_handoff
            .send(())
            .map_err(|_| anyhow!("startup admission gate was lost"))?;
        tokio::time::timeout(Duration::from_secs(10), continued).await??;
        assert_eq!(worker.pending_stop_for_test().await, None);
        assert!(!worker.terminal_stop_claimed_for_test().await);
        assert!(!worker.terminal_interrupt_pending_for_test());
        assert_eq!(worker.resident_generation_for_test(), generation);
        assert_eq!(worker.startup_attempt_for_test()?, Some(attempt));
        if next == NextStart::RestartFollower {
            tokio::time::timeout(Duration::from_secs(10), receipt.recv()).await??;
        } else {
            assert!(matches!(
                receipt.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ));
        }
        let later = InterruptKind::Interrupt(Timestamp::now_utc());
        let (_, entered, release_driver) = worker.pause_next_stop_driver_for_test();
        let later_receipt = worker.set_interrupting(later).await?;
        assert_eq!(later_receipt.is_some(), next == NextStart::BeforeOwner);
        tokio::time::timeout(Duration::from_secs(10), entered).await??;
        assert_eq!(worker.pending_stop_for_test().await, Some(later));
        release_driver.send(false).unwrap();
        worker.join_accepted_stops_for_test().await?;
        assert_eq!(worker.frozen_stop_for_test(), Some(first));
        assert_eq!(worker.pending_stop_for_test().await, Some(later));
        assert!(worker.terminal_interrupt_pending_for_test());
        assert!(!worker.terminal_stop_claimed_for_test().await);
        assert_eq!(worker.resident_generation_for_test(), generation);
        assert_eq!(worker.committed_terminal_for_test(), Some(selected.clone()));
        let mut stopped = Box::pin(worker.await_ready_to_process_commands());
        assert!((&mut stopped).now_or_never().is_none());
        release_continuation
            .send(())
            .map_err(|_| anyhow!("startup admission continuation was lost"))?;
        tokio::time::timeout(Duration::from_secs(10), &mut stopped).await??;
        if let Some(mut later_receipt) = later_receipt {
            tokio::time::timeout(Duration::from_secs(10), later_receipt.recv()).await??;
            assert!(matches!(
                later_receipt.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty
                    | tokio::sync::broadcast::error::TryRecvError::Closed)
            ));
        }
    }
    if next != NextStart::RestartFollower {
        tokio::time::timeout(Duration::from_secs(10), receipt.recv()).await??;
    }
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty
            | tokio::sync::broadcast::error::TryRecvError::Closed)
    ));
    let startup_result = tokio::time::timeout(
        Duration::from_secs(10),
        events.wait_for(|event| match event {
            Event::WorkerLoaded {
                agent_id,
                start_attempt,
                result,
            } if agent_id == &id && *start_attempt == attempt => Some(result.clone()),
            _ => None,
        }),
    )
    .await??;
    if next == NextStart::Waiting {
        assert!(
            matches!(startup_result, Err(WorkerExecutorError::Interrupted { kind }) if kind == automatic)
        );
        assert!(
            matches!(worker.startup_attempt_for_test(), Err(WorkerExecutorError::Interrupted { kind }) if kind == automatic)
        );
    } else {
        assert!(
            startup_result
                .unwrap_err()
                .to_string()
                .contains("Worker stopped before startup completed")
        );
        assert!(
            worker
                .startup_attempt_for_test()
                .unwrap_err()
                .to_string()
                .contains("Worker stopped before startup completed")
        );
    }
    worker.join_accepted_stops_for_test().await?;
    worker.retained_cleanup_for_test().await?;
    assert!(!worker.is_loaded().await);
    assert!(!worker.concurrent_agent_permit_is_held().await);
    assert_eq!(worker.pending_stop_for_test().await, None);
    assert_eq!(worker.resident_generation_for_test(), generation);
    assert_eq!(
        worker.permit_acquisitions_for_test(),
        acquisitions + usize::from(next != NextStart::Waiting)
    );
    assert_eq!(worker.committed_terminal_for_test(), Some(selected));
    let entries = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await;
    assert_eq!(
        entries
            .values()
            .filter(|entry| matches!(entry, OplogEntry::Suspend { .. }))
            .count(),
        if matches!(next, NextStart::Waiting | NextStart::BeforeOwner) {
            2
        } else {
            1
        }
    );
    assert_eq!(
        entries
            .values()
            .filter(|entry| matches!(entry, OplogEntry::Interrupted { .. }))
            .count(),
        usize::from(next != NextStart::Waiting)
    );
    assert!(
        !entries
            .values()
            .any(|entry| matches!(entry, OplogEntry::Error { .. }))
    );
    Ok(())
}

fn pause_admission(
    pause: &Mutex<Option<AdmissionPause>>,
    agent_id: &AgentId,
) -> (tokio::sync::oneshot::Receiver<()>, mpsc::Sender<()>) {
    let (entered, observe) = tokio::sync::oneshot::channel();
    let (release, wait) = mpsc::channel();
    assert!(
        pause
            .lock()
            .unwrap()
            .replace(AdmissionPause {
                agent_id: agent_id.clone(),
                entered,
                release: wait,
            })
            .is_none()
    );
    (observe, release)
}

struct AdmissionPause {
    agent_id: AgentId,
    entered: tokio::sync::oneshot::Sender<()>,
    release: mpsc::Receiver<()>,
}

struct StartupAdmissionGate {
    inner: Arc<dyn ShardService>,
    pause: Arc<Mutex<Option<AdmissionPause>>>,
}

impl ShardService for StartupAdmissionGate {
    fn check_admission(&self, agent_id: &AgentId) -> Result<(), WorkerExecutorError> {
        let pause = {
            let mut pause = self.pause.lock().unwrap();
            if pause
                .as_ref()
                .is_some_and(|pause| &pause.agent_id == agent_id)
            {
                pause.take()
            } else {
                None
            }
        };
        if let Some(pause) = pause {
            let _ = pause.entered.send(());
            // The service boundary is synchronous. Return the Tokio worker while it is held.
            tokio::task::block_in_place(|| pause.release.recv_timeout(Duration::from_secs(30)))
                .map_err(|error| {
                    WorkerExecutorError::runtime(format!("startup admission gate failed: {error}"))
                })?;
        }
        self.inner.check_admission(agent_id)
    }

    fn is_ready(&self) -> bool {
        self.inner.is_ready()
    }
    fn check_worker(&self, agent_id: &AgentId) -> Result<(), WorkerExecutorError> {
        self.inner.check_worker(agent_id)
    }
    fn assign_shards(
        &self,
        number_of_shards: usize,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        revision: ShardLeaseRevision,
    ) -> Result<ShardDeliveryOutcome, WorkerExecutorError> {
        self.inner
            .assign_shards(number_of_shards, shard_epochs, revision)
    }
    fn register(
        &self,
        number_of_shards: usize,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        expires_at: Option<Instant>,
        revision: ShardLeaseRevision,
    ) -> ShardDeliveryOutcome {
        self.inner
            .register(number_of_shards, shard_epochs, expires_at, revision)
    }
    fn install_unexpiring(
        &self,
        number_of_shards: usize,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
    ) -> ShardDeliveryOutcome {
        self.inner
            .install_unexpiring(number_of_shards, shard_epochs)
    }
    fn revoke_shards(
        &self,
        shard_ids: &HashSet<ShardId>,
        revision: ShardLeaseRevision,
    ) -> Result<ShardDeliveryOutcome, WorkerExecutorError> {
        self.inner.revoke_shards(shard_ids, revision)
    }
    fn update_lease(
        &self,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        expires_at: Instant,
        revision: ShardLeaseRevision,
    ) -> Result<ShardDeliveryOutcome, WorkerExecutorError> {
        self.inner.update_lease(shard_epochs, expires_at, revision)
    }
    fn clear_assignment(&self) {
        self.inner.clear_assignment()
    }
    fn current_assignment(&self) -> Result<ShardAssignment, WorkerExecutorError> {
        self.inner.current_assignment()
    }
    fn try_get_current_assignment(&self) -> Option<ShardAssignment> {
        self.inner.try_get_current_assignment()
    }
}
