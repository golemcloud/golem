use super::super::attachment::{AttachmentMemory, ToolAttachmentModeMetadata, attachment_pair};
use super::super::{
    FutureToolInvokeGet, ToolExecution, ToolExecutionState, capable_result_await_cohort,
};
use super::*;
use crate::preview2::golem::tool::host::ByteStreamFailure;
use crate::services::active_agents::MemoryGrant;
use golem_common::model::AgentId;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::entity::{
    AgentEntity, EntityActivationPolicy, ExecutableTarget, FilesystemCapability,
    ToolInvocationDescriptor,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::tool::{
    CompiledToolBinding, SecretKeyScope, ToolFilesystemAccess, ToolName, ToolProvisionConfig,
    ToolSource,
};
use golem_common::schema::{SchemaGraph, SchemaType, SchemaValue};
use test_r::{test, timeout};

fn parent() -> OwnerInvocationId {
    OwnerInvocationId::Agent(OplogIndex::from_u64(1))
}

fn activation(filesystem: FilesystemCapability) -> Arc<EntityActivation> {
    let component_id = ComponentId::new();
    let component_revision = ComponentRevision::try_from(1_u64).unwrap();
    let deployment_revision = DeploymentRevision::try_from(1_u64).unwrap();
    let tool_name = ToolName::try_from("search").unwrap();
    Arc::new(
        EntityActivation::new(
            ExecutableTarget::new(component_id, component_revision),
            deployment_revision,
            EntityActivationPolicy::Tool {
                provision: ToolProvisionConfig::default(),
                binding: Box::new(CompiledToolBinding {
                    deployment_revision,
                    release_id: None,
                    metadata_digest: Default::default(),
                    agent_type_name: golem_common::model::agent::AgentTypeName("Agent".to_string()),
                    tool_name,
                    version: "1".to_string(),
                    metadata_version: "1".to_string(),
                    account_id: golem_common::model::account::AccountId::new(),
                    account_email: golem_common::model::account::AccountEmail::new(
                        "owner@example.com",
                    ),
                    parameters: golem_common::model::json::NormalizedJsonValue::new(
                        serde_json::json!({}),
                    ),
                    config_keys_readable: Default::default(),
                    secret_keys_readable: SecretKeyScope::All,
                    secret_keys_revealable: SecretKeyScope::All,
                    filesystem_access: match filesystem {
                        FilesystemCapability::Capable => ToolFilesystemAccess::Allowed,
                        FilesystemCapability::Incapable => ToolFilesystemAccess::Unset,
                    },
                    source: ToolSource::Component {
                        component_id,
                        component_revision,
                        component_name: golem_common::model::component::ComponentName(
                            "tools".to_string(),
                        ),
                    },
                }),
            },
            filesystem,
        )
        .unwrap(),
    )
}

fn context() -> OwnerToolOperationContext {
    context_with_filesystem(FilesystemCapability::Incapable)
}

fn capable_context() -> OwnerToolOperationContext {
    context_with_filesystem(FilesystemCapability::Capable)
}

fn context_with_filesystem(filesystem: FilesystemCapability) -> OwnerToolOperationContext {
    let owner = golem_common::model::OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "Agent(owner)".to_string(),
        },
    );
    context_for(&owner, filesystem, EntityCallMode::Asynchronous)
}

fn context_for(
    owner: &golem_common::model::OwnedAgentId,
    filesystem: FilesystemCapability,
    call_mode: EntityCallMode,
) -> OwnerToolOperationContext {
    OwnerToolOperationContext {
        parent: parent(),
        call_mode,
        activation: activation(filesystem),
        calling_principal: Principal::Agent(golem_common::model::agent::AgentPrincipal {
            agent_id: owner.agent_id.clone(),
        }),
        principal: Principal::Agent(golem_common::model::agent::AgentPrincipal {
            agent_id: owner.agent_id.clone(),
        }),
        descriptor: EntityInvocationDescriptor::Tool(ToolInvocationDescriptor {
            attempt_ordinal: 0,
            command_path: Vec::new(),
            args: Vec::new(),
            has_stdin: false,
            has_stdout: false,
            declares_stdout: false,
        }),
        input: TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::tuple(Vec::new())),
            SchemaValue::Tuple {
                elements: Vec::new(),
            },
        ),
    }
}

fn accept_provisional(
    provisional: ProvisionalOwnerToolOperation,
    start: u64,
) -> OwnerToolOperation {
    let owner = golem_common::model::OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "Agent(owner)".to_string(),
        },
    );
    let invocation_id = EntityInvocationId::new(
        golem_common::model::entity::OwnedAgentEntityId {
            owner,
            entity: provisional.context().activation.entity(),
        },
        OplogIndex::from_u64(start),
    )
    .unwrap();
    provisional.accept(invocation_id).unwrap()
}

fn accepted_operation(
    lane: &OwnerLane,
    call_mode: EntityCallMode,
    start: u64,
) -> (
    Arc<OwnerToolOperations>,
    OwnerToolOperation,
    EntityInvocationId,
) {
    let operations = OwnerToolOperations::new();
    let operation = operations.create(context_for(
        lane.owner_id(),
        FilesystemCapability::Capable,
        call_mode,
    ));
    let invocation_id = EntityInvocationId::new(
        golem_common::model::entity::OwnedAgentEntityId {
            owner: lane.owner_id().clone(),
            entity: AgentEntity::Tool(ToolName::try_from("search").unwrap()),
        },
        OplogIndex::from_u64(start),
    )
    .unwrap();
    let operation = operation.accept(invocation_id.clone()).unwrap();
    (operations, operation, invocation_id)
}

fn tool_execution(
    operation: OwnerToolOperation,
    parent: OwnerInvocationId,
    start: u64,
) -> Arc<ToolExecution> {
    Arc::new(ToolExecution {
        parent,
        start: OplogIndex::from_u64(start),
        filesystem: FilesystemCapability::Capable,
        operation,
        cancellable: true,
        state: Mutex::new(ToolExecutionState {
            result: None,
            failure: None,
        }),
        changed: Notify::new(),
        get_active: AtomicBool::new(false),
        cancel: tokio_util::sync::CancellationToken::new(),
    })
}

#[test]
#[timeout("30s")]
async fn dropping_a_provisional_operation_unregisters_it_and_wakes_parent_waiters() {
    let owner = OwnerToolOperations::new();
    let operation = owner.create(context());
    let waiting_owner = owner.clone();
    let waiting = tokio::spawn(async move { waiting_owner.wait_parent_settled(&parent()).await });
    tokio::task::yield_now().await;

    assert_eq!(owner.operation_count(), 1);
    assert!(!waiting.is_finished());
    drop(operation);
    waiting.await.unwrap();
    assert_eq!(owner.operation_count(), 0);
    assert_eq!(owner.operation_removal_count(), 1);
}

#[test]
#[timeout("30s")]
async fn attachment_admission_rejection_preselects_ordinary_terminal() {
    let owner = OwnerToolOperations::new();
    let operation = accept_provisional(owner.create(capable_context()), 2);
    let (stdout, _consumer, _) = attachment_pair(
        8,
        AttachmentMemory::with_test_reservation(false, |_| async { None }),
    );
    stdout.write(vec![1, 2, 3]).await.unwrap();
    let stdout = stdout.controller();
    assert!(operation.attach(None, Some(stdout.clone())));

    assert_eq!(
        operation.activate_live_attachment_memory_accounting().await,
        ToolLiveAdmissionOutcome::ResourceExhausted
    );
    assert!(matches!(
        operation.winner_if_active(),
        Some(ToolOperationWinner::SelectingOrdinary)
    ));
    assert!(!operation.begin_cancel());

    let failing_owner = owner.clone();
    let owner_failure = tokio::spawn(async move {
        failing_owner
            .select_owner_failure(OwnerFailureWinner::Infrastructure(
                WorkerExecutorError::runtime("concurrent owner failure"),
            ))
            .await
    });
    tokio::task::yield_now().await;
    assert!(!owner_failure.is_finished());

    operation
        .take_live_attachment_admission_rejection()
        .await
        .expect("admission rejection marker");
    operation.complete_rejected_live_attachment_memory_accounting();
    let terminal = Arc::new(SerializableToolOperationTerminal {
        body_execution:
            golem_common::model::oplog::payload::types::SerializableEntityBodyExecution::Skipped,
        result: Err(
            golem_common::model::oplog::payload::types::SerializableToolRpcError::ResourceExhausted(
                "limit".to_string(),
            ),
        ),
    });
    operation.resolve_ordinary(terminal, true).await;
    assert!(owner_failure.await.unwrap());
    assert!(matches!(
        operation.winner_if_active(),
        Some(ToolOperationWinner::Ordinary { .. })
    ));
    assert_eq!(
        stdout.metadata().terminal,
        Some(super::super::attachment::ToolAttachmentTerminalMetadata::ResourceExhausted)
    );
    operation.settle().await;
}

