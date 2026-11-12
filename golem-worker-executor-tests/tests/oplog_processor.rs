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

use crate::Tracing;
use crate::active_agents::{ComponentLoadGate, GatedComponentService};
use axum::{Json, Router, routing::post};
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    CreateWorkerRequest, ProcessOplogEntriesRequest, create_worker_response,
    process_oplog_entries_response,
};
use golem_common::agent_id;
use golem_common::model::agent::{AgentMode, OwnerKind};
use golem_common::model::component::{InstalledPlugin, PluginPriority};
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::plugin_registration::PluginRegistrationId;
use golem_common::model::{AgentId, AgentStatus, InvocationId, OwnedAgentId};
use golem_service_base::model::auth::AuthCtx;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::{HasOplogProcessorPlugin, UsesAllDeps};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start, start_with_overrides, test_component,
};
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test, test_dep, timeout};
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
test_component!(
    processor_component,
    "processor_component",
    "oplog_processor_release",
    "oplog:processor"
);

async fn deliver(
    executor: &TestWorkerExecutor,
    request: ProcessOplogEntriesRequest,
) -> anyhow::Result<()> {
    let response = executor
        .client
        .clone()
        .process_oplog_entries(request)
        .await?
        .into_inner();
    anyhow::ensure!(
        matches!(
            response.result,
            Some(process_oplog_entries_response::Result::Success(_))
        ),
        "processor delivery failed: {response:?}"
    );
    Ok(())
}

fn assert_execution_count(entries: &[OplogEntry], key: &InvocationId, count: usize) {
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry,
                OplogEntry::AgentInvocationStarted { invocation_id, .. } if invocation_id == key
            ))
            .count(),
        count
    );
}

