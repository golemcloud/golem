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

use super::interrupt::{InterruptGuard, Interrupts};
use super::{
    PendingWorkerInterrupt, StopCompletion, StopProgressState, StopPublication, WorkerCommand,
    WorkerInstance, stop_cleanup_join_error,
};
use crate::metrics::resource_release::{Cause, Failure, ReleaseReceipt, ReleaseScope, Stage};
use crate::model::ExecutionStatus;
use crate::services::resource_limits::MonthlyResourceExhaustion;
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
#[cfg(test)]
use golem_common::model::IdempotencyKey;
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use tokio::task::JoinHandle;
#[cfg(test)]
use uuid::Uuid;

pub(super) fn publication(
    admission: &StopProgressState,
    status: &RwLock<ExecutionStatus>,
) -> (Arc<StopPublication>, Option<StopCompletion>) {
    match &admission.publication {
        Some((publication, completion)) => (publication.clone(), Some(completion.clone())),
        None => (
            Arc::new(StopPublication {
                release_scope: admission.release_scope.clone(),
                cause: OnceLock::new(),
                receipt: Arc::new(tokio::sync::broadcast::channel(1).0),
                signal: match &*status.read().unwrap() {
                    ExecutionStatus::Loading {
                        interrupt_signal, ..
                    }
                    | ExecutionStatus::Running {
                        interrupt_signal, ..
                    } => Some(interrupt_signal.clone()),
                    _ => None,
                },
            }),
            None,
        ),
    }
}

pub(super) fn freeze(
    interrupts: &mut InterruptGuard<'_>,
    publication: &StopPublication,
    monthly: &Mutex<Option<(InterruptKind, MonthlyResourceExhaustion)>>,
) -> Option<PendingWorkerInterrupt> {
    if let Some(pending) = publication.cause.get() {
        return Some(*pending);
    }
    let pending = interrupts.freeze()?;
    publication.cause.set(pending).expect("one stop cause");
    let monthly = monthly
        .lock()
        .unwrap()
        .as_ref()
        .filter(|(kind, _)| *kind == pending.kind)
        .map(|(_, resource)| *resource);
    let cause = match monthly {
        Some(MonthlyResourceExhaustion::Compute) => Cause::MonthlyCompute,
        Some(MonthlyResourceExhaustion::Memory) => Cause::MonthlyMemory,
        Some(MonthlyResourceExhaustion::DurableStorage) => Cause::MonthlyDurableStorage,
        Some(MonthlyResourceExhaustion::EphemeralStorage) => Cause::MonthlyEphemeralStorage,
        None if pending.kind == InterruptKind::Jump => Cause::Jump,
        None => pending.unload_request.reason.release_cause(),
    };
    publication.release_scope.freeze_cause(cause);
    Some(pending)
}

pub(super) async fn freeze_pending(
    interrupts: &Interrupts,
    publication: &StopPublication,
    monthly: &Mutex<Option<(InterruptKind, MonthlyResourceExhaustion)>>,
) -> Result<PendingWorkerInterrupt, WorkerExecutorError> {
    let mut interrupts = interrupts.lock().await;
    let pending = freeze(&mut interrupts, publication, monthly)
        .ok_or_else(|| WorkerExecutorError::runtime("Accepted stop lost its pending cause"))?;
    Ok(pending)
}

pub(super) struct PublicationRuntime<'a> {
    pub lifecycle: &'a WorkerInstance,
    pub resident_generation: &'a AtomicU64,
    pub accepted_generation: u64,
}

pub(super) async fn publish(
    runtime: PublicationRuntime<'_>,
    interrupts: &Interrupts,
    status: &std::sync::RwLock<ExecutionStatus>,
    publication: &StopPublication,
    pending: PendingWorkerInterrupt,
) -> Result<bool, WorkerExecutorError> {
    if runtime.resident_generation.load(Ordering::Acquire) != runtime.accepted_generation {
        return Err(WorkerExecutorError::runtime(
            "Stop runtime changed before publication",
        ));
    }
    let mut interrupts = interrupts.lock().await;
    let mut status = status.write().unwrap();
    let acknowledge_without_store = !status.publish_interrupt(
        pending.kind,
        publication.receipt.clone(),
        publication.signal.as_ref(),
    );
    interrupts.publish();
    if let WorkerInstance::Running(running) = runtime.lifecycle.deletion_runtime() {
        let _ = running.sender.send(WorkerCommand::WorkAvailable);
    }
    Ok(acknowledge_without_store)
}

pub(super) fn verify_owner_fence(
    operations: &crate::durable_host::tool::operation::OwnerToolOperations,
    kind: InterruptKind,
) -> Result<(), WorkerExecutorError> {
    use crate::durable_host::tool::operation::OwnerFailureWinner;
    if matches!(operations.selected_owner_failure(), Some(OwnerFailureWinner::Lifecycle(actual)) if actual == kind)
    {
        Ok(())
    } else {
        Err(WorkerExecutorError::runtime(
            "Stop lost the owner failure election",
        ))
    }
}

pub(super) fn acknowledge_without_store(publication: &StopPublication, acknowledge: bool) {
    if acknowledge {
        let _ = publication.receipt.send(());
    }
}

pub(super) async fn publish_follower(
    interrupts: &Interrupts,
    leader: StopCompletion,
) -> Result<(), WorkerExecutorError> {
    leader.await?;
    let mut interrupts = interrupts.lock().await;
    if interrupts.freeze().is_some() {
        interrupts.publish();
    }
    Ok(())
}

