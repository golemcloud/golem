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
async fn pending_suspend_append_keeps_result_and_cleanup_ownership_through_a_later_stop(
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
    let key = IdempotencyKey::fresh();
    let (outcome, release_outcome) = executor.gate_next_invocation_outcome_append(&id).await;
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
    let requested = InterruptKind::Suspend(Timestamp::now_utc());
    assert!(!worker.terminal_interrupt_pending_for_test());

    let mut held_permit = Box::pin(executor.acquire_account_concurrent_agent_permit(AgentId {
        component_id: component.id,
        agent_id: agent_id!("Counter", "pending-writer-permit-holder").to_string(),
    }));
    assert!((&mut held_permit).now_or_never().is_none());
    let mut signal = worker.raw_interrupt_for_test();
    let mut receipt = worker
        .set_interrupting(requested)
        .await?
        .context("accepted stop has no receipt")?;
    assert!(worker.terminal_interrupt_pending_for_test());

    let mut joined = Box::pin(worker.join_accepted_stops_for_test());
    tokio::time::timeout(Duration::from_secs(10), &mut joined).await??;
    assert_eq!(signal.recv().await?, requested);
    assert!(worker.terminal_interrupt_pending_for_test());
    // The ordinary status no longer keeps the old raw sender after notification.
    assert_eq!(
        signal.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Closed)
    );
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!((&mut held_permit).now_or_never().is_none());
    let unwritten = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await?;
    assert!(!unwritten.values().any(|entry| matches!(
        entry,
        OplogEntry::Suspend { .. } | OplogEntry::Interrupted { .. } | OplogEntry::Error { .. }
    )));
    assert_eq!(
        signal.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Closed)
    );
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    let mut unloaded = Box::pin(worker.await_ready_to_process_commands());
    assert!((&mut unloaded).now_or_never().is_none());

    release_outcome.send(false).unwrap();
    assert_eq!(
        signal.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Closed)
    );
    assert_eq!(
        worker.selected_owner_failure_for_test().await,
        Some(ToolOwnerFailureMetadata::Lifecycle)
    );
    tokio::time::timeout(
        Duration::from_secs(10),
        worker.join_accepted_stops_for_test(),
    )
    .await??;
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
    let entries = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await?;
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
        "[retained-pending-writer] callback retained its append through stop signalling; one Suspend and acknowledgement, original permit released"
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
    let key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &name,
            &key,
            "await_promise",
            crate::raw_params(vec![promise]),
        )
        .await?;
    executor
        .wait_for_status(&id, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;
    tokio::time::timeout(
        Duration::from_secs(10),
        worker.await_ready_to_process_commands(),
    )
    .await??;
    tokio::time::timeout(
        Duration::from_secs(10),
        worker.join_accepted_stops_for_test(),
    )
    .await??;
    let entries = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await?;
    let suspensions: Vec<_> = entries
        .values()
        .filter_map(|entry| match entry {
            OplogEntry::Suspend { timestamp } => Some(InterruptKind::Suspend(*timestamp)),
            _ => None,
        })
        .collect();
    let [recorded_suspend] = suspensions.as_slice() else {
        anyhow::bail!("expected one committed automatic suspension: {suspensions:?}");
    };
    // This is a new request using an older committed timestamp, not the original request receipt.
    let older_suspend = *recorded_suspend;
    // Exact written-terminal claim, internal acknowledgement and retained-drain phases
    // belong to the actual bodies. This native path observes physical release and completion.
    worker.retained_cleanup_for_test().await?;
    assert!(!worker.is_loaded().await);
    // The old resident is physically gone. A real account permit now keeps only the next
    // startup waiting; no private pre-fence phase is needed to distinguish their markers.
    let held_permit = tokio::time::timeout(
        Duration::from_secs(10),
        executor.acquire_account_concurrent_agent_permit(AgentId {
            component_id: component.id,
            agent_id: agent_id!("Counter", "retained-terminal-permit-holder").to_string(),
        }),
    )
    .await?;
    let entries = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await?;
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
    assert_eq!(
        Worker::start_if_needed(worker.clone()).await?,
        Some(attempt)
    );
    let first = if next == NextStart::RestartFollower {
        InterruptKind::Restart
    } else {
        older_suspend
    };
    let mut held_permit = Some(held_permit);
    let mut waiting_handoff = None;
    let (observe, release) = pause_admission(&admission_pause, &id);
    if next != NextStart::Waiting {
        drop(held_permit.take());
        tokio::time::timeout(Duration::from_secs(10), observe).await??;
        assert!(worker.is_loaded().await);
        assert_eq!(
            Worker::start_if_needed(worker.clone()).await?,
            Some(attempt)
        );
    } else {
        assert!(!worker.is_loaded().await);
        waiting_handoff = Some(observe);
    }
    let mut signal = worker.raw_interrupt_for_test();
    let stopping = tokio::spawn({
        let worker = worker.clone();
        async move { worker.set_interrupting(first).await }
    });
    let mut receipt = stopping
        .await??
        .context("new startup stop receipt missing")?;
    if let Some(observe) = waiting_handoff {
        assert_eq!(signal.recv().await?, first);
        assert!(matches!(
            receipt.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        assert_eq!(
            Worker::start_if_needed(worker.clone()).await?,
            Some(attempt)
        );
        // Initial stops queue until permit grant. The old Suspend must not settle
        // this receipt either before the handoff or before the new owner starts.
        drop(held_permit.take());
        tokio::time::timeout(Duration::from_secs(10), observe).await??;
    } else {
        assert_eq!(signal.recv().await?, first);
    }
    tokio::time::timeout(
        Duration::from_secs(10),
        worker.join_accepted_stops_for_test(),
    )
    .await??;
    assert_eq!(
        worker.terminal_interrupt_pending_for_test(),
        next != NextStart::RestartFollower
    );
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert_eq!(
        Worker::start_if_needed(worker.clone()).await?,
        Some(attempt)
    );

    {
        // The native loop takes the published request before checking admission again.
        // An older Suspend preserves the later start; Restart acknowledges and retries.
        let (continued, release_continuation) = pause_admission(&admission_pause, &id);
        release
            .send(())
            .map_err(|_| anyhow!("startup admission gate was lost"))?;
        tokio::time::timeout(Duration::from_secs(10), continued).await??;
        assert!(!worker.terminal_interrupt_pending_for_test());
        assert_eq!(
            Worker::start_if_needed(worker.clone()).await?,
            Some(attempt)
        );
        if next == NextStart::RestartFollower {
            tokio::time::timeout(Duration::from_secs(10), receipt.recv()).await??;
        } else {
            assert!(matches!(
                receipt.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ));
        }
        let later = InterruptKind::Interrupt(Timestamp::now_utc());
        let later_receipt = worker.set_interrupting(later).await?;
        assert_eq!(later_receipt.is_some(), next != NextStart::RestartFollower);
        worker.join_accepted_stops_for_test().await?;
        assert!(worker.terminal_interrupt_pending_for_test());
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
    assert!(
        startup_result
            .unwrap_err()
            .to_string()
            .contains("Worker stopped before startup completed")
    );
    worker.join_accepted_stops_for_test().await?;
    worker.retained_cleanup_for_test().await?;
    assert!(!worker.is_loaded().await);
    // A no-Store stop completes startup before the invocation-loop task drops its permit.
    // Require the actual scheduler return, not that notification.
    let returned = tokio::time::timeout(
        Duration::from_secs(10),
        executor.acquire_account_concurrent_agent_permit(AgentId {
            component_id: component.id,
            agent_id: agent_id!("Counter", "retained-terminal-release-probe").to_string(),
        }),
    )
    .await?;
    drop(returned);
    let entries = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await?;
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
        1
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
