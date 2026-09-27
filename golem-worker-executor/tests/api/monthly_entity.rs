//! Exercises the scoped-Store API below tool dispatch. The scope has no durable entity Start;
//! only real guest host calls are recorded. This proves active Store and meter registration,
//! not tool protocol recovery or entity-only numerical attribution of the exhausted capacity.
//! Raw host-call records are observed because public entity attribution needs a durable root.

use super::*;
use anyhow::Context as _;
use golem_common::base_model::json::NormalizedJsonValue;
use golem_common::model::account::AccountEmail;
use golem_common::model::agent::AgentPrincipal;
use golem_common::model::component::ComponentName;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::entity::{
    EntityActivation, EntityActivationPolicy, EntityInvocationId, EntityInvocationScope,
    ExecutableTarget, FilesystemCapability, InvocationExecutionMode, OwnedAgentEntityId,
};
use golem_common::model::oplog::OplogEntry;
use golem_common::model::tool::{
    CompiledToolBinding, SecretKeyScope, ToolBindingOwner, ToolFilesystemAccess, ToolName,
    ToolProvisionConfig, ToolSource,
};
use golem_common::model::{AgentInvocation, AgentInvocationResult};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::durable_host::DurableWorkerCtxView;
use golem_worker_executor::services::oplog::CommitLevel;
use golem_worker_executor::services::{HasComponentService, HasOplog};
use golem_worker_executor::worker::Worker;
use golem_worker_executor::worker::invocation::{
    InvocationMode, InvokeResult, invoke_observed_and_traced, lower_invocation,
};
use golem_worker_executor::workerctx::default::Context;
use golem_worker_executor::workerctx::{
    EntityInvocationManagement, InvocationManagement, WorkerCtx,
};
use pretty_assertions::assert_eq;
use test_r::test;
use tokio::io::AsyncReadExt;