#[test]
#[timeout("30s")]
async fn cancelled_multi_attachment_preparation_rolls_back_and_can_retry() {
    let owner = OwnerToolOperations::new();
    let operation = accept_provisional(owner.create(capable_context()), 2);
    let (stdin, _stdin_consumer, _) = attachment_pair(
        8,
        AttachmentMemory::with_test_reservation(false, |bytes| async move {
            Some(MemoryGrant::inert(bytes))
        }),
    );
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_for_reservation = attempts.clone();
    let (stdout, _stdout_consumer, _) = attachment_pair(
        8,
        AttachmentMemory::with_test_reservation(false, move |bytes| {
            let attempt = attempts_for_reservation.fetch_add(1, Ordering::AcqRel);
            async move {
                if attempt == 0 {
                    std::future::pending().await
                } else {
                    Some(MemoryGrant::inert(bytes))
                }
            }
        }),
    );
    stdin.write(vec![1]).await.unwrap();
    stdout.write(vec![2]).await.unwrap();
    let stdin_controller = stdin.controller();
    let stdout_controller = stdout.controller();
    assert!(operation.attach(
        Some(stdin_controller.clone()),
        Some(stdout_controller.clone())
    ));

    let preparing_operation = operation.clone();
    let preparation = tokio::spawn(async move {
        preparing_operation
            .activate_live_attachment_memory_accounting()
            .await
    });
    while attempts.load(Ordering::Acquire) == 0 {
        tokio::task::yield_now().await;
    }
    preparation.abort();
    assert!(preparation.await.unwrap_err().is_cancelled());

    assert_eq!(
        stdin_controller.live_memory_accounting_state(),
        (false, false)
    );
    assert_eq!(
        stdout_controller.live_memory_accounting_state(),
        (false, false)
    );
    stdin.write(vec![3]).await.unwrap();
    stdout.write(vec![4]).await.unwrap();

    assert_eq!(
        operation.activate_live_attachment_memory_accounting().await,
        ToolLiveAdmissionOutcome::Admitted
    );
    assert_eq!(
        stdin_controller.live_memory_accounting_state(),
        (true, false)
    );
    assert_eq!(
        stdout_controller.live_memory_accounting_state(),
        (true, false)
    );

    assert!(operation.begin_cancel());
    operation.resolve_cancel(true).await;
    operation.settle().await;
}

#[test]
async fn empty_incomplete_activation_does_not_skip_later_capable_attachments() {
    let owner = OwnerToolOperations::new();
    let operation = accept_provisional(owner.create(capable_context()), 2);

    assert_eq!(
        operation.activate_live_attachment_memory_accounting().await,
        ToolLiveAdmissionOutcome::Admitted
    );
    assert_eq!(
        *operation.live_admission.lock().await,
        ToolLiveAdmissionState::Historical
    );

    let (stdout, _consumer, _) = attachment_pair(
        8,
        AttachmentMemory::with_test_reservation(false, |_| async { None }),
    );
    stdout.write(vec![1, 2, 3]).await.unwrap();
    let stdout = stdout.controller();
    assert!(operation.attach(None, Some(stdout)));
    assert_eq!(
        operation.activate_live_attachment_memory_accounting().await,
        ToolLiveAdmissionOutcome::ResourceExhausted
    );
    assert!(matches!(
        operation.winner_if_active(),
        Some(ToolOperationWinner::SelectingOrdinary)
    ));

    operation
        .take_live_attachment_admission_rejection()
        .await
        .expect("admission rejection marker");
    operation.resolve_ordinary(
        Arc::new(SerializableToolOperationTerminal {
            body_execution: golem_common::model::oplog::payload::types::SerializableEntityBodyExecution::Skipped,
            result: Err(
                golem_common::model::oplog::payload::types::SerializableToolRpcError::ResourceExhausted(
                    "limit".to_string(),
                ),
            ),
        }),
        true,
    ).await;
    operation.settle().await;
}

#[test]
#[timeout("30s")]
async fn owner_failure_during_capable_attachment_activation_rolls_back_the_batch() {
    let owner = OwnerToolOperations::new();
    let operation = accept_provisional(owner.create(capable_context()), 2);
    let (stdin, _stdin_consumer, _) = attachment_pair(
        8,
        AttachmentMemory::with_test_reservation(false, |bytes| async move {
            Some(MemoryGrant::inert(bytes))
        }),
    );
    let reservation_entered = Arc::new(Notify::new());
    let reservation_release = Arc::new(Notify::new());
    let (stdout, _stdout_consumer, _) = attachment_pair(
        8,
        AttachmentMemory::with_test_reservation(false, {
            let reservation_entered = reservation_entered.clone();
            let reservation_release = reservation_release.clone();
            move |bytes| {
                let reservation_entered = reservation_entered.clone();
                let reservation_release = reservation_release.clone();
                async move {
                    reservation_entered.notify_one();
                    reservation_release.notified().await;
                    Some(MemoryGrant::inert(bytes))
                }
            }
        }),
    );
    stdin.write(vec![1]).await.unwrap();
    stdout.write(vec![2]).await.unwrap();
    let stdin_controller = stdin.controller();
    let stdout_controller = stdout.controller();
    assert!(operation.attach(
        Some(stdin_controller.clone()),
        Some(stdout_controller.clone())
    ));

    let activation = tokio::spawn({
        let operation = operation.clone();
        async move { operation.activate_live_attachment_memory_accounting().await }
    });
    reservation_entered.notified().await;
    assert!(
        owner
            .select_owner_failure(OwnerFailureWinner::Infrastructure(
                WorkerExecutorError::runtime("owner failed during attachment activation"),
            ))
            .await
    );
    reservation_release.notify_one();

    assert_eq!(activation.await.unwrap(), ToolLiveAdmissionOutcome::Fenced);
    assert_eq!(
        stdin_controller.live_memory_accounting_state(),
        (false, false)
    );
    assert_eq!(
        stdout_controller.live_memory_accounting_state(),
        (false, false)
    );
    assert!(stdin_controller.metadata().owner_fenced);
    assert!(stdout_controller.metadata().owner_fenced);

    owner.drain_owner_failure_lanes().await;
    operation.settle().await;
}

#[test]
#[timeout("30s")]
async fn cancellation_during_capable_attachment_activation_rolls_back_the_batch() {
    let owner = OwnerToolOperations::new();
    let operation = accept_provisional(owner.create(capable_context()), 2);
    let (stdin, _stdin_consumer, _) = attachment_pair(
        8,
        AttachmentMemory::with_test_reservation(false, |bytes| async move {
            Some(MemoryGrant::inert(bytes))
        }),
    );
    let reservation_entered = Arc::new(Notify::new());
    let reservation_release = Arc::new(Notify::new());
    let (stdout, _stdout_consumer, _) = attachment_pair(
        8,
        AttachmentMemory::with_test_reservation(false, {
            let reservation_entered = reservation_entered.clone();
            let reservation_release = reservation_release.clone();
            move |bytes| {
                let reservation_entered = reservation_entered.clone();
                let reservation_release = reservation_release.clone();
                async move {
                    reservation_entered.notify_one();
                    reservation_release.notified().await;
                    Some(MemoryGrant::inert(bytes))
                }
            }
        }),
    );
    stdin.write(vec![1]).await.unwrap();
    stdout.write(vec![2]).await.unwrap();
    let stdin_controller = stdin.controller();
    let stdout_controller = stdout.controller();
    assert!(operation.attach(
        Some(stdin_controller.clone()),
        Some(stdout_controller.clone())
    ));

    let local_live_tail = Arc::new(AtomicBool::new(false));
    let activation = tokio::spawn({
        let operation = operation.clone();
        let local_live_tail = local_live_tail.clone();
        async move {
            crate::durable_host::PendingReplayToLive {
                replay_target: OplogIndex::from_u64(9),
                role: crate::durable_host::replay_state::ReplayToLiveRole::NonPrimary,
                replaying_incomplete_entity: true,
                tool_entity: true,
                tool_operation: Some(operation),
                local_live_tail,
            }
            .finish()
            .await
        }
    });
    reservation_entered.notified().await;
    assert!(operation.begin_cancel());
    reservation_release.notify_one();

    let outcome = activation.await.unwrap().unwrap();
    assert_eq!(outcome, crate::durable_host::FinishReplayToLive::Cancelled);
    assert!(!local_live_tail.load(Ordering::Acquire));
    let live_action_ran = Arc::new(AtomicBool::new(false));
    if outcome.require_live().is_ok() {
        live_action_ran.store(true, Ordering::Release);
    }
    assert!(!live_action_ran.load(Ordering::Acquire));
    assert_eq!(
        stdin_controller.live_memory_accounting_state(),
        (false, false)
    );
    assert_eq!(
        stdout_controller.live_memory_accounting_state(),
        (false, false)
    );
    operation.resolve_cancel(true).await;
    operation.settle().await;
}