pub(super) fn retain_driver(
    admission: &mut StopProgressState,
    publication: Arc<StopPublication>,
    is_leader: bool,
    was_running: bool,
    driver_receipt: ReleaseReceipt,
    task: JoinHandle<Result<bool, WorkerExecutorError>>,
) -> Shared<BoxFuture<'static, Result<bool, WorkerExecutorError>>> {
    let scope = publication.release_scope.clone();
    let progress = async move {
        let joined = task.await;
        let joined_at = scope.now();
        let result = match joined {
            Ok(result) => {
                if result.is_err() {
                    scope.fail(Stage::EndToEnd, Failure::StopDriver);
                }
                result
            }
            Err(error) => {
                scope.fail(
                    Stage::EndToEnd,
                    if error.is_panic() {
                        Failure::Panic
                    } else {
                        Failure::ObserverLost
                    },
                );
                Err(stop_cleanup_join_error(error))
            }
        };
        if !was_running {
            scope.seal();
        }
        driver_receipt.complete_at(joined_at);
        result
    }
    .boxed()
    .shared();
    let completion = progress
        .clone()
        .map(|result| result.map(|_| ()))
        .boxed()
        .shared();
    if is_leader {
        admission.publication = Some((publication, completion.clone()));
    }
    admission.tasks.push(completion);
    progress
}