#[test]
#[timeout("60s")]
async fn monthly_memory_stop_retires_active_scoped_entity_and_primary_tcp(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let policy = MonthlyResourcePolicy {
        period: AccountUsagePeriod::current(),
        mode: MonthlyUsageMode::HardLimit,
        available_fuel: u64::MAX,
        available_memory_gb_seconds: u64::MAX,
        available_memory_byte_nanoseconds_remainder: 0,
        available_durable_storage_byte_seconds: u64::MAX,
        available_durable_storage_byte_nanoseconds_remainder: 0,
        available_ephemeral_storage_byte_seconds: u64::MAX,
        available_ephemeral_storage_byte_nanoseconds_remainder: 0,
    };
    let registry = Arc::new(MutableResourceLimitsRegistry::new(policy.clone()));
    let shutdown = CancellationToken::new();
    let _shutdown_on_drop = shutdown.clone().drop_guard();
    let metering = ResourceUsageMeteringConfig {
        compute: false,
        memory: true,
        filesystem: false,
    };
    let limits = ResourceLimitsGrpc::new(
        registry.clone(),
        Duration::from_secs(3600),
        Duration::ZERO,
        metering,
        shutdown,
    );
    let context = TestContext::new(last_unique_id);
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        limits.clone(),
        Arc::new(move |config| config.resource_usage_metering = metering),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let name = agent_id!("Networking", "monthly-active-scoped-entity");
    let id = executor.start_agent(&component.id, name.clone()).await?;
    wait_for_invocation_pair(&executor, &id, OplogIndex::INITIAL).await?;
    let key = IdempotencyKey::fresh();
    let primary_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let entity_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let entity_port = entity_listener.local_addr()?.port();
    executor
        .invoke_agent_with_key(
            &component,
            &name,
            &key,
            "tcp_collect_p3",
            data_value!(primary_listener.local_addr()?.port()),
        )
        .await?;
    let (mut primary_socket, _) =
        tokio::time::timeout(Duration::from_secs(10), primary_listener.accept())
            .await
            .context("primary TCP connection")??;
    let owned = OwnedAgentId::new(context.default_environment_id, &id);
    let active = executor.production_active_agent(&owned).await.unwrap();
    let worker = active.primary();
    let primary_calls = pending_receive(&worker, OplogIndex::INITIAL, None).await?;
    let resources = active.resources();
    let account = limits.initialize_account(context.account_id).await?;
    assert!(worker.concurrent_agent_permit_is_held().await);
    assert_eq!(account.monthly_observer_count_for_test(), 1);
    assert_eq!(resources.live_usage_flusher_count_for_test(), 1);
    let generation = worker.resident_generation_for_test();
    let acquisitions = worker.permit_acquisitions_for_test();
    let admitted_revision = registry.current_limits().monthly_policy_revision;
    let metadata = Arc::new(
        worker
            .component_service()
            .get_metadata(component.id, Some(component.revision))
            .await?,
    );
    let deployment_revision = DeploymentRevision::try_from(1_u64).unwrap();
    let activation = Arc::new(
        EntityActivation::new(
            ExecutableTarget::new(component.id, component.revision),
            deployment_revision,
            EntityActivationPolicy::Tool {
                provision: ToolProvisionConfig::default(),
                binding: Box::new(CompiledToolBinding {
                    deployment_revision,
                    release_id: None,
                    owner: ToolBindingOwner::AgentType {
                        agent_type_name: name.agent_type.clone(),
                    },
                    tool_name: ToolName::try_from("monthly-scoped-entity").unwrap(),
                    version: "1.0.0".to_string(),
                    metadata_version: "0.1.0".to_string(),
                    metadata_digest: Default::default(),
                    account_id: context.account_id,
                    account_email: AccountEmail::new("test@golem"),
                    parameters: NormalizedJsonValue::new(serde_json::json!({})),
                    config_keys_readable: Default::default(),
                    secret_keys_readable: SecretKeyScope::All,
                    secret_keys_revealable: SecretKeyScope::All,
                    filesystem_access: ToolFilesystemAccess::Denied,
                    source: ToolSource::Component {
                        component_id: component.id,
                        component_revision: component.revision,
                        component_name: ComponentName("test:monthly-scoped-entity".to_string()),
                    },
                }),
                mcp_import: None,
            },
            FilesystemCapability::Incapable,
        )
        .unwrap(),
    );
    let slot = active.entity_slot(&activation.entity());
    let host = active.entity_instance_host(&activation, metadata)?;
    assert!(Arc::ptr_eq(&host.owner_execution(), &active.execution()));
    assert!(Arc::ptr_eq(&host.owner_resources(), &resources));
    let hosted = tokio::time::timeout(Duration::from_secs(10), host.instantiate_entity())
        .await
        .context("entity Store instantiation")??;
    assert_eq!(resources.live_usage_flusher_count_for_test(), 2);
    let before_entity = worker.oplog().current_oplog_index().await;
    let scope_index = before_entity.next();
    let principal = Principal::Agent(AgentPrincipal {
        agent_id: id.clone(),
    });
    // Like the instance-layer tests, install an in-memory scope without a tool dispatcher.
    // The index identifies the scope only; this test does not create a durable entity root.
    let scope = EntityInvocationScope::new(
        EntityInvocationId::new(
            OwnedAgentEntityId {
                owner: owned.clone(),
                entity: activation.entity(),
            },
            scope_index,
        )
        .unwrap(),
        before_entity,
        activation,
        principal.clone(),
        InvocationExecutionMode::Live,
        IdempotencyKey::fresh(),
        true,
        false,
        IdempotencyKey::fresh(),
    )
    .unwrap();
    let expected_scope = scope.clone();
    let entity = tokio::spawn(hosted.invoke_scoped(scope, move |instance, store| {
        Box::pin(async move {
            assert_eq!(
                store.data().entity_invocation_scope(),
                Some(&expected_scope)
            );
            let parsed = store.data().parsed_agent_id().unwrap();
            let metadata = store.data().component_metadata().metadata.clone();
            let init_key = IdempotencyKey::fresh();
            store
                .data_mut()
                .set_current_idempotency_key(init_key.clone())
                .await;
            store
                .data_mut()
                .set_current_invocation_context(InvocationContextStack::fresh())
                .await?;
            let init = AgentInvocation::AgentInitialization {
                idempotency_key: init_key,
                input: parsed.parameters.value().clone(),
                invocation_context: InvocationContextStack::fresh(),
                principal: principal.clone(),
            };
            // Replay mode suppresses primary invocation markers. The entity scope remains Live.
            let result = invoke_observed_and_traced(
                lower_invocation(init, &metadata, Some(&parsed))?,
                store,
                instance,
                InvocationMode::Replay,
            )
            .await?;
            assert!(matches!(result, InvokeResult::Succeeded { result, .. }
                if matches!(*result, AgentInvocationResult::AgentInitialization)));
            assert!(store.data().durable_ctx().total_linear_memory_size() > 0);
            let method_key = IdempotencyKey::fresh();
            store
                .data_mut()
                .set_current_idempotency_key(method_key.clone())
                .await;
            let method = AgentInvocation::AgentMethod {
                idempotency_key: method_key,
                method_name: "tcp_collect_p3".to_string(),
                input: data_value!(entity_port).value().clone(),
                invocation_context: InvocationContextStack::fresh(),
                principal,
                scope_card: None,
            };
            invoke_observed_and_traced(
                lower_invocation(method, &metadata, Some(&parsed))?,
                store,
                instance,
                InvocationMode::Replay,
            )
            .await
        })
    }));
    let (mut entity_socket, _) =
        tokio::time::timeout(Duration::from_secs(10), entity_listener.accept())
            .await
            .context("scoped entity guest TCP connection")??;
    let entity_calls = pending_receive(&worker, before_entity, Some(scope_index)).await?;
    let registered = slot.active_invocations();
    assert_eq!(registered.len(), 1);
    assert_eq!(registered[0].invocation_id.start_index(), scope_index);
    assert_eq!(registered[0].mode, InvocationExecutionMode::Live);
    assert!(registered[0].store_attached);
    assert!(registered[0].linear_memory_bytes > 0);
    assert!(!entity.is_finished());
    assert_eq!(resources.live_usage_flusher_count_for_test(), 2);
    assert_eq!(account.monthly_observer_count_for_test(), 1);
    assert!(worker.concurrent_agent_permit_is_held().await);
    eprintln!(
        "[monthly-entity] primary calls={primary_calls:?}, entity calls={entity_calls:?}, registration={registered:?}, live owner flushers=2, observers=1, generation={generation}"
    );

    let (_, accepted, release_driver) = worker.pause_next_stop_driver_for_test();
    let mut raw = worker.raw_interrupt_for_test();
    let mut exhausted = policy;
    exhausted.available_memory_gb_seconds = 0;
    registry.set_policy(exhausted);
    let refresh = tokio::spawn({
        let limits = limits.clone();
        async move { limits.run_batch_for_test().await }
    });
    tokio::time::timeout(Duration::from_secs(10), accepted)
        .await
        .context("quota accepted with both Stores active")??;
    assert_eq!(
        limits
            .initialized_policy_revision_for_test(context.account_id)
            .await,
        Some(registry.current_limits().monthly_policy_revision)
    );
    let cause = worker
        .monthly_stop_for_test()
        .expect("accepted monthly stop");
    assert!(matches!(cause, InterruptKind::Suspend(_)));
    assert_eq!(worker.pending_stop_for_test().await, Some(cause));
    assert_eq!(worker.frozen_stop_for_test(), None);
    assert_eq!(worker.owner_stop_for_test().await, None);
    assert_eq!(
        pending_receive(&worker, OplogIndex::INITIAL, None).await?,
        primary_calls
    );
    assert_eq!(
        pending_receive(&worker, before_entity, Some(scope_index)).await?,
        entity_calls
    );
    assert_eq!(resources.live_usage_flusher_count_for_test(), 2);
    assert_eq!(slot.active_invocation_count(), 1);
    assert!(!entity.is_finished());
    assert!(worker.concurrent_agent_permit_is_held().await);
    assert_eq!(account.monthly_observer_count_for_test(), 1);
    let mut receipt = worker.accepted_stop_receipt_for_test();
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    eprintln!(
        "[monthly-entity] accepted {cause:?}, both Stores remain active, permit held, one monitor, receipt pending"
    );
    release_driver.send(false).unwrap();
    let entity_result = tokio::time::timeout(Duration::from_secs(10), entity)
        .await
        .context("scoped entity invocation retirement")??;
    eprintln!("[monthly-entity] scoped body outcome={entity_result:?}");
    // The guest's cooperative interrupt and the slot's cancellation race after owner fencing.
    match entity_result {
        Ok(InvokeResult::Interrupted { interrupt_kind, .. }) => assert_eq!(interrupt_kind, cause),
        Err(WorkerExecutorError::Runtime { details }) => {
            assert_eq!(details, "Entity body was cancelled")
        }
        other => panic!("scoped body must retire through the accepted stop: {other:?}"),
    }
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??,
        cause
    );
    assert_eq!(worker.frozen_stop_for_test(), Some(cause));
    assert_eq!(worker.owner_stop_for_test().await, Some(cause));
    let mut byte = [0_u8];
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), entity_socket.read(&mut byte))
            .await
            .context("entity socket physical close")??,
        0
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), primary_socket.read(&mut byte))
            .await
            .context("primary socket physical close")??,
        0
    );
    tokio::time::timeout(
        Duration::from_secs(10),
        worker.join_accepted_stops_for_test(),
    )
    .await??;
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("joined physical unload and permit release")?;
    tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
    tokio::time::timeout(Duration::from_secs(10), refresh).await??;
    tokio::time::timeout(Duration::from_secs(10), receipt.recv()).await??;
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty
            | tokio::sync::broadcast::error::TryRecvError::Closed)
    ));
    assert_eq!(slot.active_invocation_count(), 0);
    assert_eq!(slot.charged_linear_memory_bytes(), 0);
    assert!(!slot.is_accepting());
    assert_eq!(resources.live_usage_flusher_count_for_test(), 0);
    assert_eq!(account.monthly_observer_count_for_test(), 0);
    assert_eq!(worker.resident_generation_for_test(), generation);
    assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
    let stopped = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await;
    assert_eq!(stopped.values().filter(|e| matches!(e, OplogEntry::AgentInvocationStarted { idempotency_key, .. } if idempotency_key == &key)).count(), 1);
    assert_eq!(
        stopped
            .values()
            .filter(|e| matches!(e, OplogEntry::AgentInvocationStarted { .. }))
            .count(),
        2
    );
    assert_eq!(
        stopped
            .values()
            .filter(|e| matches!(e, OplogEntry::AgentInvocationFinished { .. }))
            .count(),
        1
    );
    assert_eq!(
        stopped
            .values()
            .filter(|entry| matches!(entry, OplogEntry::Suspend { .. }))
            .count(),
        1
    );
    assert!(
        !stopped
            .values()
            .any(|e| matches!(e, OplogEntry::Error { .. } | OplogEntry::Interrupted { .. }))
    );
    for interrupted_start in [
        primary_calls.0,
        primary_calls.1,
        entity_calls.0,
        entity_calls.1,
    ] {
        assert!(
            !stopped.values().any(|e| matches!(e,
                OplogEntry::End { start_index, .. } | OplogEntry::Cancelled { start_index, .. }
                if *start_index == interrupted_start
            )),
            "interrupted receive {interrupted_start} must remain incomplete"
        );
    }
    limits.run_batch_for_test().await;
    let usage = registry.applied_memory_byte_nanoseconds();
    assert!(usage > 0);
    assert!(registry.applied_memory_byte_nanoseconds_at_revision(admitted_revision) > 0);
    limits.run_batch_for_test().await;
    assert_eq!(registry.applied_memory_byte_nanoseconds(), usage);
    eprintln!(
        "[monthly-entity] scoped body and both sockets retired, physical cleanup joined, one Suspend, no receive terminals, permit released, live owner flushers=0, observers=0, settled owner byte-nanoseconds={usage}"
    );
    Ok(())
}