#[test]
#[timeout("30s")]
async fn cancellation_during_failed_attachment_activation_rolls_back_the_batch() {
    let owner = OwnerToolOperations::new();
    let operation = accept_provisional(owner.create(capable_context()), 2);
    let (stdin, _stdin_consumer, _) = attachment_pair(
        8,
        AttachmentMemory::with_test_reservation(false, |bytes| async move {
            Some(MemoryGrant::inert(bytes))
        }),
    );
    let reservation_entered = Arc::new(Notify::new());
    let reservation_release = Arc::new(Notify::new());
    let (stdout, _stdout_consumer, _) = attachment_pair(
        8,
        AttachmentMemory::with_test_reservation(false, {
            let reservation_entered = reservation_entered.clone();
            let reservation_release = reservation_release.clone();
            move |_| {
                let reservation_entered = reservation_entered.clone();
                let reservation_release = reservation_release.clone();
                async move {
                    reservation_entered.notify_one();
                    reservation_release.notified().await;
                    None
                }
            }
        }),
    );
    stdin.write(vec![1]).await.unwrap();
    stdout.write(vec![2]).await.unwrap();
    let stdin_controller = stdin.controller();
    let stdout_controller = stdout.controller();
    assert!(operation.attach(
        Some(stdin_controller.clone()),
        Some(stdout_controller.clone())
    ));

    let activation = tokio::spawn({
        let operation = operation.clone();
        async move { operation.activate_live_attachment_memory_accounting().await }
    });
    reservation_entered.notified().await;
    assert!(operation.begin_cancel());
    reservation_release.notify_one();

    assert_eq!(
        activation.await.unwrap(),
        ToolLiveAdmissionOutcome::Cancelled
    );
    assert_eq!(
        stdin_controller.live_memory_accounting_state(),
        (false, false)
    );
    assert_eq!(
        stdout_controller.live_memory_accounting_state(),
        (false, false)
    );
    assert!(owner.owner_winner().is_none());
    operation.resolve_cancel(true).await;
    operation.settle().await;
}

#[test]
fn dropping_an_accepted_observer_does_not_drop_the_owner_operation() {
    let owner = OwnerToolOperations::new();
    let operation = accept_provisional(owner.create(context()), 2);
    let lease = operation.lease.clone();

    assert_eq!(owner.operation_count(), 1);
    drop(operation);
    assert_eq!(owner.operation_count(), 1);
    assert_eq!(lease.handles.load(Ordering::Acquire), 0);
    assert_eq!(owner.operation_removal_count(), 0);
}

#[test]
#[timeout("30s")]
async fn owner_failure_drain_removes_a_handleless_operation_and_wakes_parent_waiters() {
    let owner = OwnerToolOperations::new();
    let operation = accept_provisional(owner.create(context()), 2);
    let lease = operation.lease.clone();
    drop(operation);
    assert_eq!(lease.handles.load(Ordering::Acquire), 0);
    assert_eq!(owner.operation_count(), 1);

    let waiting_owner = owner.clone();
    let waiting = tokio::spawn(async move { waiting_owner.wait_parent_settled(&parent()).await });
    tokio::task::yield_now().await;
    assert!(!waiting.is_finished());

    assert!(
        owner
            .select_owner_failure(OwnerFailureWinner::Infrastructure(
                WorkerExecutorError::runtime("owner failed"),
            ))
            .await
    );
    owner.drain_owner_failure_lanes().await;

    waiting.await.unwrap();
    assert_eq!(owner.operation_count(), 0);
    assert_eq!(owner.operation_removal_count(), 1);
}

#[test]
#[timeout("30s")]
async fn concurrent_final_handle_drops_remove_once_and_wake_parent_waiters() {
    let owner = OwnerToolOperations::new();
    let first = accept_provisional(owner.create(context()), 2);
    let second = first.clone();
    let lease = first.lease.clone();
    assert!(first.begin_cancel());
    first.resolve_cancel(true).await;

    let waiting_owner = owner.clone();
    let waiting = tokio::spawn(async move { waiting_owner.wait_parent_settled(&parent()).await });
    tokio::task::yield_now().await;
    assert!(!waiting.is_finished());

    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let first_barrier = barrier.clone();
    let first_drop = tokio::spawn(async move {
        first_barrier.wait().await;
        drop(first);
    });
    let second_barrier = barrier.clone();
    let second_drop = tokio::spawn(async move {
        second_barrier.wait().await;
        drop(second);
    });
    barrier.wait().await;
    first_drop.await.unwrap();
    second_drop.await.unwrap();

    waiting.await.unwrap();
    assert_eq!(lease.handles.load(Ordering::Acquire), 0);
    assert_eq!(owner.operation_count(), 0);
    assert_eq!(owner.operation_removal_count(), 1);
}

#[test]
async fn metadata_exposes_mode_backpressure_and_sanitized_owner_failure() {
    let owner = OwnerToolOperations::new();
    let operation = accept_provisional(owner.create(context()), 2);
    let (stdout, _reader, _observer) = attachment_pair(4, AttachmentMemory::inert());
    assert!(operation.attach(None, Some(stdout.controller())));
    assert!(stdout.configure_live());
    stdout.write(vec![1, 2, 3, 4]).await.unwrap();

    let metadata = owner.metadata();
    let operation_metadata = &metadata.operations[0];
    assert_eq!(operation_metadata.call_mode, EntityCallMode::Asynchronous);
    assert_eq!(
        operation_metadata.filesystem,
        FilesystemCapability::Incapable
    );
    let stdout_metadata = operation_metadata.stdout.as_ref().unwrap();
    assert_eq!(stdout_metadata.capacity_bytes, 4);
    assert_eq!(stdout_metadata.buffered_bytes, 4);
    assert_eq!(stdout_metadata.charged_bytes, 4);
    assert!(stdout_metadata.backpressured);

    assert!(
        owner
            .select_owner_failure(OwnerFailureWinner::Infrastructure(
                WorkerExecutorError::runtime("private infrastructure detail"),
            ))
            .await
    );
    let metadata = owner.metadata();
    assert_eq!(
        metadata.owner_failure,
        Some(ToolOwnerFailureMetadata::Infrastructure)
    );
    let stdout_metadata = metadata.operations[0].stdout.as_ref().unwrap();
    assert_eq!(
        stdout_metadata.terminal,
        Some(super::super::attachment::ToolAttachmentTerminalMetadata::Cancelled)
    );
    assert!(stdout_metadata.owner_fenced);
    assert!(!stdout_metadata.backpressured);

    owner.drain_owner_failure_lanes().await;
    operation.settle().await;
}

#[test]
async fn attaching_to_a_fenced_operation_fences_and_clears_local_attachments() {
    let owner = OwnerToolOperations::new();
    let operation = accept_provisional(owner.create(context()), 2);
    let (stdout, consumer, _observer) = attachment_pair(4, AttachmentMemory::inert());
    let controller = consumer.controller();
    stdout.write(vec![1, 2, 3, 4]).await.unwrap();

    assert!(
        owner
            .select_owner_failure(OwnerFailureWinner::Infrastructure(
                WorkerExecutorError::runtime("owner failed before attachment"),
            ))
            .await
    );
    assert!(!operation.attach(None, Some(controller.clone())));

    let metadata = controller.metadata();
    assert!(metadata.owner_fenced);
    assert_eq!(metadata.mode, ToolAttachmentModeMetadata::Pending);
    assert_eq!(metadata.buffered_bytes, 0);
    assert_eq!(metadata.charged_bytes, 0);

    owner.drain_owner_failure_lanes().await;
    operation.settle().await;
}