#[test]
#[timeout("120s")]
async fn processor_only_owner_recovers_accepted_batch_and_deduplicates(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    #[tagged_as("processor_component")] processor_component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let gate = Arc::new(ComponentLoadGate::new());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            wrap_component_service: Some(Arc::new({
                let gate = gate.clone();
                move |inner| {
                    Arc::new(GatedComponentService {
                        inner,
                        gate: gate.clone(),
                    })
                }
            })),
            ..Default::default()
        },
    )
    .await?;
    let processor = executor
        .component_dep(&context.default_environment_id, processor_component)
        .store()
        .await?;
    let source = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let source_id = executor
        .start_agent(&source.id, agent_id!("Clock", "processor-source"))
        .await?;
    executor
        .wait_for_status(&source_id, AgentStatus::Idle, Duration::from_secs(20))
        .await?;
    let metadata = executor.get_worker_metadata(&source_id).await?;
    let source_entries = executor.stored_oplog(&source_id).await;
    let target = AgentId {
        component_id: processor.id,
        agent_id: OwnerKind::oplog_processor_instance_name(Uuid::new_v4()),
    };

    // Ordinary ingress must not turn a reserved processor identity into an agent.
    let creation = executor
        .client
        .clone()
        .create_worker(CreateWorkerRequest {
            agent_id: Some(target.clone().into()),
            component_owner_account_id: Some(context.account_id.into()),
            environment_id: Some(context.default_environment_id.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            ..Default::default()
        })
        .await?
        .into_inner();
    assert!(matches!(
        creation.result,
        Some(create_worker_response::Result::Failure(_))
    ));
    assert!(executor.get_worker_metadata(&target).await.is_err());

    let (callbacks_tx, mut callbacks) = tokio::sync::mpsc::unbounded_channel();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let callback_url = format!("http://{}/batch", listener.local_addr()?);
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/batch",
                post(move |Json(body): Json<serde_json::Value>| {
                    let callbacks_tx = callbacks_tx.clone();
                    async move {
                        callbacks_tx.send(body).unwrap();
                    }
                }),
            ),
        )
        .await
        .unwrap();
    });
    // Abort the in-process listener even if an assertion fails.
    let _server_guard = tokio_util::task::AbortOnDropHandle::new(server);
    let source_owned = OwnedAgentId::new(context.default_environment_id, &source_id);
    let seed = executor
        .active_agent(&source_owned)
        .await
        .unwrap()
        .primary();
    let source_metadata = seed.get_latest_worker_metadata().await;
    let dispatcher = seed.all().oplog_processor_plugin();
    drop(seed);
    let plugin = InstalledPlugin {
        environment_plugin_grant_id: EnvironmentPluginGrantId::new(),
        priority: PluginPriority(0),
        parameters: [("callback-url".to_string(), callback_url.clone())].into(),
        plugin_registration_id: PluginRegistrationId::new(),
        plugin_name: "processor-owner-test".to_string(),
        plugin_version: "1".to_string(),
        oplog_processor_component_id: Some(processor.id),
        oplog_processor_component_revision: Some(processor.revision),
    };
    let target = dispatcher
        .resolve_target(context.default_environment_id, &plugin)
        .await?;
    assert!(dispatcher.is_local(&target).await?);
    let mut request = ProcessOplogEntriesRequest {
        agent_id: Some(target.clone().into()),
        environment_id: Some(context.default_environment_id.into()),
        component_revision: processor.revision.into(),
        idempotency_key: Some(InvocationId(Uuid::new_v4()).into()),
        account_id: Some(context.account_id.into()),
        config: [("callback-url".to_string(), callback_url)].into(),
        metadata: Some(metadata.try_into().map_err(anyhow::Error::msg)?),
        first_entry_index: 1,
        entries: source_entries
            .into_iter()
            .take(1)
            .map(TryInto::try_into)
            .collect::<Result<_, _>>()
            .map_err(anyhow::Error::msg)?,
        auth_ctx: Some(AuthCtx::system().into()),
    };
    let mut user_request = request.clone();
    user_request.auth_ctx = Some(executor.auth_ctx().into());
    let rejected = executor
        .client
        .clone()
        .process_oplog_entries(user_request)
        .await?
        .into_inner();
    assert!(matches!(
        rejected.result,
        Some(process_oplog_entries_response::Result::Failure(_))
    ));
    assert!(executor.get_worker_metadata(&target).await.is_err());

    // The service acknowledges durable acceptance before loading the component.
    gate.arm();
    dispatcher
        .send(
            source_metadata,
            &plugin,
            &target,
            OplogIndex::INITIAL,
            vec![executor.stored_oplog(&source_id).await.remove(0)],
        )
        .await?;
    drop(dispatcher);
    gate.entered().await;
    let accepted = executor.stored_oplog(&target).await;
    let key = accepted
        .iter()
        .find_map(|entry| match entry {
            OplogEntry::PendingAgentInvocation { invocation_id, .. } => Some(invocation_id.clone()),
            _ => None,
        })
        .expect("local dispatch must durably accept the batch");
    request.idempotency_key = Some(key.clone().into());
    let OplogEntry::Create { parameters, .. } = &accepted[0] else {
        panic!("missing Create")
    };
    assert_eq!(parameters.owner_kind, OwnerKind::OplogProcessor);
    assert_eq!(parameters.agent_mode, AgentMode::Durable);
    assert!(parameters.agent_type_name.is_none());
    assert!(parameters.canonical_key.is_none());
    assert!(accepted.iter().any(|entry| matches!(entry,
        OplogEntry::PendingAgentInvocation { invocation_id, .. } if invocation_id == &key
    )));
    assert_execution_count(&accepted, &key, 0);
    assert!(callbacks.try_recv().is_err());

    // Do not resubmit or explicitly resume: shard recovery must rediscover this work.
    drop(executor);
    let executor = start(deps, &context).await?;
    let callback = tokio::time::timeout(Duration::from_secs(30), callbacks.recv())
        .await?
        .unwrap();
    assert_eq!(callback["source_worker_id"], source_id.agent_id);
    assert_eq!(callback["component_id"], source_id.component_id.to_string());
    assert_eq!(callback["account_id"], context.account_id.to_string());
    assert_eq!(callback["first_entry_index"], 1);
    assert_eq!(callback["entry_count"], 1);
    executor
        .wait_for_status(&target, AgentStatus::Idle, Duration::from_secs(20))
        .await?;
    let completed = executor.stored_oplog(&target).await;
    assert_execution_count(&completed, &key, 1);

    let fork_target = AgentId {
        component_id: processor.id,
        agent_id: agent_id!("Clock", "processor-fork-target").to_string(),
    };
    let cut = OplogIndex::from_u64(completed.len() as u64);
    assert!(cut >= OplogIndex::INITIAL.next());
    let error = executor
        .fork_worker(&target, &fork_target.agent_id, cut)
        .await
        .expect_err("oplog-processor owners must not be forked");
    let error = format!("{error:?}");
    assert!(error.contains("InvalidRequest"), "{error}");
    assert!(
        error.contains("Only component-agent owners can be forked"),
        "{error}"
    );
    assert!(executor.get_worker_metadata(&fork_target).await.is_err());
    assert!(executor.stored_oplog(&fork_target).await.is_empty());
    assert!(
        !executor
            .worker_is_cached(&OwnedAgentId::new(
                context.default_environment_id,
                &fork_target
            ))
            .await
    );

    deliver(&executor, request.clone()).await?;
    let fresh_key = InvocationId(Uuid::new_v4());
    let mut fresh = request.clone();
    fresh.idempotency_key = Some(fresh_key.clone().into());
    deliver(&executor, fresh).await?;
    let next = tokio::time::timeout(Duration::from_secs(30), callbacks.recv())
        .await?
        .unwrap();
    assert_eq!(next, callback);
    executor
        .wait_for_status(&target, AgentStatus::Idle, Duration::from_secs(20))
        .await?;
    let completed = executor.stored_oplog(&target).await;
    assert_execution_count(&completed, &key, 1);
    assert_execution_count(&completed, &fresh_key, 1);
    assert_eq!(
        completed
            .iter()
            .filter(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { .. }))
            .count(),
        2
    );
    assert!(
        callbacks.try_recv().is_err(),
        "a duplicate batch repeated the HTTP effect"
    );

    drop(executor);
    let executor = start(deps, &context).await?;
    deliver(&executor, request.clone()).await?;
    let after_restart_key = InvocationId(Uuid::new_v4());
    let mut after_restart = request;
    after_restart.idempotency_key = Some(after_restart_key.clone().into());
    deliver(&executor, after_restart).await?;
    let after_restart_callback = tokio::time::timeout(Duration::from_secs(30), callbacks.recv())
        .await?
        .unwrap();
    assert_eq!(after_restart_callback, callback);
    executor
        .wait_for_status(&target, AgentStatus::Idle, Duration::from_secs(20))
        .await?;
    let reconstructed = executor.stored_oplog(&target).await;
    assert_execution_count(&reconstructed, &key, 1);
    assert_execution_count(&reconstructed, &fresh_key, 1);
    assert_execution_count(&reconstructed, &after_restart_key, 1);
    assert_eq!(
        reconstructed
            .iter()
            .filter(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { .. }))
            .count(),
        3
    );
    assert!(
        callbacks.try_recv().is_err(),
        "completed replay repeated the HTTP effect"
    );
    Ok(())
}