async fn pending_receive(
    worker: &Worker<Context>,
    after: OplogIndex,
    parent: Option<OplogIndex>,
) -> anyhow::Result<(OplogIndex, OplogIndex)> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let oplog = worker.oplog();
            let observed_tip = oplog.current_oplog_index().await;
            oplog.commit(CommitLevel::Always).await;
            let entries = oplog
                .read_exact(OplogIndex::INITIAL, observed_tip.as_u64())
                .await;
            let scopes = entries.iter().filter_map(|(index, entry)| match entry {
                OplogEntry::Start { function_name, parent_start_index, .. }
                    if *index > after && function_name.to_string() == "<scope:batched-write>"
                        && *parent_start_index == parent => Some(*index),
                _ => None,
            }).collect::<HashSet<_>>();
            let receives = entries.iter().filter_map(|(index, entry)| match entry {
                OplogEntry::Start { function_name, parent_start_index: Some(scope), .. }
                    if *index > after && function_name.to_string() == "sockets::types::tcp-socket::receive"
                        && scopes.contains(scope) => Some((*index, *scope)),
                _ => None,
            }).collect::<Vec<_>>();
            if let [(receive, scope)] = receives.as_slice() {
                let chunks = entries.iter().filter_map(|(index, entry)| match entry {
                    OplogEntry::Start { function_name, parent_start_index, .. }
                        if function_name.to_string() == "sockets::types::tcp-socket::receive-chunk"
                            && *parent_start_index == Some(*scope) => Some(*index),
                    _ => None,
                }).collect::<Vec<_>>();
                if let [chunk] = chunks.as_slice() {
                    for pending in [scope, receive, chunk] {
                        assert!(!entries.values().any(|entry| matches!(entry,
                            OplogEntry::End { start_index, .. } | OplogEntry::Cancelled { start_index, .. }
                                if start_index == pending
                        )));
                    }
                    return Ok((*receive, *chunk));
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.context("committed silent TCP receive and chunk without terminal")?
}