#[test]
async fn accepted_operation_gates_parent_settlement_until_owner_settlement() {
    let owner = OwnerToolOperations::new();
    let provisional = owner.create(context());
    let operation = accept_provisional(provisional, 2);
    let waiting_owner = owner.clone();
    let waiting = tokio::spawn(async move { waiting_owner.wait_parent_settled(&parent()).await });
    tokio::task::yield_now().await;
    assert!(!waiting.is_finished());
    assert!(
        owner
            .select_owner_failure(OwnerFailureWinner::Infrastructure(
                WorkerExecutorError::runtime("owner failed"),
            ))
            .await
    );
    owner.drain_owner_failure_lanes().await;
    assert!(!waiting.is_finished());
    operation.settle().await;

    waiting.await.unwrap();
    assert_eq!(owner.operation_count(), 0);
    assert_eq!(owner.operation_removal_count(), 1);
}

#[test]
#[timeout("30s")]
async fn owner_failure_becomes_interruptible_only_after_operation_cleanup() {
    let owner = OwnerToolOperations::new();
    let operation = accept_provisional(owner.create(context()), 2);
    let sibling = accept_provisional(owner.create(context()), 3);
    let waiting_owner = owner.clone();
    let waiting = tokio::spawn(async move { waiting_owner.wait_parent_settled(&parent()).await });
    tokio::task::yield_now().await;
    assert!(!waiting.is_finished());

    assert!(
        operation
            .select_infrastructure(WorkerExecutorError::runtime("owner failed"))
            .await
    );
    let cleanup = operation
        .claim_owner_failure_cleanup()
        .expect("the selecting operation owns cleanup");
    assert!(matches!(
        sibling.winner_if_active(),
        Some(ToolOperationWinner::FencedByOwner)
    ));
    owner.drain_owner_failure_lanes().await;
    assert_eq!(owner.operation_count(), 2);
    assert!(owner.interruptible_owner_failure().is_none());

    operation.settle().await;
    sibling.settle().await;
    owner.wait_owner_settled().await;
    owner.complete_owner_failure_cleanup(cleanup);

    waiting.await.unwrap();
    assert_eq!(owner.operation_count(), 0);
    assert_eq!(owner.operation_removal_count(), 2);
    assert!(matches!(
        owner.interruptible_owner_failure(),
        Some(OwnerFailureWinner::Infrastructure(_))
    ));
}

#[test]
#[timeout("30s")]
async fn owner_failure_seals_operation_creation_through_cleanup_completion() {
    let owner = OwnerToolOperations::new();
    let operation = accept_provisional(owner.create(context()), 2);
    assert!(
        operation
            .select_infrastructure(WorkerExecutorError::runtime("owner failed"))
            .await
    );
    let cleanup = operation
        .claim_owner_failure_cleanup()
        .expect("the selecting operation owns cleanup");

    let during_cleanup = owner.create(context());
    assert!(during_cleanup.is_rejected());
    assert_eq!(owner.operation_count(), 1);

    operation.settle().await;
    owner.wait_owner_settled().await;
    owner.complete_owner_failure_cleanup(cleanup);

    let after_cleanup = owner.create(context());
    assert!(after_cleanup.is_rejected());
    assert_eq!(owner.operation_count(), 0);
    assert!(matches!(
        owner.interruptible_owner_failure(),
        Some(OwnerFailureWinner::Infrastructure(_))
    ));
}

#[test]
#[timeout("30s")]
async fn settlement_wakes_parent_waiters_before_cancellable_lane_drain() {
    let owner = OwnerToolOperations::new();
    let operation = accept_provisional(owner.create(context()), 2);
    let acquisition = tokio::spawn(std::future::pending::<()>());
    {
        let mut state = owner.state.lock().unwrap();
        state.operations.get_mut(&operation.id).unwrap().lane =
            LaneOwnership::Acquiring(AcquisitionControl {
                abort: acquisition.abort_handle(),
                drained: Arc::new(AcquisitionDrain::default()),
            });
    }
    assert!(
        owner
            .select_owner_failure(OwnerFailureWinner::Infrastructure(
                WorkerExecutorError::runtime("owner failed"),
            ))
            .await
    );

    let waiting_owner = owner.clone();
    let waiting = tokio::spawn(async move { waiting_owner.wait_parent_settled(&parent()).await });
    tokio::task::yield_now().await;
    assert!(!waiting.is_finished());

    let settlement = tokio::spawn(operation.settle());
    while owner.operation_count() != 0 {
        tokio::task::yield_now().await;
    }
    assert!(!settlement.is_finished());
    tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
        .await
        .unwrap()
        .unwrap();

    settlement.abort();
    assert!(settlement.await.unwrap_err().is_cancelled());
    assert!(acquisition.await.unwrap_err().is_cancelled());
    assert_eq!(owner.operation_removal_count(), 1);
}

#[test]
async fn owner_failure_waits_for_non_interleavable_terminal_selection() {
    let owner = OwnerToolOperations::new();
    let operation = accept_provisional(owner.create(context()), 2);
    assert!(operation.begin_cancel());

    let selecting_owner = owner.clone();
    let failure = tokio::spawn(async move {
        selecting_owner
            .select_owner_failure(OwnerFailureWinner::Infrastructure(
                WorkerExecutorError::runtime("owner failed"),
            ))
            .await
    });
    tokio::task::yield_now().await;
    assert!(!failure.is_finished());

    operation.resolve_cancel(true).await;
    assert!(failure.await.unwrap());
    assert!(matches!(
        operation.winner_if_active(),
        Some(ToolOperationWinner::Cancelled)
    ));
    assert!(matches!(
        owner.owner_winner(),
        Some(OwnerFailureWinner::Infrastructure(_))
    ));
}

#[test]
async fn explicit_cancel_preserves_owner_interrupt_and_fences_streams_before_return() {
    let owner = OwnerToolOperations::new();
    let cancelled = accept_provisional(owner.create(context()), 2);
    let sibling = accept_provisional(owner.create(context()), 3);
    let (_cancelled_producer, cancelled_consumer, cancelled_observer) =
        attachment_pair(16, AttachmentMemory::inert());
    let (_sibling_producer, sibling_consumer, sibling_observer) =
        attachment_pair(16, AttachmentMemory::inert());
    assert!(cancelled.attach(None, Some(cancelled_consumer.controller())));
    assert!(sibling.attach(None, Some(sibling_consumer.controller())));

    assert!(cancelled.begin_cancel());
    let selecting_owner = owner.clone();
    let lifecycle = tokio::spawn(async move {
        selecting_owner
            .select_owner_failure(OwnerFailureWinner::Lifecycle(InterruptKind::Jump))
            .await
    });
    tokio::task::yield_now().await;
    assert!(!lifecycle.is_finished());

    cancelled.resolve_cancel(true).await;
    assert!(lifecycle.await.unwrap());

    assert!(matches!(
        cancelled_observer.wait_terminal().await,
        crate::preview2::golem::tool::host::ByteStreamCloseCause::Failed(
            ByteStreamFailure::Cancelled
        )
    ));
    assert!(matches!(
        sibling_observer.wait_terminal().await,
        crate::preview2::golem::tool::host::ByteStreamCloseCause::Failed(
            ByteStreamFailure::Cancelled
        )
    ));
    assert!(matches!(
        cancelled.winner_if_active(),
        Some(ToolOperationWinner::Cancelled)
    ));
    assert!(matches!(
        sibling.winner_if_active(),
        Some(ToolOperationWinner::FencedByOwner)
    ));
    assert!(matches!(
        owner.owner_winner(),
        Some(OwnerFailureWinner::Lifecycle(InterruptKind::Jump))
    ));
}

#[test]
async fn lifecycle_winner_forces_a_losing_guest_trap_stdout_to_cancelled() {
    let owner = OwnerToolOperations::new();
    let operation = accept_provisional(owner.create(context()), 2);
    let (_producer, consumer, observer) = attachment_pair(16, AttachmentMemory::inert());
    let stdout = consumer.controller();
    assert!(operation.attach(None, Some(stdout.clone())));

    assert!(
        owner
            .select_owner_failure(OwnerFailureWinner::Lifecycle(InterruptKind::Restart))
            .await
    );
    let failure =
        super::super::guest_trap_stdout_failure(&operation, TrapType::Exit, false, false).await;
    assert!(matches!(failure, ByteStreamFailure::Cancelled));
    let _ = stdout.host_fail(failure);
    assert!(matches!(
        owner.owner_winner(),
        Some(OwnerFailureWinner::Lifecycle(InterruptKind::Restart))
    ));
    assert!(matches!(
        operation.winner_if_active(),
        Some(ToolOperationWinner::FencedByOwner)
    ));
    assert!(matches!(
        observer.wait_terminal().await,
        crate::preview2::golem::tool::host::ByteStreamCloseCause::Failed(
            ByteStreamFailure::Cancelled
        )
    ));
}