#[test]
#[timeout("120s")]
async fn processor_committed_admission_survives_owner_retirement(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    #[tagged_as("processor_component")] processor_component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::{AgentInvocation, OwnedAgentId};
    use golem_worker_executor::services::{HasWorkerActivator, UsesAllDeps};
    use golem_worker_executor::storage::keyvalue::fault_injecting::{
        FaultInjectingKeyValueStorage, KeyValueStorageFaults,
    };

    let context = TestContext::new(last_unique_id);
    let faults = KeyValueStorageFaults::default();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            wrap_key_value_storage: Some(Arc::new({
                let faults = faults.clone();
                move |inner| Arc::new(FaultInjectingKeyValueStorage::new(inner, faults.clone()))
            })),
            ..Default::default()
        },
    )
    .await?;
    let source_component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let processor = executor
        .component_dep(&context.default_environment_id, processor_component)
        .store()
        .await?;
    let source = executor
        .start_agent(
            &source_component.id,
            agent_id!("Clock", "provisional-admission-source"),
        )
        .await?;
    executor
        .wait_for_status(&source, AgentStatus::Idle, Duration::from_secs(20))
        .await?;
    let source_owned = OwnedAgentId::new(context.default_environment_id, &source);
    let seed = executor
        .active_agent(&source_owned)
        .await
        .unwrap()
        .primary();
    let metadata = seed.get_latest_worker_metadata().await;
    seed.stop_if_idle().await;
    // Keep the service graph, but make the processor the only cached owner that the
    // public environment-unload boundary will retire.
    executor.active_agents().remove(&source_owned).await;
    let target = AgentId {
        component_id: processor.id,
        agent_id: OwnerKind::oplog_processor_instance_name(Uuid::new_v4()),
    };
    let target_owned = OwnedAgentId::new(context.default_environment_id, &target);
    let key = InvocationId(Uuid::new_v4());
    // Recovery-index publication follows the durable PendingAgentInvocation commit,
    // while admission still holds the lifecycle lock.
    let gate = faults.pause_next("add");
    let activator = seed.all().worker_activator();
    let admission = tokio::spawn({
        let target_owned = target_owned.clone();
        let key = key.clone();
        async move {
            activator
                .enqueue_oplog_processor(
                    &target_owned,
                    processor.revision,
                    AgentInvocation::ProcessOplogEntries {
                        idempotency_key: key,
                        account_id: metadata.created_by,
                        config: Vec::new(),
                        metadata: metadata.into(),
                        first_entry_index: golem_common::model::oplog::OplogIndex::INITIAL,
                        entries: vec![OplogEntry::GrowMemory {
                            timestamp: golem_common::model::Timestamp::now_utc(),
                            delta: 1,
                        }],
                    },
                )
                .await
        }
    });
    gate.entered().await;
    let accepted = executor.stored_oplog(&target).await;
    assert!(
        accepted.iter().any(|entry| matches!(entry,
            OplogEntry::PendingAgentInvocation { invocation_id, .. } if invocation_id == &key
        )),
        "the gate must follow durable acceptance"
    );
    let active = executor.active_agents();
    let retirement = active.unload_environment(context.default_environment_id);
    tokio::pin!(retirement);
    // The uncontended cache snapshot reaches retirement synchronously; retirement
    // records its cancellation before it waits for admission's lifecycle lock.
    assert!(futures::poll!(&mut retirement).is_pending());
    gate.release();
    let ((), result) = tokio::join!(retirement, admission);
    let result = result?;
    assert!(
        result.is_ok(),
        "committed processor batch was returned as rejected: {result:?}"
    );
    Ok(())
}