pub(super) async fn join_waiting_task(
    task: JoinHandle<()>,
    scope: &ReleaseScope,
    waiting_receipt: ReleaseReceipt,
) -> Result<(), WorkerExecutorError> {
    task.abort();
    let joined = task.await;
    let joined_at = scope.now();
    if let Err(error) = &joined
        && !error.is_cancelled()
    {
        scope.fail(
            Stage::EndToEnd,
            if error.is_panic() {
                Failure::Panic
            } else {
                Failure::Other
            },
        );
    }
    if ![
        Stage::PrimaryStoreDrop,
        Stage::FilesystemDeleted,
        Stage::PermitReleased,
    ]
    .into_iter()
    .any(|stage| scope.has_receipt(stage))
    {
        scope
            .receipt(Stage::PermitWaitJoined)
            .complete_at(joined_at);
    }
    waiting_receipt.complete_at(joined_at);
    if let Err(error) = joined
        && !error.is_cancelled()
    {
        return Err(WorkerExecutorError::runtime(format!(
            "Waiting worker failed: {error}"
        )));
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::metrics::resource_release::tests::TestMetrics;
    use crate::worker::{
        StopProgress, UnloadReason, UnloadRequest, WorkerInterruptState, join_accepted_stops,
    };
    use golem_common::model::AgentId;
    use golem_common::model::OplogIndex;
    use golem_common::model::Timestamp;
    use golem_common::model::agent::AgentMode;
    use golem_common::model::component::ComponentId;
    use test_r::{test, timeout};

    fn demand(kind: InterruptKind) -> PendingWorkerInterrupt {
        PendingWorkerInterrupt {
            kind,
            reacquire_permits: false,
            unload_request: UnloadRequest::ordinary(UnloadReason::Interrupt),
        }
    }

    pub(in crate::worker) fn running_lifecycle(
        interrupts: Arc<Interrupts>,
        start_attempt: Uuid,
        owner_generation_started: bool,
    ) -> WorkerInstance {
        WorkerInstance::Running(super::super::RunningWorker {
            handle: None,
            sender: tokio::sync::mpsc::unbounded_channel().0,
            queue: Default::default(),
            waiting_for_command: Default::default(),
            concurrent_agent_permit_held: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            filesystem_activity: Default::default(),
            filesystem_snapshot_slot: Default::default(),
            unload_request: Default::default(),
            idle_since_millis: Default::default(),
            interrupts,
            resume_replay_pending: Default::default(),
            start_attempt,
            owner_generation_started,
        })
    }

    fn owner_resources() -> Arc<super::super::instance::OwnerRuntimeResources> {
        Arc::new(super::super::instance::OwnerRuntimeResources::new(
            Arc::new(crate::services::resource_limits::AtomicResourceEntry::new(
                u64::MAX,
                usize::MAX,
                usize::MAX,
                u64::MAX,
                2,
            )),
            Arc::new(std::sync::RwLock::new(ExecutionStatus::loading(
                AgentMode::Durable,
            ))),
        ))
    }

    #[test]
    #[timeout("5s")]
    async fn publication_checks_generation_and_original_signal_without_a_writer_protocol() {
        for case in ["running", "stale_generation", "waiting", "suspended"] {
            let interrupts = Arc::new(Interrupts::default());
            let kind = InterruptKind::Suspend(Timestamp::from(60_u64));
            let attempt = Uuid::new_v4();
            let lifecycle = if case == "suspended" {
                WorkerInstance::Unloaded {
                    startup_failure: None,
                }
            } else if case == "waiting" {
                WorkerInstance::WaitingForPermit(super::super::WaitingWorker {
                    handle: Mutex::new(None),
                    start_attempt: attempt,
                })
            } else {
                running_lifecycle(interrupts.clone(), attempt, true)
            };
            let status = std::sync::RwLock::new(if case == "suspended" {
                ExecutionStatus::Suspended {
                    agent_mode: AgentMode::Durable,
                    timestamp: Timestamp::now_utc(),
                }
            } else if case == "waiting" {
                ExecutionStatus::loading(AgentMode::Durable)
            } else {
                ExecutionStatus::Running {
                    agent_mode: AgentMode::Durable,
                    timestamp: Timestamp::now_utc(),
                    interrupt_signal: Arc::new(tokio::sync::broadcast::channel(8).0),
                }
            });
            let (publication, _) = publication(&StopProgressState::default(), &status);
            let mut receipt = publication.receipt.subscribe();
            let mut raw = publication.signal.as_ref().map(|signal| signal.subscribe());
            assert!(interrupts.lock().await.queue(demand(kind)));
            let pending = freeze_pending(&interrupts, &publication, &Mutex::new(None))
                .await
                .unwrap();
            let generation = AtomicU64::new(if case == "stale_generation" { 8 } else { 7 });
            let result = publish(
                PublicationRuntime {
                    lifecycle: &lifecycle,
                    resident_generation: &generation,
                    accepted_generation: 7,
                },
                &interrupts,
                &status,
                &publication,
                pending,
            )
            .await;
            if case == "stale_generation" {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("Stop runtime changed")
                );
                assert!(matches!(
                    raw.as_mut().unwrap().try_recv(),
                    Err(tokio::sync::broadcast::error::TryRecvError::Empty)
                ));
                continue;
            }
            let acknowledge = result.unwrap();
            assert_eq!(acknowledge, case == "suspended");
            assert!(matches!(
                receipt.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ));
            let guard = interrupts.lock().await;
            assert!(matches!(*guard, WorkerInterruptState::Pending(actual) if actual.kind == kind));
            drop(guard);
            if let Some(raw) = &mut raw {
                assert_eq!(raw.recv().await.unwrap(), kind);
                assert!(matches!(
                    raw.try_recv(),
                    Err(tokio::sync::broadcast::error::TryRecvError::Empty)
                ));
            }
            acknowledge_without_store(&publication, acknowledge);
            if acknowledge {
                assert_eq!(receipt.recv().await.unwrap(), ());
            }
            assert!(matches!(
                receipt.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ));
        }
    }

    #[test]
    #[timeout("5s")]
    async fn teardown_and_driver_fences_share_the_frozen_winner_in_both_orders() {
        use crate::durable_host::tool::operation::{OwnerFailureWinner, OwnerToolOperations};
        for teardown_first in [false, true] {
            let interrupts = Arc::new(Interrupts::default());
            let operations = OwnerToolOperations::new();
            let resources = owner_resources();
            let entities = Mutex::new(Default::default());
            let accepting = std::sync::atomic::AtomicBool::new(true);
            let fence_generation = AtomicU64::new(0);
            let fence = crate::services::active_agents::EntityFence {
                operations: &operations,
                entities: &entities,
                accepting: &accepting,
                generation: &fence_generation,
                resources: &resources,
            };
            let quota = InterruptKind::Suspend(Timestamp::from(70_u64));
            let fallback = InterruptKind::Suspend(Timestamp::from(71_u64));
            let monthly = Mutex::new(Some((quota, MonthlyResourceExhaustion::Compute)));
            let status = std::sync::RwLock::new(ExecutionStatus::loading(AgentMode::Durable));
            let (publication, _) = publication(&StopProgressState::default(), &status);
            let mut raw = publication.signal.as_ref().unwrap().subscribe();
            {
                let mut guard = interrupts.lock().await;
                assert!(guard.queue(demand(quota)));
                assert!(!guard.queue(demand(fallback)));
                if teardown_first {
                    let selected = freeze(&mut guard, &publication, &monthly).unwrap();
                    assert_eq!(selected.kind, quota);
                }
            }
            if teardown_first {
                fence.begin(OwnerFailureWinner::Lifecycle(quota)).await;
            }
            let pending = freeze_pending(&interrupts, &publication, &monthly)
                .await
                .unwrap();
            fence
                .begin(OwnerFailureWinner::Lifecycle(pending.kind))
                .await;
            if !teardown_first {
                let selected = {
                    let mut guard = interrupts.lock().await;
                    freeze(&mut guard, &publication, &monthly).unwrap()
                };
                fence
                    .begin(OwnerFailureWinner::Lifecycle(selected.kind))
                    .await;
            }
            verify_owner_fence(&operations, quota).unwrap();
            assert!(!accepting.load(Ordering::Acquire));
            let attempt = Uuid::new_v4();
            let lifecycle = running_lifecycle(interrupts.clone(), attempt, true);
            assert!(
                !publish(
                    PublicationRuntime {
                        lifecycle: &lifecycle,
                        resident_generation: &AtomicU64::new(7),
                        accepted_generation: 7,
                    },
                    &interrupts,
                    &status,
                    &publication,
                    pending,
                )
                .await
                .unwrap()
            );
            assert_eq!(raw.recv().await.unwrap(), quota);
            assert!(matches!(
                raw.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ));
        }
        let operations = OwnerToolOperations::new();
        operations
            .select_owner_failure(OwnerFailureWinner::Infrastructure(
                WorkerExecutorError::runtime("independent owner failure"),
            ))
            .await;
        let resources = owner_resources();
        let entities = Mutex::new(Default::default());
        let accepting = std::sync::atomic::AtomicBool::new(true);
        let generation = AtomicU64::new(0);
        crate::services::active_agents::EntityFence {
            operations: &operations,
            entities: &entities,
            accepting: &accepting,
            generation: &generation,
            resources: &resources,
        }
        .begin(OwnerFailureWinner::Lifecycle(InterruptKind::Restart))
        .await;
        assert!(verify_owner_fence(&operations, InterruptKind::Restart).is_err());
        assert!(!accepting.load(Ordering::Acquire));
        assert!(matches!(
            operations.selected_owner_failure(),
            Some(OwnerFailureWinner::Infrastructure(_))
        ));
    }

    #[test]
    #[timeout("5s")]
    async fn accepted_unfrozen_stop_retains_entity_store_registration_and_active_tail() {
        use crate::durable_host::tool::operation::{OwnerFailureWinner, OwnerToolOperations};
        use crate::worker::entity_slot::EntitySlot;
        use golem_common::model::entity::{
            EntityActivation, EntityActivationPolicy, EntityInvocationId, EntityInvocationScope,
            ExecutableTarget, FilesystemCapability, InvocationExecutionMode, OwnedAgentEntityId,
        };
        use golem_common::model::tool::{SecretKeyScope, ToolFilesystemAccess};
        struct StoreData(Arc<std::sync::atomic::AtomicBool>);
        impl Drop for StoreData {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let owner = golem_common::model::OwnedAgentId::new(
            golem_common::model::environment::EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "owner".into(),
            },
        );
        let activation = Arc::new(
            EntityActivation::new(
                ExecutableTarget::new(ComponentId::new(), 1_u64.try_into().unwrap()),
                1_u64.try_into().unwrap(),
                EntityActivationPolicy::ToolMiddleware {
                    middleware_name: "stop-body".try_into().unwrap(),
                    provision: Default::default(),
                    config_keys_readable: Default::default(),
                    secret_keys_readable: SecretKeyScope::All,
                    secret_keys_revealable: SecretKeyScope::All,
                    filesystem_access: ToolFilesystemAccess::Unset,
                },
                FilesystemCapability::Incapable,
            )
            .unwrap(),
        );
        let entity = OwnedAgentEntityId {
            owner: owner.clone(),
            entity: activation.entity(),
        };
        let slot = Arc::new(EntitySlot::new(entity.clone()));
        let scope = EntityInvocationScope::new(
            EntityInvocationId::new(entity.clone(), OplogIndex::from_u64(2)).unwrap(),
            OplogIndex::from_u64(1),
            activation,
            golem_common::model::agent::Principal::Agent(
                golem_common::model::agent::AgentPrincipal {
                    agent_id: owner.agent_id,
                },
            ),
            InvocationExecutionMode::Live,
            IdempotencyKey::fresh(),
            false,
            false,
            IdempotencyKey::fresh(),
        )
        .unwrap();
        let registration = slot.register(&scope, Default::default()).unwrap();
        let resources = owner_resources();
        registration
            .attach_linear_memory(
                crate::services::linear_memory::LinearMemoryTracker::new_with_metering(
                    64,
                    64,
                    AgentMode::Durable,
                    false,
                    resources.resource_limits(),
                    Arc::new(Mutex::new(
                        crate::services::active_agents::MemoryGrant::inert(64),
                    )),
                    false,
                ),
            )
            .unwrap();
        let engine = wasmtime::Engine::default();
        let primary_dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let entity_dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let primary = super::super::ObservedPrimaryStore::new(
            &engine,
            StoreData(primary_dropped.clone()),
            None,
        );
        let entity_store = super::super::ObservedPrimaryStore::new(
            &engine,
            StoreData(entity_dropped.clone()),
            None,
        );
        let cancellation = registration.cancellation();
        let entity_task = tokio::spawn(async move {
            cancellation.cancelled().await;
            drop(entity_store);
            drop(registration);
        });
        let tracker = crate::durable_host::tail_work::TailWorkTracker::new();
        let activity = tracker.activity();
        let status = Arc::new(std::sync::RwLock::new(ExecutionStatus::loading(
            AgentMode::Durable,
        )));
        let tail_signal = status.read().unwrap().create_await_interrupt_signal();
        let tail = tokio::spawn(async move {
            let kind = tail_signal.await;
            drop(activity);
            kind
        });
        let operations = OwnerToolOperations::new();
        let entities = Arc::new(Mutex::new(std::collections::HashMap::from([(
            entity.entity,
            slot.clone(),
        )])));
        let interrupts = Arc::new(Interrupts::default());
        let progress: StopProgress = Arc::default();
        let kind = InterruptKind::Suspend(Timestamp::from(80_u64));
        let mut guard = interrupts.lock().await;
        assert!(guard.queue(demand(kind)));
        let publication = {
            let mut admission = progress.lock().unwrap();
            let (publication, _) = publication(&admission, &status);
            let captured = publication.clone();
            let captured_interrupts = interrupts.clone();
            let captured_status = status.clone();
            let captured_ops = operations.clone();
            let captured_entities = entities.clone();
            let task = tokio::spawn(async move {
                let pending =
                    freeze_pending(&captured_interrupts, &captured, &Mutex::new(None)).await?;
                crate::services::active_agents::EntityFence {
                    operations: &captured_ops,
                    entities: &captured_entities,
                    accepting: &std::sync::atomic::AtomicBool::new(true),
                    generation: &AtomicU64::new(0),
                    resources: &resources,
                }
                .begin(OwnerFailureWinner::Lifecycle(pending.kind))
                .await;
                verify_owner_fence(&captured_ops, pending.kind)?;
                let attempt = Uuid::new_v4();
                let lifecycle = running_lifecycle(captured_interrupts.clone(), attempt, true);
                publish(
                    PublicationRuntime {
                        lifecycle: &lifecycle,
                        resident_generation: &AtomicU64::new(7),
                        accepted_generation: 7,
                    },
                    &captured_interrupts,
                    &captured_status,
                    &captured,
                    pending,
                )
                .await?;
                Ok(false)
            });
            let receipt = publication.release_scope.accepted_driver();
            drop(retain_driver(
                &mut admission,
                publication.clone(),
                true,
                true,
                receipt,
                task,
            ));
            publication
        };
        assert!(publication.cause.get().is_none());
        assert!(operations.selected_owner_failure().is_none());
        assert_eq!(slot.active_invocation_count(), 1);
        assert!(slot.active_invocations()[0].store_attached);
        assert_eq!(slot.charged_linear_memory_bytes(), 64);
        assert!(tracker.has_active());
        assert!(!primary_dropped.load(Ordering::Acquire));
        assert!(!entity_dropped.load(Ordering::Acquire));
        assert!(progress.lock().unwrap().ready().is_none());
        drop(guard);
        join_accepted_stops(&progress).await.unwrap();
        entity_task.await.unwrap();
        assert_eq!(tail.await.unwrap(), kind);
        slot.wait_drained().await;
        assert_eq!(slot.active_invocation_count(), 0);
        assert!(!tracker.has_active());
        assert!(entity_dropped.load(Ordering::Acquire));
        assert!(!primary_dropped.load(Ordering::Acquire));
        drop(primary);
        assert!(primary_dropped.load(Ordering::Acquire));
    }

    #[test]
    #[timeout("5s")]
    async fn registered_unfrozen_stops_preserve_supersession_first_terminal_and_loading_subscribers()
     {
        use crate::durable_host::tool::operation::{OwnerFailureWinner, OwnerToolOperations};
        let suspend = InterruptKind::Suspend(Timestamp::from(40_u64));
        let interrupt = InterruptKind::Interrupt(Timestamp::from(41_u64));
        for (first, second, expected, second_accepted) in [
            (InterruptKind::Restart, suspend, suspend, true),
            (InterruptKind::Jump, interrupt, interrupt, true),
            (suspend, interrupt, suspend, false),
            (interrupt, suspend, interrupt, false),
        ] {
            let interrupts = Arc::new(Interrupts::default());
            let progress: StopProgress = Arc::default();
            let status = Arc::new(std::sync::RwLock::new(ExecutionStatus::loading(
                AgentMode::Durable,
            )));
            let operations = OwnerToolOperations::new();
            let mut existing = status.read().unwrap().create_await_interrupt_signal();
            let mut guard = interrupts.lock().await;
            assert!(guard.queue(demand(first)));
            let (publication, receipt, mut raw) = {
                let mut admission = progress.lock().unwrap();
                let (publication, leader) = publication(&admission, &status);
                assert!(leader.is_none());
                let receipt = publication.receipt.subscribe();
                let raw = publication.signal.as_ref().unwrap().subscribe();
                let driver_publication = publication.clone();
                let driver_interrupts = interrupts.clone();
                let driver_status = status.clone();
                let driver_operations = operations.clone();
                let task = tokio::spawn(async move {
                    let pending =
                        freeze_pending(&driver_interrupts, &driver_publication, &Mutex::new(None))
                            .await?;
                    let resources = owner_resources();
                    crate::services::active_agents::EntityFence {
                        operations: &driver_operations,
                        entities: &Mutex::new(Default::default()),
                        accepting: &std::sync::atomic::AtomicBool::new(true),
                        generation: &AtomicU64::new(0),
                        resources: &resources,
                    }
                    .begin(OwnerFailureWinner::Lifecycle(pending.kind))
                    .await;
                    verify_owner_fence(&driver_operations, pending.kind)?;
                    let attempt = Uuid::new_v4();
                    let lifecycle = running_lifecycle(driver_interrupts.clone(), attempt, true);
                    let generation = AtomicU64::new(7);
                    let acknowledge = publish(
                        PublicationRuntime {
                            lifecycle: &lifecycle,
                            resident_generation: &generation,
                            accepted_generation: 7,
                        },
                        &driver_interrupts,
                        &driver_status,
                        &driver_publication,
                        pending,
                    )
                    .await?;
                    acknowledge_without_store(&driver_publication, acknowledge);
                    Ok(false)
                });
                let driver_receipt = publication.release_scope.accepted_driver();
                drop(retain_driver(
                    &mut admission,
                    publication.clone(),
                    true,
                    true,
                    driver_receipt,
                    task,
                ));
                (publication, receipt, raw)
            };
            assert_eq!(guard.queue(demand(second)), second_accepted);
            if second_accepted {
                let mut admission = progress.lock().unwrap();
                let (same, leader) = super::publication(&admission, &status);
                assert!(Arc::ptr_eq(&same, &publication));
                let leader = leader.unwrap();
                let driver_interrupts = interrupts.clone();
                let task = tokio::spawn(async move {
                    publish_follower(&driver_interrupts, leader).await?;
                    Ok(false)
                });
                let driver_receipt = same.release_scope.accepted_driver();
                drop(retain_driver(
                    &mut admission,
                    same,
                    false,
                    true,
                    driver_receipt,
                    task,
                ));
            }
            assert!(
                matches!(*guard, WorkerInterruptState::Unpublished(pending) if pending.kind == expected)
            );
            assert!(publication.cause.get().is_none());
            assert!(operations.selected_owner_failure().is_none());
            assert!(matches!(
                raw.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ));
            assert!(existing.as_mut().now_or_never().is_none());
            let late_pending = status.read().unwrap().create_await_interrupt_signal();
            let mut receipt = receipt;
            assert!(matches!(
                receipt.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ));
            assert_eq!(
                progress.lock().unwrap().tasks.len(),
                1 + usize::from(second_accepted)
            );
            drop(guard);
            join_accepted_stops(&progress).await.unwrap();
            assert_eq!(publication.cause.get().unwrap().kind, expected);
            assert_eq!(raw.recv().await.unwrap(), expected);
            assert_eq!(existing.await, expected);
            assert_eq!(late_pending.await, expected);
            let late_published = status.read().unwrap().create_await_interrupt_signal();
            assert_eq!(late_published.await, expected);
            assert!(
                !interrupts.lock().await.queue(demand(second)),
                "the published first terminal rejects a later or duplicate terminal"
            );
            assert_eq!(publication.cause.get().unwrap().kind, expected);
            assert!(matches!(
                raw.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ));
            assert!(
                matches!(operations.selected_owner_failure(), Some(OwnerFailureWinner::Lifecycle(actual)) if actual == expected)
            );
        }
    }

    #[test]
    #[timeout("5s")]
    async fn retained_restart_publisher_keeps_later_terminal_demand_and_its_progress() {
        let interrupts = Arc::new(Interrupts::default());
        let progress: StopProgress = Arc::default();
        let status = Arc::new(std::sync::RwLock::new(ExecutionStatus::loading(
            AgentMode::Durable,
        )));
        let mut guard = interrupts.lock().await;
        assert!(guard.queue(demand(InterruptKind::Restart)));
        let first = {
            let mut admission = progress.lock().unwrap();
            let (publication, _) = publication(&admission, &status);
            let captured = publication.clone();
            let captured_interrupts = interrupts.clone();
            let captured_status = status.clone();
            let task = tokio::spawn(async move {
                let pending =
                    freeze_pending(&captured_interrupts, &captured, &Mutex::new(None)).await?;
                let attempt = Uuid::new_v4();
                let lifecycle = running_lifecycle(captured_interrupts.clone(), attempt, true);
                publish(
                    PublicationRuntime {
                        lifecycle: &lifecycle,
                        resident_generation: &AtomicU64::new(7),
                        accepted_generation: 7,
                    },
                    &captured_interrupts,
                    &captured_status,
                    &captured,
                    pending,
                )
                .await?;
                Ok(false)
            });
            let receipt = publication.release_scope.accepted_driver();
            drop(retain_driver(
                &mut admission,
                publication.clone(),
                true,
                true,
                receipt,
                task,
            ));
            publication
        };
        drop(guard);
        join_accepted_stops(&progress).await.unwrap();
        assert_eq!(
            interrupts.take().await.unwrap().kind,
            InterruptKind::Restart
        );
        let later = InterruptKind::Interrupt(Timestamp::from(50_u64));
        let mut guard = interrupts.lock().await;
        assert!(guard.queue(demand(later)));
        {
            let mut admission = progress.lock().unwrap();
            let (publication, leader) = publication(&admission, &status);
            assert!(Arc::ptr_eq(&publication, &first));
            let leader = leader.unwrap();
            let captured = interrupts.clone();
            let task = tokio::spawn(async move {
                publish_follower(&captured, leader).await?;
                Ok(false)
            });
            let receipt = publication.release_scope.accepted_driver();
            drop(retain_driver(
                &mut admission,
                publication,
                false,
                true,
                receipt,
                task,
            ));
            assert!(admission.ready().is_none());
            assert_eq!(admission.tasks.len(), 2);
        }
        assert!(
            matches!(*guard, WorkerInterruptState::Unpublished(pending) if pending.kind == later)
        );
        assert_eq!(first.cause.get().unwrap().kind, InterruptKind::Restart);
        drop(guard);
        join_accepted_stops(&progress).await.unwrap();
        assert!(interrupts.terminal_pending());
        assert_eq!(first.cause.get().unwrap().kind, InterruptKind::Restart);
        assert_eq!(interrupts.take().await.unwrap().kind, later);
        assert!(progress.lock().unwrap().ready().unwrap().is_ok());
    }

    #[test]
    #[timeout("5s")]
    async fn captured_waiting_task_join_stays_pending_until_entered_io_finishes() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut peer = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut backend, _) = listener.accept().unwrap();
        let (started, entered) = tokio::sync::oneshot::channel();
        let task = tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            backend.read_exact(&mut [0u8]).unwrap();
        });
        entered.await.unwrap();
        let metrics = TestMetrics::new();
        let scope = metrics.scope();
        let driver = scope.accepted_driver();
        let interrupts = Interrupts::default();
        let mut guard = interrupts.lock().await;
        assert!(guard.queue(demand(InterruptKind::Interrupt(Timestamp::from(85_u64)))));
        let (publication, _) = publication(
            &StopProgressState {
                release_scope: scope.clone(),
                ..Default::default()
            },
            &RwLock::new(ExecutionStatus::loading(AgentMode::Durable)),
        );
        freeze(&mut guard, &publication, &Mutex::new(None)).unwrap();
        drop(guard);
        let waiting = scope.accepted_join();
        let mut joining = Box::pin(join_waiting_task(task, &scope, waiting));
        assert!(futures::poll!(joining.as_mut()).is_pending());
        assert_eq!(
            metrics.count("accepted_stop", "permit_wait_joined", "released"),
            0.0
        );
        peer.write_all(&[1]).unwrap();
        joining.await.unwrap();
        driver.complete();
        scope.seal();
        assert_eq!(
            metrics.count("accepted_stop", "permit_wait_joined", "released"),
            1.0
        );
    }

    #[test]
    #[timeout("5s")]
    async fn guarded_freeze_preserves_cause_and_releases_guard_without_a_writer_join() {
        let interrupts = Interrupts::default();
        let admission = StopProgressState::default();
        let (publication, leader) = publication(
            &admission,
            &RwLock::new(ExecutionStatus::loading(AgentMode::Durable)),
        );
        assert!(leader.is_none());
        let kind = InterruptKind::Suspend(Timestamp::from(10_u64));
        let monthly = Mutex::new(Some((kind, MonthlyResourceExhaustion::Compute)));
        let mut guard = interrupts.lock().await;
        assert!(guard.queue(demand(kind)));
        let mut freezing = Box::pin(freeze_pending(&interrupts, &publication, &monthly));
        assert!(futures::poll!(freezing.as_mut()).is_pending());
        assert!(publication.cause.get().is_none());
        drop(guard);
        let pending = freezing.await.unwrap();
        assert_eq!(pending.kind, kind);
        assert_eq!(publication.cause.get().unwrap().kind, kind);
        assert!(matches!(
            *interrupts.lock().await,
            WorkerInterruptState::Freezing(actual) if actual.kind == kind
        ));
        assert!(interrupts.try_lock().is_some());
        assert_eq!(
            freeze_pending(&interrupts, &publication, &monthly)
                .await
                .unwrap()
                .kind,
            kind
        );
        assert_eq!(publication.cause.get().unwrap().kind, kind);
        assert!(
            matches!(*interrupts.lock().await, WorkerInterruptState::Freezing(actual) if actual.kind == kind)
        );
    }

    #[test]
    async fn missing_pending_cause_is_an_error_not_a_publication() {
        let interrupts = Interrupts::default();
        let (publication, _) = publication(
            &StopProgressState::default(),
            &RwLock::new(ExecutionStatus::loading(AgentMode::Durable)),
        );
        let result = freeze_pending(&interrupts, &publication, &Mutex::new(None)).await;
        assert!(
            matches!(result, Err(error) if error.to_string().contains("Accepted stop lost its pending cause"))
        );
        assert!(publication.cause.get().is_none());
        assert!(interrupts.try_lock().is_some());
    }

    #[test]
    async fn teardown_and_driver_freeze_the_same_cause_once_and_keep_release_attribution() {
        let metrics = TestMetrics::new();
        let admission = StopProgressState {
            release_scope: metrics.scope(),
            ..Default::default()
        };
        let (publication, _) = publication(
            &admission,
            &RwLock::new(ExecutionStatus::loading(AgentMode::Durable)),
        );
        let receipt = publication.release_scope.accepted_driver();
        let interrupts = Interrupts::default();
        let kind = InterruptKind::Suspend(Timestamp::from(20_u64));
        let mut guard = interrupts.lock().await;
        assert!(guard.queue(demand(InterruptKind::Restart)));
        assert!(guard.queue(demand(kind)));
        assert_eq!(
            freeze(
                &mut guard,
                &publication,
                &Mutex::new(Some((kind, MonthlyResourceExhaustion::Compute)))
            )
            .unwrap()
            .kind,
            kind
        );
        assert!(!guard.queue(demand(InterruptKind::Interrupt(Timestamp::from(21_u64)))));
        assert_eq!(
            freeze(
                &mut guard,
                &publication,
                &Mutex::new(Some((kind, MonthlyResourceExhaustion::Memory)))
            )
            .unwrap()
            .kind,
            kind
        );
        drop(guard);
        receipt.complete();
        publication.release_scope.seal();
        assert_eq!(
            metrics.value(
                "golem_agent_resource_release_seconds",
                &[
                    ("origin", "accepted_stop"),
                    ("stage", "end_to_end"),
                    ("cause", "monthly_compute")
                ]
            ),
            1.0
        );
        assert_eq!(
            metrics.value(
                "golem_agent_resource_release_seconds",
                &[("cause", "monthly_memory")]
            ),
            0.0
        );
    }

    #[test]
    #[timeout("5s")]
    async fn retained_publication_and_frozen_cause_skip_unused_status_and_monthly_locks() {
        let mut admission = StopProgressState::default();
        let status = RwLock::new(ExecutionStatus::loading(AgentMode::Durable));
        let (first, leader) = publication(&admission, &status);
        assert!(leader.is_none());
        let receipt = first.release_scope.accepted_driver();
        let observer = retain_driver(
            &mut admission,
            first.clone(),
            true,
            false,
            receipt,
            tokio::spawn(async { Ok(false) }),
        );
        assert!(
            std::panic::catch_unwind(|| {
                let _held = status.write().unwrap();
                panic!("unused execution status is poisoned");
            })
            .is_err()
        );
        let (reused, retained) = publication(&admission, &status);
        assert!(Arc::ptr_eq(&first, &reused));
        assert!(
            retained
                .unwrap()
                .ptr_eq(&admission.publication.as_ref().unwrap().1)
        );

        let interrupts = Interrupts::default();
        let kind = InterruptKind::Suspend(Timestamp::from(25_u64));
        let monthly = Mutex::new(Some((kind, MonthlyResourceExhaustion::Compute)));
        let mut guard = interrupts.lock().await;
        assert!(guard.queue(demand(kind)));
        assert_eq!(freeze(&mut guard, &first, &monthly).unwrap().kind, kind);
        assert!(
            std::panic::catch_unwind(|| {
                let _held = monthly.lock().unwrap();
                panic!("unused monthly cause is poisoned");
            })
            .is_err()
        );
        assert_eq!(freeze(&mut guard, &reused, &monthly).unwrap().kind, kind);
        assert_eq!(reused.cause.get().unwrap().kind, kind);
        drop(guard);
        assert!(!observer.await.unwrap());
        assert!(admission.ready().unwrap().is_ok());
    }

    #[test]
    #[timeout("5s")]
    async fn retained_real_tasks_keep_first_publication_and_survive_observer_drop() {
        let progress: StopProgress = Arc::default();
        let (release, dependency) = tokio::sync::oneshot::channel::<()>();
        let (first, observer) = {
            let mut admission = progress.lock().unwrap();
            let (publication, leader) = publication(
                &admission,
                &RwLock::new(ExecutionStatus::loading(AgentMode::Durable)),
            );
            assert!(leader.is_none());
            let receipt = publication.release_scope.accepted_driver();
            let task = tokio::spawn(async move {
                dependency.await.expect("owned task dependency closed");
                Ok(false)
            });
            let observer = retain_driver(
                &mut admission,
                publication.clone(),
                true,
                false,
                receipt,
                task,
            );
            (publication, observer)
        };
        assert!(observer.clone().now_or_never().is_none());
        {
            let mut admission = progress.lock().unwrap();
            let (publication, leader) = publication(
                &admission,
                &RwLock::new(ExecutionStatus::loading(AgentMode::Durable)),
            );
            assert!(Arc::ptr_eq(&first, &publication));
            let leader = leader.unwrap();
            assert!(leader.ptr_eq(&admission.publication.as_ref().unwrap().1));
            let receipt = publication.release_scope.accepted_driver();
            let task = tokio::spawn(async move {
                leader.await?;
                Ok(false)
            });
            drop(retain_driver(
                &mut admission,
                publication,
                false,
                false,
                receipt,
                task,
            ));
            assert_eq!(admission.tasks.len(), 2);
            assert!(Arc::ptr_eq(
                &first,
                &admission.publication.as_ref().unwrap().0
            ));
        }
        drop(observer);
        release.send(()).unwrap();
        join_accepted_stops(&progress).await.unwrap();
        assert!(progress.lock().unwrap().ready().unwrap().is_ok());
    }

    #[test]
    #[timeout("5s")]
    async fn real_driver_panic_cancellation_and_error_retain_distinct_failure_metrics() {
        for failure in ["panic", "cancel", "error"] {
            let metrics = TestMetrics::new();
            let progress: StopProgress = Arc::new(Mutex::new(StopProgressState {
                release_scope: metrics.scope(),
                ..Default::default()
            }));
            let interrupts = Interrupts::default();
            let kind = InterruptKind::Interrupt(Timestamp::from(60_u64));
            assert!(interrupts.lock().await.queue(demand(kind)));
            let task: JoinHandle<Result<bool, WorkerExecutorError>> = match failure {
                "panic" => tokio::spawn(async {
                    panic!("actual stop task panic");
                }),
                "cancel" => tokio::spawn(std::future::pending()),
                _ => tokio::spawn(async {
                    Err(WorkerExecutorError::runtime("actual stop task failure"))
                }),
            };
            if failure == "cancel" {
                task.abort();
            }
            let publication = {
                let mut admission = progress.lock().unwrap();
                let (publication, _) = publication(
                    &admission,
                    &RwLock::new(ExecutionStatus::loading(AgentMode::Durable)),
                );
                let receipt = publication.release_scope.accepted_driver();
                drop(retain_driver(
                    &mut admission,
                    publication.clone(),
                    true,
                    false,
                    receipt,
                    task,
                ));
                publication
            };
            let error = join_accepted_stops(&progress).await.unwrap_err();
            match failure {
                "panic" => assert!(error.to_string().contains("actual stop task panic")),
                "cancel" => assert!(error.to_string().contains("Stop cleanup failed")),
                _ => assert!(error.to_string().contains("actual stop task failure")),
            }
            assert_eq!(
                progress.lock().unwrap().ready().unwrap().unwrap_err(),
                error
            );
            assert_eq!(join_accepted_stops(&progress).await.unwrap_err(), error);
            let reason = match failure {
                "panic" => "panic",
                "cancel" => "observer_lost",
                _ => "stop_driver",
            };
            assert_eq!(
                metrics.value(
                    "golem_agent_resource_cleanup_failures_total",
                    &[("stage", "end_to_end"), ("reason", reason)]
                ),
                1.0
            );
            assert_eq!(
                metrics.value(
                    "golem_agent_resource_release_unfinished",
                    &[
                        ("origin", "accepted_stop"),
                        ("stage", "end_to_end"),
                        ("health", "failed"),
                        ("cause", "pending"),
                    ]
                ),
                1.0
            );
            assert_eq!(
                metrics.count("accepted_stop", "end_to_end", "released_with_failure"),
                0.0,
                "an unfrozen cause is not a completed release interval"
            );
            let mut guard = interrupts.lock().await;
            assert_eq!(
                freeze(&mut guard, &publication, &Mutex::new(None))
                    .unwrap()
                    .kind,
                kind
            );
            drop(guard);
            metrics.assert_finished("accepted_stop", "released_with_failure");
            assert_eq!(join_accepted_stops(&progress).await.unwrap_err(), error);
            assert_eq!(
                progress.lock().unwrap().ready().unwrap().unwrap_err(),
                error
            );
            assert_eq!(
                metrics.value(
                    "golem_agent_resource_cleanup_failures_total",
                    &[("stage", "end_to_end"), ("reason", reason)]
                ),
                1.0,
                "cause freezing must not duplicate the retained failure"
            );
        }
    }
}