#[test]
async fn guest_trap_winner_rejects_a_later_lifecycle_failure() {
    let owner = OwnerToolOperations::new();
    let operation = accept_provisional(owner.create(context()), 2);

    assert!(operation.select_trap(TrapType::Exit).await);
    assert!(
        !owner
            .select_owner_failure(OwnerFailureWinner::Lifecycle(InterruptKind::Jump))
            .await
    );
    assert!(matches!(
        owner.owner_winner(),
        Some(OwnerFailureWinner::Trap(TrapType::Exit))
    ));
    assert!(matches!(
        operation.winner_if_active(),
        Some(ToolOperationWinner::Trap)
    ));
}

#[test]
async fn cancel_trap_and_lifecycle_race_selects_one_owner_failure() {
    let owner = OwnerToolOperations::new();
    let cancelled = accept_provisional(owner.create(context()), 2);
    let trapped = accept_provisional(owner.create(context()), 3);
    assert!(cancelled.begin_cancel());

    let trapping = tokio::spawn({
        let trapped = trapped.clone();
        async move { trapped.select_trap(TrapType::Exit).await }
    });
    let lifecycle = tokio::spawn({
        let owner = owner.clone();
        async move {
            owner
                .select_owner_failure(OwnerFailureWinner::Lifecycle(InterruptKind::Restart))
                .await
        }
    });
    tokio::task::yield_now().await;
    assert!(!trapping.is_finished());
    assert!(!lifecycle.is_finished());

    cancelled.resolve_cancel(true).await;
    let trap_selected = trapping.await.unwrap();
    let lifecycle_selected = lifecycle.await.unwrap();
    assert_ne!(trap_selected, lifecycle_selected);
    assert!(matches!(
        cancelled.winner_if_active(),
        Some(ToolOperationWinner::Cancelled)
    ));
    match owner.owner_winner() {
        Some(OwnerFailureWinner::Trap(TrapType::Exit)) => assert!(trap_selected),
        Some(OwnerFailureWinner::Lifecycle(InterruptKind::Restart)) => {
            assert!(lifecycle_selected)
        }
        other => panic!("three-way race selected an unexpected owner winner: {other:?}"),
    }
}

#[test]
async fn guest_trap_atomically_fences_siblings_and_preserves_exact_owner_winner() {
    let owner = OwnerToolOperations::new();
    let trapped = accept_provisional(owner.create(context()), 2);
    let sibling = accept_provisional(owner.create(context()), 3);
    let (trapped_stdin, _consumer, _) = attachment_pair(16, AttachmentMemory::inert());
    let (trapped_stdout, _consumer, _) = attachment_pair(16, AttachmentMemory::inert());
    let (sibling_stdin, _consumer, _) = attachment_pair(16, AttachmentMemory::inert());
    let (sibling_stdout, _consumer, _) = attachment_pair(16, AttachmentMemory::inert());
    let trapped_stdin = trapped_stdin.controller();
    let trapped_stdout = trapped_stdout.controller();
    let sibling_stdin = sibling_stdin.controller();
    let sibling_stdout = sibling_stdout.controller();
    assert!(trapped.attach(Some(trapped_stdin.clone()), Some(trapped_stdout.clone())));
    assert!(sibling.attach(Some(sibling_stdin.clone()), Some(sibling_stdout.clone())));
    let trap = TrapType::Exit;

    assert!(trapped.select_trap(trap.clone()).await);
    assert!(matches!(
        trapped.winner_if_active(),
        Some(ToolOperationWinner::Trap)
    ));
    assert!(matches!(
        sibling.winner_if_active(),
        Some(ToolOperationWinner::FencedByOwner)
    ));
    assert!(matches!(
        owner.owner_winner(),
        Some(OwnerFailureWinner::Trap(TrapType::Exit))
    ));
    for attachment in [trapped_stdin, trapped_stdout, sibling_stdin, sibling_stdout] {
        assert!(attachment.metadata().owner_fenced);
    }
}

#[test]
async fn guest_trap_drains_a_sibling_blocked_on_lane_acquisition() {
    let owner_id = golem_common::model::OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "Agent(owner)".to_string(),
        },
    );
    let lane = OwnerLane::new(owner_id.clone());
    let primary = lane
        .enter_primary(OplogIndex::from_u64(1))
        .unwrap()
        .acquire()
        .await
        .unwrap();
    let operations = OwnerToolOperations::new();
    let accept = |start| {
        let provisional = operations.create(context_for(
            &owner_id,
            FilesystemCapability::Capable,
            EntityCallMode::Asynchronous,
        ));
        let invocation_id = EntityInvocationId::new(
            golem_common::model::entity::OwnedAgentEntityId {
                owner: owner_id.clone(),
                entity: provisional.context().activation.entity(),
            },
            OplogIndex::from_u64(start),
        )
        .unwrap();
        provisional.accept(invocation_id).unwrap()
    };
    let trapped = accept(2);
    let sibling = accept(3);
    assert!(sibling.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready));
    assert!(sibling.register_body(&lane).unwrap());
    let acquiring_sibling = sibling.clone();
    let acquiring = tokio::spawn(async move { acquiring_sibling.acquire_registered_body().await });
    while sibling.is_acquiring_lane_if_active() != Some(true) {
        tokio::task::yield_now().await;
    }

    assert!(trapped.select_trap(TrapType::Exit).await);
    operations.drain_owner_failure_lanes().await;

    assert!(!acquiring.await.unwrap().unwrap());
    assert_eq!(lane.holder(), Some(parent()));
    trapped.settle().await;
    sibling.settle().await;
    primary.complete();
}

#[test]
async fn later_trap_preserves_committed_sibling_terminals() {
    let owner = OwnerToolOperations::new();
    let trapped = accept_provisional(owner.create(context()), 2);
    let ordinary = accept_provisional(owner.create(context()), 3);
    let cancelled = accept_provisional(owner.create(context()), 4);
    let terminal = Arc::new(SerializableToolOperationTerminal {
        body_execution:
            golem_common::model::oplog::payload::types::SerializableEntityBodyExecution::Skipped,
        result: Err(
            golem_common::model::oplog::payload::types::SerializableToolRpcError::ResourceExhausted(
                "limit".to_string(),
            ),
        ),
    });

    assert!(ordinary.begin_ordinary());
    ordinary.resolve_ordinary(terminal.clone(), true).await;
    assert!(cancelled.begin_cancel());
    cancelled.resolve_cancel(true).await;

    assert!(trapped.select_trap(TrapType::Exit).await);
    assert!(matches!(
        trapped.winner_if_active(),
        Some(ToolOperationWinner::Trap)
    ));
    assert!(matches!(
        ordinary.winner_if_active(),
        Some(ToolOperationWinner::Ordinary {
            _terminal: recorded
        }) if recorded == terminal
    ));
    assert!(matches!(
        cancelled.winner_if_active(),
        Some(ToolOperationWinner::Cancelled)
    ));
}

#[test]
async fn cancellation_before_registration_owns_no_lane_value() {
    let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "Agent(owner)".to_string(),
        },
    ));
    let (_operations, operation, _) = accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
    assert!(operation.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready));
    assert_eq!(operation.owns_lane_value_if_active(), Some(false));

    assert!(operation.begin_cancel());
    operation.resolve_cancel(true).await;
    assert_eq!(operation.owns_lane_value_if_active(), Some(false));
    assert!(matches!(
        operation.winner_if_active(),
        Some(ToolOperationWinner::Cancelled)
    ));
}

#[test]
async fn cancellation_drops_a_queued_lane_ticket_without_starting_the_body() {
    let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "Agent(owner)".to_string(),
        },
    ));
    let primary = lane
        .enter_primary(OplogIndex::from_u64(1))
        .unwrap()
        .acquire()
        .await
        .unwrap();
    let (_operations, operation, invocation_id) =
        accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
    assert!(operation.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready));
    assert!(operation.register_body(&lane).unwrap());
    assert_eq!(operation.owns_lane_value_if_active(), Some(true));

    assert!(operation.begin_cancel());
    operation.resolve_cancel(true).await;
    assert_eq!(operation.owns_lane_value_if_active(), Some(false));
    assert_eq!(lane.holder(), Some(parent()));
    assert!(
        lane.await_invocations(&parent(), [OwnerInvocationId::Entity(invocation_id)])
            .is_err()
    );
    primary.complete();
}

#[test]
async fn cancellation_after_grant_releases_permit_before_the_first_guest_poll() {
    let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "Agent(owner)".to_string(),
        },
    ));
    let primary = lane
        .enter_primary(OplogIndex::from_u64(1))
        .unwrap()
        .acquire()
        .await
        .unwrap();
    let (_operations, operation, invocation_id) =
        accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
    assert!(operation.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready));
    assert!(operation.register_body(&lane).unwrap());
    let wait = lane
        .await_invocations(
            &parent(),
            [OwnerInvocationId::Entity(invocation_id.clone())],
        )
        .unwrap();
    assert!(operation.acquire_registered_body().await.unwrap());
    assert_eq!(operation.owns_lane_value_if_active(), Some(true));
    assert_eq!(
        lane.holder(),
        Some(OwnerInvocationId::Entity(invocation_id))
    );

    assert!(operation.begin_cancel());
    operation.resolve_cancel(true).await;
    assert_eq!(operation.owns_lane_value_if_active(), Some(false));
    wait.wait().await;
    assert_eq!(lane.holder(), Some(parent()));
    primary.complete();
}

#[test]
async fn capable_terminal_commit_returns_lane_before_completion_publication() {
    let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "Agent(owner)".to_string(),
        },
    ));
    let primary = lane
        .enter_primary(OplogIndex::from_u64(1))
        .unwrap()
        .acquire()
        .await
        .unwrap();
    let (_operations, operation, invocation_id) =
        accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
    let (stdout, _reader, _observer) = attachment_pair(16, AttachmentMemory::inert());
    let stdout_controller = stdout.controller();
    assert!(operation.attach(None, Some(stdout_controller.clone())));
    assert!(stdout.configure_completion());
    stdout.write(b"staged".to_vec()).await.unwrap();
    stdout.finish().unwrap();

    assert!(operation.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready));
    assert!(operation.register_body(&lane).unwrap());
    let wait = lane
        .await_invocations(
            &parent(),
            [OwnerInvocationId::Entity(invocation_id.clone())],
        )
        .unwrap();
    assert!(operation.acquire_registered_body().await.unwrap());
    assert_eq!(
        lane.holder(),
        Some(OwnerInvocationId::Entity(invocation_id))
    );
    assert_eq!(
        stdout_controller.metadata().mode,
        ToolAttachmentModeMetadata::CompletionStaged
    );
    assert_eq!(stdout_controller.metadata().delivered_bytes, 0);

    let terminal = Arc::new(SerializableToolOperationTerminal {
        body_execution:
            golem_common::model::oplog::payload::types::SerializableEntityBodyExecution::Skipped,
        result: Err(
            golem_common::model::oplog::payload::types::SerializableToolRpcError::ResourceExhausted(
                "limit".to_string(),
            ),
        ),
    });
    assert!(operation.begin_ordinary());
    operation.resolve_ordinary(terminal, true).await;
    wait.wait().await;

    assert!(matches!(
        operation.winner_if_active(),
        Some(ToolOperationWinner::Ordinary { .. })
    ));
    assert_eq!(lane.holder(), Some(parent()));
    assert_eq!(
        stdout_controller.metadata().mode,
        ToolAttachmentModeMetadata::CompletionStaged
    );
    assert_eq!(stdout_controller.metadata().delivered_bytes, 0);

    operation.settle().await;
    assert!(stdout_controller.publish_completion());
    assert_eq!(
        stdout_controller.metadata().mode,
        ToolAttachmentModeMetadata::CompletionPublished
    );
    primary.complete();
}

#[test]
async fn owner_fence_keeps_granted_lane_until_sidecar_drain() {
    let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "Agent(owner)".to_string(),
        },
    ));
    let primary = lane
        .enter_primary(OplogIndex::from_u64(1))
        .unwrap()
        .acquire()
        .await
        .unwrap();
    let (operations, operation, invocation_id) =
        accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
    assert!(operation.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready));
    assert!(operation.register_body(&lane).unwrap());
    let wait = lane
        .await_invocations(
            &parent(),
            [OwnerInvocationId::Entity(invocation_id.clone())],
        )
        .unwrap();
    assert!(operation.acquire_registered_body().await.unwrap());

    assert!(
        operations
            .select_owner_failure(OwnerFailureWinner::Infrastructure(
                WorkerExecutorError::runtime("owner failed"),
            ))
            .await
    );
    assert_eq!(operation.owns_lane_value_if_active(), Some(true));
    assert_eq!(
        lane.holder(),
        Some(OwnerInvocationId::Entity(invocation_id))
    );

    operations.drain_owner_failure_lanes().await;
    assert_eq!(operation.owns_lane_value_if_active(), Some(false));
    wait.wait().await;
    assert_eq!(lane.holder(), Some(parent()));
    drop(operation);
    assert_eq!(operations.operation_count(), 0);
    primary.complete();
}

#[test]
async fn cancellation_drains_a_blocked_in_flight_lane_acquisition() {
    let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "Agent(owner)".to_string(),
        },
    ));
    let primary = lane
        .enter_primary(OplogIndex::from_u64(1))
        .unwrap()
        .acquire()
        .await
        .unwrap();
    let (_operations, operation, invocation_id) =
        accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
    assert!(operation.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready));
    assert!(operation.register_body(&lane).unwrap());

    let acquiring_operation = operation.clone();
    let acquiring =
        tokio::spawn(async move { acquiring_operation.acquire_registered_body().await });
    while operation.is_acquiring_lane_if_active() != Some(true) {
        tokio::task::yield_now().await;
    }

    assert!(operation.begin_cancel());
    operation.resolve_cancel(true).await;
    assert!(!acquiring.await.unwrap().unwrap());
    assert_eq!(operation.owns_lane_value_if_active(), Some(false));
    assert!(
        lane.await_invocations(&parent(), [OwnerInvocationId::Entity(invocation_id)])
            .is_err()
    );
    assert_eq!(lane.holder(), Some(parent()));
    primary.complete();
}

#[test]
async fn rolled_back_cancellation_preserves_a_permit_granted_during_selection() {
    let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "Agent(owner)".to_string(),
        },
    ));
    let primary = lane
        .enter_primary(OplogIndex::from_u64(1))
        .unwrap()
        .acquire()
        .await
        .unwrap();
    let (_operations, operation, invocation_id) =
        accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
    assert!(operation.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready));
    assert!(operation.register_body(&lane).unwrap());

    let acquiring_operation = operation.clone();
    let acquiring =
        tokio::spawn(async move { acquiring_operation.acquire_registered_body().await });
    while operation.is_acquiring_lane_if_active() != Some(true) {
        tokio::task::yield_now().await;
    }
    assert!(operation.begin_cancel());
    let wait = lane
        .await_invocations(
            &parent(),
            [OwnerInvocationId::Entity(invocation_id.clone())],
        )
        .unwrap();
    tokio::task::yield_now().await;
    assert!(!acquiring.is_finished());

    operation.resolve_cancel(false).await;
    assert!(acquiring.await.unwrap().unwrap());
    assert_eq!(operation.owns_lane_value_if_active(), Some(true));
    assert_eq!(
        operation.admission_if_active(),
        Some(BodyAdmissionState::Running)
    );
    assert_eq!(
        lane.holder(),
        Some(OwnerInvocationId::Entity(invocation_id))
    );

    assert!(operation.begin_cancel());
    operation.resolve_cancel(true).await;
    wait.wait().await;
    assert_eq!(lane.holder(), Some(parent()));
    primary.complete();
}

#[test]
async fn cancellation_selection_is_idempotent_until_its_terminal_commits() {
    let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "Agent(owner)".to_string(),
        },
    ));
    let (_operations, operation, _invocation_id) =
        accepted_operation(&lane, EntityCallMode::Asynchronous, 2);

    assert!(operation.begin_cancel());
    assert!(operation.begin_cancel());
    operation.resolve_cancel(true).await;
    assert!(!operation.begin_cancel());
    operation.clone().settle().await;
    assert!(!operation.begin_cancel());
}

#[test]
async fn cancellation_selection_closes_attached_streams() {
    let owner = OwnerToolOperations::new();
    let operation = accept_provisional(owner.create(context()), 2);
    let (_producer, consumer, observer) = attachment_pair(16, AttachmentMemory::inert());
    assert!(operation.attach(None, Some(consumer.controller())));

    assert!(operation.begin_cancel());

    assert!(matches!(
        observer.wait_terminal().await,
        crate::preview2::golem::tool::host::ByteStreamCloseCause::Failed(
            ByteStreamFailure::Cancelled
        )
    ));
    operation.resolve_cancel(true).await;
}

#[test]
async fn local_cancellation_interruption_does_not_enter_trap_election() {
    let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "Agent(owner)".to_string(),
        },
    ));
    let (operations, operation, _invocation_id) =
        accepted_operation(&lane, EntityCallMode::Asynchronous, 2);

    assert!(operation.claim_local_cancellation_interruption());
    operation.resolve_cancel(true).await;

    assert!(operations.owner_winner().is_none());
    assert!(matches!(
        operation.winner_if_active(),
        Some(ToolOperationWinner::Cancelled)
    ));
}

#[test]
async fn retained_clone_has_only_optional_state_after_settlement() {
    let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "Agent(owner)".to_string(),
        },
    ));
    let (_operations, operation, _invocation_id) =
        accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
    let retained = operation.clone();
    assert!(operation.begin_cancel());
    operation.resolve_cancel(true).await;
    operation.settle().await;

    assert_eq!(retained.invocation_id(), None);
    assert_eq!(retained.admission_if_active(), None);
    assert!(retained.winner_if_active().is_none());
    assert!(!retained.cancellation_selected_if_active());
    assert_eq!(retained.owns_lane_value_if_active(), None);
    assert_eq!(retained.is_acquiring_lane_if_active(), None);
    let _ = retained.context();
    let _ = retained.subscribe();
}

#[test]
fn completed_future_observation_is_repeatable_without_an_active_cohort() {
    let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "Agent(owner)".to_string(),
        },
    ));
    let (_operations, operation, _invocation_id) =
        accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
    let execution = tool_execution(operation, parent(), 2);
    execution.complete(Ok(Err(
        golem_common::model::oplog::payload::types::SerializableToolRpcError::Cancelled,
    )));

    for _ in 0..2 {
        assert!(matches!(
            execution.get_plan(),
            FutureToolInvokeGet::Ready(response)
                if matches!(
                    *response,
                    Err(golem_common::model::oplog::payload::types::SerializableToolRpcError::Cancelled)
                )
        ));
    }
    assert!(capable_result_await_cohort(&[execution.get_plan()], &parent()).is_none());
}

#[test]
fn mixed_completed_and_current_futures_only_cohort_the_active_parent() {
    let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "Agent(owner)".to_string(),
        },
    ));
    let (_old_operations, old_operation, _old_invocation_id) =
        accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
    let (_current_operations, current_operation, _current_invocation_id) =
        accepted_operation(&lane, EntityCallMode::Asynchronous, 3);
    let old_parent = OwnerInvocationId::Agent(OplogIndex::from_u64(1));
    let current_parent = OwnerInvocationId::Agent(OplogIndex::from_u64(10));
    let old = tool_execution(old_operation, old_parent, 2);
    let current = tool_execution(current_operation, current_parent.clone(), 3);
    old.complete(Ok(Err(
        golem_common::model::oplog::payload::types::SerializableToolRpcError::Cancelled,
    )));

    let cohort =
        capable_result_await_cohort(&[old.get_plan(), current.get_plan()], &current_parent)
            .unwrap();
    assert_eq!(cohort, vec![OplogIndex::from_u64(3)]);
}

#[test]
async fn result_await_batch_promotes_before_parent_end_in_start_order() {
    let owner_id = golem_common::model::OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "Agent(owner)".to_string(),
        },
    );
    let lane = OwnerLane::new(owner_id.clone());
    let primary = lane
        .enter_primary(OplogIndex::from_u64(1))
        .unwrap()
        .acquire()
        .await
        .unwrap();
    let operations = OwnerToolOperations::new();
    let accept = |start| {
        let provisional = operations.create(context_for(
            &owner_id,
            FilesystemCapability::Capable,
            EntityCallMode::Asynchronous,
        ));
        let invocation_id = EntityInvocationId::new(
            golem_common::model::entity::OwnedAgentEntityId {
                owner: owner_id.clone(),
                entity: provisional.context().activation.entity(),
            },
            OplogIndex::from_u64(start),
        )
        .unwrap();
        (
            provisional.accept(invocation_id.clone()).unwrap(),
            invocation_id,
        )
    };
    let (earlier, earlier_id) = accept(2);
    let (later, later_id) = accept(3);
    assert!(later.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready));
    assert!(earlier.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready));

    let registration_operation = earlier.clone();
    let registration_waiter = registration_operation.wait_until_registered();
    tokio::pin!(registration_waiter);
    assert!(futures::poll!(registration_waiter.as_mut()).is_pending());
    let (registered, wait) = operations
        .register_ready_bodies(
            &lane,
            &[OplogIndex::from_u64(3), OplogIndex::from_u64(2)],
            Some(&parent()),
        )
        .unwrap();
    assert_eq!(
        futures::poll!(registration_waiter.as_mut()),
        std::task::Poll::Ready(true)
    );
    assert_eq!(
        registered,
        vec![
            OwnerInvocationId::Entity(earlier_id.clone()),
            OwnerInvocationId::Entity(later_id.clone()),
        ]
    );
    let wait = wait.expect("result-await registration must return its causal barrier");

    let earlier_for_acquire = earlier.clone();
    let earlier_acquire =
        tokio::spawn(async move { earlier_for_acquire.acquire_registered_body().await });
    let later_for_acquire = later.clone();
    let later_acquire =
        tokio::spawn(async move { later_for_acquire.acquire_registered_body().await });
    assert!(earlier_acquire.await.unwrap().unwrap());
    assert_eq!(lane.holder(), Some(OwnerInvocationId::Entity(earlier_id)));
    assert!(!later_acquire.is_finished());

    assert!(earlier.begin_cancel());
    earlier.resolve_cancel(true).await;
    assert!(later_acquire.await.unwrap().unwrap());
    assert_eq!(lane.holder(), Some(OwnerInvocationId::Entity(later_id)));
    assert!(later.begin_cancel());
    later.resolve_cancel(true).await;
    wait.wait().await;
    assert_eq!(lane.holder(), Some(parent()));

    earlier.settle().await;
    later.settle().await;
    primary.complete();
}

#[test]
fn generation_reset_clears_closed_parents_but_rejects_staged_admissions() {
    let table = DeferredAdmissionTable::default();
    let closed = parent();
    assert_eq!(table.close_parent_and_snapshot(&closed), Some(Vec::new()));
    table
        .begin_generation()
        .expect("closed parents from the fenced generation can be cleared");
    assert!(!table.closed_parents.lock().unwrap().contains(&closed));

    let staged_parent = parent();
    assert!(table.insert(staged_parent, OplogIndex::from_u64(2)));
    assert!(table.begin_generation().is_err());
}

#[test]
fn deferred_cohort_waits_for_earlier_eligible_staging_and_releases_in_start_order() {
    let table = DeferredAdmissionTable::default();
    let parent = parent();
    let first = OplogIndex::from_u64(2);
    let second = OplogIndex::from_u64(3);
    let third = OplogIndex::from_u64(4);
    assert!(table.insert(parent.clone(), first));
    assert!(table.insert(parent.clone(), second));
    assert!(table.insert(parent.clone(), third));
    assert!(table.settle_staging(&parent, second, DeferredAdmissionReadiness::Ready));
    assert!(table.settle_staging(&parent, third, DeferredAdmissionReadiness::Ready));

    let first_cohort = DeferredAdmissionCohort::ResultAwait(first);
    let second_cohort = DeferredAdmissionCohort::ResultAwait(third);
    assert_eq!(
        table.release_cohort(&parent, first_cohort, [first, second]),
        None
    );
    assert_eq!(table.release_cohort(&parent, second_cohort, [third]), None);
    assert!(table.settle_staging(&parent, first, DeferredAdmissionReadiness::Ready));
    assert_eq!(
        table.release_cohort(&parent, first_cohort, [first, second]),
        Some(vec![first, second])
    );
    assert_eq!(
        table.release_cohort(&parent, second_cohort, [third]),
        Some(vec![third])
    );
}

#[test]
fn no_body_members_settle_a_cohort_without_lane_registration() {
    let table = DeferredAdmissionTable::default();
    let parent = parent();
    let skipped = OplogIndex::from_u64(2);
    let ready = OplogIndex::from_u64(3);
    assert!(table.insert(parent.clone(), skipped));
    assert!(table.insert(parent.clone(), ready));
    assert!(table.settle_staging(
        &parent,
        skipped,
        DeferredAdmissionReadiness::SettledWithoutBody
    ));
    assert!(table.settle_staging(&parent, ready, DeferredAdmissionReadiness::Ready));

    assert_eq!(
        table.release_cohort(
            &parent,
            DeferredAdmissionCohort::ResultAwait(ready),
            [ready, skipped]
        ),
        Some(vec![ready])
    );
}

#[test]
async fn result_await_skips_a_member_released_after_planning_without_losing_survivors() {
    let table = Arc::new(DeferredAdmissionTable::default());
    let parent = parent();
    let released = OplogIndex::from_u64(2);
    let survivor = OplogIndex::from_u64(3);
    assert!(table.insert(parent.clone(), released));
    assert!(table.insert(parent.clone(), survivor));
    assert!(table.settle_staging(&parent, released, DeferredAdmissionReadiness::Ready));
    assert!(table.settle_staging(&parent, survivor, DeferredAdmissionReadiness::Ready));

    let release_barrier = Arc::new(tokio::sync::Barrier::new(2));
    let cohort_table = table.clone();
    let cohort_parent = parent.clone();
    let cohort_barrier = release_barrier.clone();
    let cohort = tokio::spawn(async move {
        cohort_barrier.wait().await;
        cohort_table.release_cohort(
            &cohort_parent,
            DeferredAdmissionCohort::ResultAwait(released),
            [released, survivor],
        )
    });

    assert!(table.remove(&parent, released));
    release_barrier.wait().await;
    assert_eq!(cohort.await.unwrap(), Some(vec![survivor]));
}

#[test]
fn cohort_claim_holds_parent_close_lock_through_lane_registration() {
    let owner_id = golem_common::model::OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "Agent(owner)".to_string(),
        },
    );
    let lane = OwnerLane::new(owner_id);
    let _primary = lane.enter_primary(OplogIndex::from_u64(1)).unwrap();
    let (operations, operation, invocation_id) =
        accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
    assert!(operation.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready));
    let table = Arc::new(DeferredAdmissionTable::default());
    let parent = parent();
    let start = OplogIndex::from_u64(2);
    assert!(table.insert(parent.clone(), start));
    assert!(table.settle_staging(&parent, start, DeferredAdmissionReadiness::Ready));

    let (claim_entered_tx, claim_entered_rx) = std::sync::mpsc::channel();
    let (continue_claim_tx, continue_claim_rx) = std::sync::mpsc::channel();
    let claim_table = table.clone();
    let claim_operations = operations.clone();
    let claim_lane = lane.clone();
    let claim_parent = parent.clone();
    let claim = std::thread::spawn(move || {
        claim_table
            .try_claim_cohort(
                &claim_parent,
                DeferredAdmissionCohort::ResultAwait(start),
                [start],
                |ready| {
                    claim_entered_tx.send(()).unwrap();
                    continue_claim_rx.recv().unwrap();
                    let (invocations, _wait) = claim_operations.register_ready_bodies(
                        &claim_lane,
                        ready,
                        Some(&claim_parent),
                    )?;
                    Ok::<_, WorkerExecutorError>(invocations)
                },
            )
            .unwrap()
            .unwrap()
    });

    claim_entered_rx.recv().unwrap();
    assert!(table.closed_parents.try_lock().is_err());
    continue_claim_tx.send(()).unwrap();
    assert_eq!(
        claim.join().unwrap(),
        vec![OwnerInvocationId::Entity(invocation_id)]
    );
    assert_eq!(
        operation.admission_if_active(),
        Some(BodyAdmissionState::Registered)
    );
    assert_eq!(table.close_parent_and_snapshot(&parent), Some(Vec::new()));
}

#[test]
fn ready_cancellation_and_lane_registration_have_one_winner() {
    let owner_id = golem_common::model::OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "Agent(owner)".to_string(),
        },
    );
    let lane = OwnerLane::new(owner_id);
    let _primary = lane.enter_primary(OplogIndex::from_u64(1)).unwrap();
    let parent = parent();
    let start = OplogIndex::from_u64(2);

    let (cancelled_operations, cancelled, _) =
        accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
    assert!(cancelled.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready));
    let cancelled_table = DeferredAdmissionTable::default();
    assert!(cancelled_table.insert(parent.clone(), start));
    assert!(cancelled_table.settle_staging(&parent, start, DeferredAdmissionReadiness::Ready));
    assert!(cancelled_table.settle_operation_without_body(
        &parent,
        start,
        DeferredAdmissionReadiness::Ready,
        &cancelled,
        BodyAdmissionState::Ready,
    ));
    assert_eq!(
        cancelled.admission_if_active(),
        Some(BodyAdmissionState::SettledWithoutBody)
    );
    assert_eq!(
        cancelled_table.release_cohort(
            &parent,
            DeferredAdmissionCohort::ResultAwait(start),
            [start]
        ),
        Some(Vec::new())
    );
    drop(cancelled_operations);

    let (registered_operations, registered, invocation_id) =
        accepted_operation(&lane, EntityCallMode::Asynchronous, 3);
    let registered_start = OplogIndex::from_u64(3);
    assert!(
        registered.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready)
    );
    let registered_table = DeferredAdmissionTable::default();
    assert!(registered_table.insert(parent.clone(), registered_start));
    assert!(registered_table.settle_staging(
        &parent,
        registered_start,
        DeferredAdmissionReadiness::Ready
    ));
    assert_eq!(
        registered_table
            .try_claim_cohort(
                &parent,
                DeferredAdmissionCohort::ResultAwait(registered_start),
                [registered_start],
                |ready| {
                    let (invocations, _wait) =
                        registered_operations.register_ready_bodies(&lane, ready, Some(&parent))?;
                    Ok::<_, WorkerExecutorError>(invocations)
                },
            )
            .unwrap()
            .unwrap(),
        vec![OwnerInvocationId::Entity(invocation_id)]
    );
    assert!(!registered_table.settle_operation_without_body(
        &parent,
        registered_start,
        DeferredAdmissionReadiness::Ready,
        &registered,
        BodyAdmissionState::Ready,
    ));
    assert_eq!(
        registered.admission_if_active(),
        Some(BodyAdmissionState::Registered)
    );
}

#[test]
fn parent_end_closes_admission_and_consumes_the_atomic_snapshot() {
    let table = DeferredAdmissionTable::default();
    let parent = parent();
    let ready = OplogIndex::from_u64(2);
    let skipped = OplogIndex::from_u64(3);
    assert!(table.insert(parent.clone(), ready));
    assert!(table.insert(parent.clone(), skipped));
    assert!(table.settle_staging(&parent, ready, DeferredAdmissionReadiness::Ready));
    assert!(table.settle_staging(
        &parent,
        skipped,
        DeferredAdmissionReadiness::SettledWithoutBody
    ));

    assert_eq!(
        table.close_parent_and_snapshot(&parent),
        Some(vec![ready, skipped])
    );
    assert!(!table.insert(parent.clone(), OplogIndex::from_u64(4)));
    assert_eq!(
        table.release_cohort(
            &parent,
            DeferredAdmissionCohort::ParentEnd,
            [ready, skipped]
        ),
        Some(vec![ready])
    );
    assert!(table.clear_closed_parent(&parent));
}

#[test]
fn parent_end_supersedes_an_unfinished_result_await_cohort() {
    let table = DeferredAdmissionTable::default();
    let parent = parent();
    let first = OplogIndex::from_u64(2);
    let second = OplogIndex::from_u64(3);
    assert!(table.insert(parent.clone(), first));
    assert!(table.insert(parent.clone(), second));
    assert_eq!(
        table.release_cohort(
            &parent,
            DeferredAdmissionCohort::ResultAwait(second),
            [second]
        ),
        None
    );
    assert_eq!(
        table.close_parent_and_snapshot(&parent),
        Some(vec![first, second])
    );
    assert!(table.settle_staging(&parent, first, DeferredAdmissionReadiness::Ready));
    assert!(table.settle_staging(&parent, second, DeferredAdmissionReadiness::Ready));
    assert_eq!(
        table.release_cohort(
            &parent,
            DeferredAdmissionCohort::ResultAwait(second),
            [second]
        ),
        Some(Vec::new())
    );
    assert_eq!(
        table.release_cohort(&parent, DeferredAdmissionCohort::ParentEnd, [first, second]),
        Some(vec![first, second])
    );
    assert!(table.clear_closed_parent(&parent));
}
