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
use futures::poll;
use golem_common::model::agent::{
    AgentMode, AgentPrincipal, AgentTarget, InvocationFreshnessDisposition, Principal,
};
use golem_common::model::card::{InvocationWalletPin, WalletVersionToken};
use golem_common::model::component::ComponentDto;
use golem_common::model::durable_stream::*;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{
    CreateRequestKind, OplogEntry, OplogIndex, OplogPayload, SpanData,
};
use golem_common::model::{
    AgentId, AgentInvocationPayload, AgentInvocationResult, IdempotencyKey, OwnedAgentId, Timestamp,
};
use golem_common::{agent_id, data_value};
use golem_schema::schema::SchemaFingerprintV1;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::storage::blob::agent_path_segment;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::model::LookupResult;
use golem_worker_executor::services::oplog::OplogOps;
use golem_worker_executor::services::rpc::Rpc;
use golem_worker_executor::services::{
    HasActiveAgents, HasOplog, HasOplogService, HasRpc, HasShardService, HasWorkerProxy,
    HasWorkerService, UsesAllDeps,
};
use golem_worker_executor::storage::keyvalue::KeyValueStorageError;
use golem_worker_executor::storage::keyvalue::fault_injecting::{
    FaultInjectingKeyValueStorage, KeyValueStorageFaults,
};
use golem_worker_executor::worker::Worker;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerCtx,
    TestWorkerExecutor, WorkerExecutorTestDependencies, start_with_overrides,
};
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);

const CACHE_TTL: Duration = Duration::from_millis(50);

#[path = "create_config_admission.rs"]
mod create_config_admission;

#[test]
#[timeout("2 minutes")]
async fn warm_get_or_create_does_not_wait_for_oplog_maintenance(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        CreateWorkerRequest, create_worker_response,
    };

    let (executor, seed, _) = setup(last_unique_id, deps, component).await?;
    let metadata = seed.get_initial_worker_metadata();
    let oplog_service = seed.all().oplog_service();
    let _maintenance = oplog_service.lock_lifecycle(&seed.agent_id()).await;
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        executor.client.clone().prepare_worker(CreateWorkerRequest {
            agent_id: Some(seed.agent_id().into()),
            component_owner_account_id: Some(metadata.created_by.into()),
            environment_id: Some(metadata.environment_id.into()),
            ignore_already_existing: true,
            auth_ctx: Some(AuthCtx::System.into()),
            ..Default::default()
        }),
    )
    .await??
    .into_inner();
    match response.result {
        Some(create_worker_response::Result::Success(success)) => {
            assert_eq!(success.instance_id.unwrap(), metadata.fingerprint.0.into());
        }
        other => anyhow::bail!("warm acquisition failed: {other:?}"),
    }
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn fresh_ephemeral_streaming_preparation_skips_the_existence_lookup(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let (executor, seed, _) = setup(last_unique_id, deps, component).await?;
    let key = IdempotencyKey::fresh();
    let parsed = agent_id!("EphemeralCounter", "fresh-streaming-preparation")
        .with_ephemeral_invocation_phantom(
            &golem_common::model::InvocationRequestKey::Internal(key.clone())
                .owner_key("increment"),
        )
        .map_err(anyhow::Error::msg)?;
    let target = OwnedAgentId::new(
        seed.get_initial_worker_metadata().environment_id,
        &AgentId::from_agent_id(seed.agent_id().component_id, &parsed)
            .map_err(anyhow::Error::msg)?,
    );
    let demand = seed
        .all()
        .rpc()
        .create_demand(
            AgentTarget::Current(target.agent_id()),
            target.environment_id,
            "increment",
            seed.get_initial_worker_metadata().created_by,
            &seed.agent_id(),
            &[],
            InvocationContextStack::fresh(),
            vec![],
            &AuthCtx::System,
            Some(key.clone()),
            InvocationFreshnessDisposition::KnownFresh,
        )
        .await?;

    assert!(!demand.fingerprint().0.is_nil());
    assert_eq!(
        executor.oplog_service_call_count(&target.agent_id, "create_fresh"),
        1
    );
    assert_eq!(
        executor.oplog_service_call_count(&target.agent_id, "create"),
        0
    );
    assert_eq!(
        executor.oplog_service_call_count(&target.agent_id, "exists"),
        0
    );

    let retry = seed
        .all()
        .rpc()
        .create_demand(
            AgentTarget::Current(target.agent_id()),
            target.environment_id,
            "increment",
            seed.get_initial_worker_metadata().created_by,
            &seed.agent_id(),
            &[],
            InvocationContextStack::fresh(),
            vec![],
            &AuthCtx::System,
            Some(key),
            InvocationFreshnessDisposition::MayExist,
        )
        .await?;
    assert_eq!(retry.fingerprint(), demand.fingerprint());
    assert_eq!(
        executor.oplog_service_call_count(&target.agent_id, "create_fresh"),
        1
    );
    assert_eq!(
        executor.oplog_service_call_count(&target.agent_id, "create"),
        0
    );
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn grpc_ephemeral_streaming_preparation_validates_freshness_and_reuses_the_fingerprint(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition as ProtoFreshness;
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        CreateWorkerRequest, create_worker_response,
    };

    let (executor, seed, _) = setup(last_unique_id, deps, component).await?;
    let key = IdempotencyKey::fresh();
    let parsed = agent_id!("EphemeralCounter", "fresh-grpc-streaming-preparation")
        .with_ephemeral_invocation_phantom(&key)
        .map_err(anyhow::Error::msg)?;
    let target = AgentId::from_agent_id(seed.agent_id().component_id, &parsed)
        .map_err(anyhow::Error::msg)?;
    let mut request = CreateWorkerRequest {
        agent_id: Some(target.clone().into()),
        component_owner_account_id: Some(seed.get_initial_worker_metadata().created_by.into()),
        environment_id: Some(seed.get_initial_worker_metadata().environment_id.into()),
        ignore_already_existing: true,
        auth_ctx: Some(AuthCtx::System.into()),
        idempotency_key: Some(IdempotencyKey::fresh().into()),
        freshness_disposition: ProtoFreshness::KnownFresh as i32,
        ..Default::default()
    };
    let invalid = executor
        .client
        .clone()
        .prepare_worker(request.clone())
        .await?
        .into_inner();
    assert!(matches!(
        invalid.result,
        Some(create_worker_response::Result::Failure(_))
    ));
    assert_eq!(
        executor.oplog_service_call_count(&target, "create_fresh"),
        0
    );

    request.idempotency_key = Some(key.clone().into());
    let first = executor
        .client
        .clone()
        .prepare_worker(request.clone())
        .await?
        .into_inner();
    let fingerprint = match first.result {
        Some(create_worker_response::Result::Success(success)) => success.instance_id.unwrap(),
        other => anyhow::bail!("prepare_worker failed: {other:?}"),
    };
    assert_eq!(
        executor.oplog_service_call_count(&target, "create_fresh"),
        1
    );
    assert_eq!(executor.oplog_service_call_count(&target, "create"), 0);
    assert_eq!(executor.oplog_service_call_count(&target, "exists"), 0);
    let recorded = Worker::get_latest_metadata(
        seed.all(),
        &OwnedAgentId::new(seed.get_initial_worker_metadata().environment_id, &target),
    )
    .await?
    .expect("the prepared agent exists")
    .create_request;
    assert_eq!(recorded.kind, CreateRequestKind::Phantom);
    assert_eq!(recorded.idempotency_key, Some(key));

    request.freshness_disposition = ProtoFreshness::MayExist as i32;
    let retry = executor
        .client
        .clone()
        .prepare_worker(request)
        .await?
        .into_inner();
    let retry_fingerprint = match retry.result {
        Some(create_worker_response::Result::Success(success)) => success.instance_id.unwrap(),
        other => anyhow::bail!("prepare_worker retry failed: {other:?}"),
    };
    assert_eq!(retry_fingerprint, fingerprint);
    assert_eq!(
        executor.oplog_service_call_count(&target, "create_fresh"),
        1
    );
    assert_eq!(executor.oplog_service_call_count(&target, "create"), 0);

    // An RPC caller derives a fresh ephemeral target from the child key's owner key. When the
    // target lives on another executor, the remote preparation must pass that same check.
    let child_key = IdempotencyKey::fresh();
    let parsed = agent_id!("EphemeralCounter", "fresh-remote-rpc-preparation")
        .with_ephemeral_invocation_phantom(
            &golem_common::model::InvocationRequestKey::Internal(child_key.clone())
                .owner_key("increment"),
        )
        .map_err(anyhow::Error::msg)?;
    let rpc_target = AgentId::from_agent_id(seed.agent_id().component_id, &parsed)
        .map_err(anyhow::Error::msg)?;
    let metadata = seed.get_initial_worker_metadata();
    let remote = crate::fork::remote_rpc_preparing_on(
        &executor,
        metadata.environment_id,
        metadata.created_by,
        seed.all().worker_proxy(),
        seed.all().shard_service(),
    );
    remote
        .create_demand(
            AgentTarget::Current(rpc_target.clone()),
            metadata.environment_id,
            "increment",
            metadata.created_by,
            &seed.agent_id(),
            &[],
            InvocationContextStack::fresh(),
            vec![],
            &AuthCtx::System,
            Some(child_key),
            InvocationFreshnessDisposition::KnownFresh,
        )
        .await
        .map_err(|error| anyhow::anyhow!("remote KnownFresh preparation failed: {error}"))?;
    assert_eq!(
        executor.oplog_service_call_count(&rpc_target, "create_fresh"),
        1
    );
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn grpc_create_records_its_kind_key_and_principal(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        CreateWorkerRequest, create_worker_response,
    };
    let (executor, seed, _) = setup(last_unique_id, deps, component).await?;
    let metadata = seed.get_initial_worker_metadata();
    let principal = Principal::Agent(AgentPrincipal {
        agent_id: seed.agent_id(),
    });
    for (name, ignore_already_existing, kind) in [
        ("exclusive-create", false, CreateRequestKind::Exclusive),
        ("joining-create", true, CreateRequestKind::GetOrCreate),
    ] {
        let id = target(&seed, name);
        let key = IdempotencyKey::fresh();
        let response = executor
            .client
            .clone()
            .create_worker(CreateWorkerRequest {
                agent_id: Some(id.agent_id.clone().into()),
                component_owner_account_id: Some(metadata.created_by.into()),
                environment_id: Some(metadata.environment_id.into()),
                ignore_already_existing,
                auth_ctx: Some(AuthCtx::System.into()),
                principal: Some(principal.clone().into()),
                idempotency_key: Some(key.clone().into()),
                ..Default::default()
            })
            .await?
            .into_inner();
        assert!(
            matches!(
                response.result,
                Some(create_worker_response::Result::Success(_))
            ),
            "{name}: {response:?}"
        );
        let requests = executor
            .stored_oplog(&id.agent_id)
            .await
            .into_iter()
            .filter_map(|entry| match entry {
                OplogEntry::Create { parameters, .. } => Some(parameters.create_request),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(requests.len(), 1, "{name}");
        assert_eq!(requests[0].kind, kind, "{name}");
        assert_eq!(requests[0].idempotency_key, Some(key), "{name}");
        assert_eq!(requests[0].principal, principal, "{name}");
    }
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn grpc_create_waits_for_prepared_constructor(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        CreateWorkerRequest, create_worker_response,
    };
    use golem_common::model::{AgentEvent, PromiseId};

    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    let stored = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let seed = seed_worker(&executor, &stored).await?;
    let name = agent_id!("ConstructorProbe", "grpc-constructor-gate", false, true);
    let id =
        AgentId::from_agent_id(seed.agent_id().component_id, &name).map_err(anyhow::Error::msg)?;
    let metadata = seed.get_initial_worker_metadata();
    let request = CreateWorkerRequest {
        agent_id: Some(id.clone().into()),
        component_owner_account_id: Some(metadata.created_by.into()),
        environment_id: Some(metadata.environment_id.into()),
        ignore_already_existing: true,
        auth_ctx: Some(AuthCtx::System.into()),
        idempotency_key: Some(IdempotencyKey::fresh().into()),
        ..Default::default()
    };
    let prepared = executor
        .client
        .clone()
        .prepare_worker(request.clone())
        .await?
        .into_inner();
    assert!(matches!(
        prepared.result,
        Some(create_worker_response::Result::Success(_))
    ));
    let mut events = executor.capture_output(&id).await?;
    let mut client = executor.client.clone();
    let create = client.create_worker(request.clone());
    tokio::pin!(create);
    let mut stdout = String::new();
    let promise_index = loop {
        tokio::select! {
            biased;
            response = &mut create => anyhow::bail!("create returned before its constructor gate: {:?}", response?),
            event = events.recv() => {
                let event = event.ok_or_else(|| anyhow::anyhow!("constructor output closed"))?;
                if let AgentEvent::StdOut { bytes, .. } = AgentEvent::try_from(event).map_err(anyhow::Error::msg)? {
                    stdout.push_str(&String::from_utf8(bytes)?);
                    if let Some(index) = stdout.lines().find_map(|line| line.strip_prefix("constructor-probe:promise:").and_then(|s| s.parse::<u64>().ok())) {
                        break index;
                    }
                }
            }
        }
    };
    assert!(
        poll!(create.as_mut()).is_pending(),
        "create returned before promise completion"
    );
    executor
        .complete_promise(
            &PromiseId {
                agent_id: id.clone(),
                fingerprint: executor.get_worker_metadata(&id).await?.fingerprint,
                oplog_idx: OplogIndex::from_u64(promise_index),
            },
            vec![],
        )
        .await?;
    let created = create.await?.into_inner();
    let fingerprint = match created.result {
        Some(create_worker_response::Result::Success(success)) => success.instance_id,
        other => anyhow::bail!("constructor completion failed: {other:?}"),
    };
    let retried = executor
        .client
        .clone()
        .create_worker(request.clone())
        .await?
        .into_inner();
    assert!(
        matches!(retried.result, Some(create_worker_response::Result::Success(success)) if success.instance_id == fingerprint)
    );
    let history = executor.stored_oplog(&id).await;
    assert_eq!(
        history
            .iter()
            .filter(|entry| matches!(entry, OplogEntry::AgentInvocationStarted { .. }))
            .count(),
        1
    );
    assert_eq!(
        history
            .iter()
            .filter(|entry| matches!(entry, OplogEntry::PendingAgentInvocation { .. }))
            .count(),
        1
    );
    assert_eq!(
        history
            .iter()
            .filter(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { .. }))
            .count(),
        1
    );
    let context = executor.context.clone();
    drop(events);
    drop(seed);
    drop(executor);

    let load = Arc::new(crate::active_agents::ComponentLoadGate::new());
    load.arm();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            wrap_component_service: Some(Arc::new({
                let load = load.clone();
                move |inner| {
                    Arc::new(crate::active_agents::GatedComponentService {
                        inner,
                        gate: load.clone(),
                    })
                }
            })),
            ..Default::default()
        },
    )
    .await?;
    // Start a real reconstruction, but withhold the component needed to build its Store.
    // Create observes the durable constructor result, not the runtime's readiness.
    executor.resume(&id, false).await?;
    tokio::time::timeout(Duration::from_secs(20), load.entered()).await?;
    let before_create = executor.stored_oplog(&id).await;
    let retried = tokio::time::timeout(
        Duration::from_secs(20),
        executor.client.clone().create_worker(request),
    )
    .await??
    .into_inner();
    assert!(
        matches!(retried.result, Some(create_worker_response::Result::Success(success)) if success.instance_id == fingerprint)
    );
    assert_eq!(executor.stored_oplog(&id).await, before_create);
    load.release();
    let result = executor
        .invoke_and_await_agent(&stored, &name, "increment", data_value!())
        .await?;
    assert_eq!(result.into_typed::<u32>()?, 1);
    let settled = executor.stored_oplog(&id).await;
    assert_eq!(
        settled
            .iter()
            .filter(|entry| matches!(entry, OplogEntry::PendingAgentInvocation { .. }))
            .count(),
        2
    );
    assert_eq!(
        settled
            .iter()
            .filter(|entry| matches!(entry, OplogEntry::AgentInvocationStarted { .. }))
            .count(),
        2
    );
    assert_eq!(
        settled
            .iter()
            .filter(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { .. }))
            .count(),
        2
    );
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn grpc_create_keeps_startup_failure_out_of_constructor_results(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        CreateWorkerRequest, create_worker_response,
    };
    use golem_common::model::oplog::OplogErrorKind;

    let context = TestContext::new(last_unique_id);
    let load = Arc::new(crate::active_agents::ComponentLoadGate::new());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            wrap_component_service: Some(Arc::new({
                let load = load.clone();
                move |inner| {
                    Arc::new(crate::active_agents::GatedComponentService {
                        inner,
                        gate: load.clone(),
                    })
                }
            })),
            ..Default::default()
        },
    )
    .await?;
    let stored = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let id = AgentId::from_agent_id(stored.id, &agent_id!("Counter", "grpc-startup-failure"))
        .map_err(anyhow::Error::msg)?;
    let owned = OwnedAgentId::new(context.default_environment_id, &id);
    let request = CreateWorkerRequest {
        agent_id: Some(id.clone().into()),
        component_owner_account_id: Some(context.account_id.into()),
        environment_id: Some(context.default_environment_id.into()),
        ignore_already_existing: true,
        auth_ctx: Some(AuthCtx::System.into()),
        idempotency_key: Some(IdempotencyKey::fresh().into()),
        ..Default::default()
    };
    // Acquisition and constructor reservation succeed before the runtime's component get fails.
    let prepared = executor
        .client
        .clone()
        .prepare_worker(request.clone())
        .await?
        .into_inner();
    assert!(matches!(
        prepared.result,
        Some(create_worker_response::Result::Success(_))
    ));
    let worker = executor
        .active_agent(&owned)
        .await
        .expect("prepare retains the agent in the test-context cache")
        .primary();
    let revision = worker
        .get_initial_worker_metadata()
        .last_known_status
        .component_revision;
    let pending = executor.stored_oplog(&id).await;
    let keys = pending
        .iter()
        .filter_map(|entry| match entry {
            OplogEntry::PendingAgentInvocation { invocation_id, .. } => Some(*invocation_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(keys.len(), 1);
    assert!(
        !pending
            .iter()
            .any(|entry| matches!(entry, OplogEntry::AgentInvocationStarted { .. }))
    );
    let injected = WorkerExecutorError::ComponentParseFailed {
        component_id: stored.id,
        component_revision: revision,
        reason: "injected runtime startup parse failure".into(),
    };
    load.fail_next(injected.clone());
    Worker::start_if_needed(worker.clone()).await?;
    tokio::time::timeout(Duration::from_secs(20), load.entered()).await?;
    load.release();

    // Stop cleanup caches an operational error under the pending constructor's key. It is not
    // a durable constructor result, even though the warm lookup reports a complete failure.
    let cached = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let LookupResult::Complete(Err(error)) =
                worker.lookup_invocation_result(&keys[0]).await
            {
                break error;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    assert!(matches!(
        cached,
        WorkerExecutorError::InvocationFailed { .. }
    ));
    assert!(
        cached.to_string().contains(&injected.to_string()),
        "{cached}"
    );
    assert!(executor.worker_is_cached(&owned).await);
    assert!(
        worker
            .get_last_known_status()
            .await
            .invocation_results
            .get(&keys[0])
            .is_none(),
        "the synthetic warm failure must not have a durable constructor result"
    );
    let response = tokio::time::timeout(
        Duration::from_secs(20),
        executor.client.clone().create_worker(request),
    )
    .await??
    .into_inner();
    let error = match response.result {
        Some(create_worker_response::Result::Failure(error)) => {
            WorkerExecutorError::try_from(error).map_err(anyhow::Error::msg)?
        }
        other => anyhow::bail!("startup failure returned success: {other:?}"),
    };
    assert!(
        matches!(
            error,
            WorkerExecutorError::ComponentParseFailed { .. }
                | WorkerExecutorError::InvocationFailed { .. }
                | WorkerExecutorError::FailedToResumeAgent { .. }
        ),
        "startup failure became a constructor failure: {error:?}"
    );
    assert!(error.to_string().contains(&injected.to_string()), "{error}");
    let history = executor.stored_oplog(&id).await;
    assert!(history.iter().any(|entry| matches!(
        entry,
        OplogEntry::Error {
            kind: OplogErrorKind::Recovery,
            ..
        }
    )));
    assert!(!history.iter().any(|entry| matches!(
        entry,
        OplogEntry::AgentInvocationStarted { .. } | OplogEntry::AgentInvocationFinished { .. }
    )));
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn grpc_create_reports_recorded_constructor_failure(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        CreateWorkerRequest, create_worker_response,
    };
    use golem_common::model::AgentStatus;

    let (mut executor, seed, _) = setup(last_unique_id, deps, component).await?;
    let name = agent_id!("ConstructorProbe", "grpc-constructor-failure", true, false);
    let id =
        AgentId::from_agent_id(seed.agent_id().component_id, &name).map_err(anyhow::Error::msg)?;
    let metadata = seed.get_initial_worker_metadata();
    let request = CreateWorkerRequest {
        agent_id: Some(id.clone().into()),
        component_owner_account_id: Some(metadata.created_by.into()),
        environment_id: Some(metadata.environment_id.into()),
        ignore_already_existing: true,
        auth_ctx: Some(AuthCtx::System.into()),
        idempotency_key: Some(IdempotencyKey::fresh().into()),
        ..Default::default()
    };
    let context = executor.context.clone();
    drop(seed);
    let mut first_error = None;
    let mut recorded_history = None;
    for attempt in 0..3 {
        if attempt == 2 {
            drop(executor);
            executor =
                start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
        }
        let result = executor
            .client
            .clone()
            .create_worker(request.clone())
            .await?
            .into_inner();
        let error = match result.result {
            Some(create_worker_response::Result::Failure(error)) => {
                WorkerExecutorError::try_from(error).map_err(anyhow::Error::msg)?
            }
            other => anyhow::bail!("failed constructor returned success: {other:?}"),
        };
        match &error {
            WorkerExecutorError::AgentCreationFailed { agent_id, details } => {
                assert_eq!(agent_id, &id);
                assert!(
                    details.contains("constructor-probe:rejected:grpc-constructor-failure"),
                    "{details}"
                );
            }
            other => anyhow::bail!("constructor failure lost its create context: {other:?}"),
        }
        if let Some(expected) = &first_error {
            assert_eq!(&error, expected);
        } else {
            first_error = Some(error);
        }
        executor
            .wait_for_status(&id, AgentStatus::Failed, Duration::from_secs(20))
            .await?;
        let history = executor.stored_oplog(&id).await;
        if let Some(expected) = &recorded_history {
            assert_eq!(
                &history, expected,
                "create attempt {attempt} changed history"
            );
        } else {
            recorded_history = Some(history);
        }
    }
    executor
        .wait_for_status(&id, AgentStatus::Failed, Duration::from_secs(20))
        .await?;
    let history = executor.stored_oplog(&id).await;
    assert_eq!(
        history
            .iter()
            .filter(|entry| matches!(entry, OplogEntry::AgentInvocationStarted { .. }))
            .count(),
        1
    );
    assert_eq!(
        history
            .iter()
            .filter(|entry| matches!(entry, OplogEntry::PendingAgentInvocation { .. }))
            .count(),
        1
    );
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn grpc_create_keeps_constructor_success_after_a_later_method_failure(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        CreateWorkerRequest, create_worker_response,
    };
    use golem_common::model::AgentStatus;

    let context = TestContext::new(last_unique_id);
    let mut executor =
        start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let name = agent_id!("FailingCounter", "constructor-outcome-after-method-failure");
    let id = AgentId::from_agent_id(component.id, &name).map_err(anyhow::Error::msg)?;
    let request = CreateWorkerRequest {
        agent_id: Some(id.clone().into()),
        component_owner_account_id: Some(context.account_id.into()),
        environment_id: Some(context.default_environment_id.into()),
        ignore_already_existing: true,
        auth_ctx: Some(AuthCtx::System.into()),
        idempotency_key: Some(IdempotencyKey::fresh().into()),
        ..Default::default()
    };
    let response = executor
        .client
        .clone()
        .create_worker(request.clone())
        .await?
        .into_inner();
    let fingerprint = match response.result {
        Some(create_worker_response::Result::Success(success)) => success.instance_id,
        other => anyhow::bail!("initial construction did not succeed: {other:?}"),
    };
    let failure = executor
        .invoke_and_await_agent(&component, &name, "add", data_value!(99_u64))
        .await;
    assert!(failure.is_err(), "the control method must fail");
    executor
        .wait_for_status(&id, AgentStatus::Failed, Duration::from_secs(20))
        .await?;
    let settled = executor.stored_oplog(&id).await;
    for cold in [false, true] {
        if cold {
            executor.shutdown_and_wait_for_invocation_loops().await?;
            drop(executor);
            executor =
                start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
        }
        let response = executor
            .client
            .clone()
            .create_worker(request.clone())
            .await?
            .into_inner();
        assert!(
            matches!(response.result, Some(create_worker_response::Result::Success(success))
            if success.instance_id == fingerprint),
            "a later method failure replaced constructor success (cold={cold})"
        );
        assert_eq!(executor.stored_oplog(&id).await, settled);
    }
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn grpc_create_reports_constructor_exit_without_restarting_it(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        CreateWorkerRequest, create_worker_response,
    };

    let context = TestContext::new(last_unique_id);
    let mut executor =
        start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let name = agent_id!("ConstructorExitProbe", "constructor-exit");
    let id = AgentId::from_agent_id(component.id, &name).map_err(anyhow::Error::msg)?;
    let request = CreateWorkerRequest {
        agent_id: Some(id.clone().into()),
        component_owner_account_id: Some(context.account_id.into()),
        environment_id: Some(context.default_environment_id.into()),
        ignore_already_existing: true,
        auth_ctx: Some(AuthCtx::System.into()),
        idempotency_key: Some(IdempotencyKey::fresh().into()),
        ..Default::default()
    };
    let mut history = None;
    for cold in [false, true] {
        if cold {
            executor.shutdown_and_wait_for_invocation_loops().await?;
            drop(executor);
            executor =
                start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
        }
        let response = executor
            .client
            .clone()
            .create_worker(request.clone())
            .await?
            .into_inner();
        let error = match response.result {
            Some(create_worker_response::Result::Failure(error)) => {
                WorkerExecutorError::try_from(error).map_err(anyhow::Error::msg)?
            }
            other => anyhow::bail!("constructor exit returned success: {other:?}"),
        };
        assert!(
            matches!(&error, WorkerExecutorError::AgentCreationFailed { agent_id, details }
            if agent_id == &id && details.contains("Constructor exited")),
            "{error:?}"
        );
        let recorded = executor.stored_oplog(&id).await;
        if let Some(history) = &history {
            assert_eq!(&recorded, history);
        } else {
            assert_eq!(
                recorded
                    .iter()
                    .filter(|entry| matches!(entry, OplogEntry::AgentInvocationStarted { .. }))
                    .count(),
                1
            );
            history = Some(recorded);
        }
    }
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn grpc_create_preserves_interrupted_constructor_retry_across_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        CreateWorkerRequest, CreateWorkerResponse, create_worker_response,
    };
    use golem_common::model::AgentStatus;
    use golem_common::model::oplog::{AgentError, OplogErrorKind};
    use golem_service_base::error::worker_executor::InterruptKind;

    let context = TestContext::new(last_unique_id);
    let faults = KeyValueStorageFaults::default();
    let mut options = overrides(&faults, Duration::from_millis(50));
    options.configure = Some(Arc::new(|config| {
        config.max_in_function_retry_delay = Duration::ZERO;
        config.retry.max_attempts = 5;
        config.retry.min_delay = Duration::from_secs(3600);
        config.retry.max_delay = Duration::from_secs(3600);
        config.retry.max_jitter_factor = None;
        config.agent_status_flush.enabled = false;
    }));
    let executor = start_with_overrides(deps, &context, options.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let name = agent_id!(
        "ConstructorProbe",
        "grpc-constructor-interrupted-retry",
        false,
        true
    );
    let id = AgentId::from_agent_id(component.id, &name).map_err(anyhow::Error::msg)?;
    let owned = OwnedAgentId::new(context.default_environment_id, &id);
    let request = CreateWorkerRequest {
        agent_id: Some(id.clone().into()),
        component_owner_account_id: Some(context.account_id.into()),
        environment_id: Some(context.default_environment_id.into()),
        ignore_already_existing: true,
        auth_ctx: Some(AuthCtx::System.into()),
        idempotency_key: Some(IdempotencyKey::fresh().into()),
        ..Default::default()
    };
    let prepared = executor
        .client
        .clone()
        .prepare_worker(request.clone())
        .await?
        .into_inner();
    assert!(matches!(
        prepared.result,
        Some(create_worker_response::Result::Success(_))
    ));

    // Only the constructor's promise creation reaches this label after preparation.
    let promise = faults.gate_next(
        "create",
        KeyValueStorageError::Other("constructor promise storage outage".into()),
    );
    let creating = tokio::spawn({
        let mut client = executor.client.clone();
        let request = request.clone();
        async move { client.create_worker(request).await }
    });
    tokio::time::timeout(Duration::from_secs(20), promise.entered()).await?;
    assert!(!creating.is_finished());
    promise.release();
    executor
        .wait_for_status(&id, AgentStatus::Retrying, Duration::from_secs(20))
        .await?;
    let worker = executor
        .active_agent(&owned)
        .await
        .expect("the constructor retry is resident")
        .primary();
    let status = worker.get_last_known_status().await;
    let history = executor.stored_oplog(&id).await;
    let init_key = history
        .iter()
        .find_map(|entry| match entry {
            OplogEntry::PendingAgentInvocation { invocation_id, .. } => Some(*invocation_id),
            _ => None,
        })
        .expect("the constructor reservation must be recorded");
    assert_eq!(status.current_idempotency_key.as_ref(), Some(&init_key));
    let error_index = *status
        .invocation_results
        .get(&init_key)
        .expect("the retryable invocation error must be indexed");
    assert!(matches!(
        &history[error_index.as_u64() as usize - 1],
        OplogEntry::Error {
            kind: OplogErrorKind::Invocation,
            error: AgentError::Unknown(details),
            ..
        } if details.contains("constructor promise storage outage")
    ));
    assert!(
        !creating.is_finished(),
        "a retryable error must not settle create"
    );
    executor.interrupt(&id).await?;
    let response = tokio::time::timeout(Duration::from_secs(20), creating)
        .await???
        .into_inner();
    let assert_interrupted = |response: CreateWorkerResponse| -> anyhow::Result<()> {
        let error = match response.result {
            Some(create_worker_response::Result::Failure(error)) => {
                WorkerExecutorError::try_from(error).map_err(anyhow::Error::msg)?
            }
            other => anyhow::bail!("interrupted constructor returned {other:?}"),
        };
        assert!(
            matches!(
                error,
                WorkerExecutorError::Interrupted {
                    kind: InterruptKind::Interrupt(_)
                }
            ),
            "create lost the explicit interruption: {error:?}"
        );
        Ok(())
    };
    assert_interrupted(response)?;
    executor
        .wait_for_status(&id, AgentStatus::Interrupted, Duration::from_secs(20))
        .await?;
    let status = worker.get_last_known_status().await;
    assert_eq!(status.status, AgentStatus::Interrupted);
    assert_eq!(status.current_idempotency_key.as_ref(), Some(&init_key));
    assert_eq!(status.invocation_results.get(&init_key), Some(&error_index));
    let interrupted = executor.stored_oplog(&id).await;
    assert!(
        !interrupted
            .iter()
            .any(|entry| matches!(entry, OplogEntry::Resumed { .. }))
    );
    assert_eq!(
        interrupted
            .iter()
            .filter(|entry| matches!(entry, OplogEntry::AgentInvocationStarted { .. }))
            .count(),
        1
    );
    assert_eq!(
        interrupted
            .iter()
            .filter(|entry| matches!(entry, OplogEntry::PendingAgentInvocation { .. }))
            .count(),
        1
    );
    drop(status);
    drop(worker);
    drop(executor);

    let executor = start_with_overrides(deps, &context, options).await?;
    let response = tokio::time::timeout(
        Duration::from_secs(20),
        executor.client.clone().create_worker(request),
    )
    .await??
    .into_inner();
    assert_interrupted(response)?;
    assert_eq!(executor.stored_oplog(&id).await, interrupted);
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn grpc_create_joins_an_initialized_fork_with_the_source_constructor_key(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        CreateWorkerRequest, create_worker_response,
    };

    let context = TestContext::new(last_unique_id);
    let executor = crate::fork::start_with_local_resume(deps, &context, false).await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let source = agent_id!("ConstructorProbe", "grpc-constructor-fork", false, false);
    let source_id = executor.start_agent(&component.id, source.clone()).await?;
    let prefix = executor.stored_oplog(&source_id).await;
    let cut = prefix
        .iter()
        .rposition(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { .. }))
        .map(|index| OplogIndex::from_u64(index as u64 + 1))
        .expect("the source constructor completed");
    let fork = golem_common::phantom_agent_id!(
        "ConstructorProbe",
        uuid::Uuid::new_v4(),
        "grpc-constructor-fork",
        false,
        false
    );
    let fork_id = AgentId::from_agent_id(component.id, &fork).map_err(anyhow::Error::msg)?;
    executor
        .fork_worker(&source_id, &fork.to_string(), cut)
        .await?;
    let result = executor
        .client
        .clone()
        .create_worker(CreateWorkerRequest {
            agent_id: Some(fork_id.clone().into()),
            component_owner_account_id: Some(context.account_id.into()),
            environment_id: Some(context.default_environment_id.into()),
            ignore_already_existing: true,
            auth_ctx: Some(AuthCtx::System.into()),
            idempotency_key: Some(IdempotencyKey::fresh().into()),
            ..Default::default()
        })
        .await?
        .into_inner();
    assert!(
        matches!(
            result.result,
            Some(create_worker_response::Result::Success(_))
        ),
        "create must observe the fork's retained initialization: {result:?}"
    );
    let fork_history = executor.stored_oplog(&fork_id).await;
    let source_initialization = prefix
        .iter()
        .find_map(|entry| match entry {
            OplogEntry::PendingAgentInvocation { invocation_id, .. } => Some(invocation_id),
            _ => None,
        })
        .expect("the source constructor reservation must be retained");
    for history in [&prefix, &fork_history] {
        let pending = history
            .iter()
            .filter_map(|entry| match entry {
                OplogEntry::PendingAgentInvocation { invocation_id, .. } => Some(invocation_id),
                _ => None,
            })
            .collect::<Vec<_>>();
        let started = history
            .iter()
            .filter_map(|entry| match entry {
                OplogEntry::AgentInvocationStarted { invocation_id, .. } => Some(invocation_id),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(pending, vec![source_initialization]);
        assert_eq!(started, pending);
        assert_eq!(
            history
                .iter()
                .filter(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { .. }))
                .count(),
            1
        );
    }
    let result = executor
        .invoke_and_await_agent(&component, &fork, "increment", data_value!())
        .await?;
    assert_eq!(result.into_typed::<u32>()?, 1);
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn rpc_create_waits_for_constructor_before_returning_the_child(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_common::model::{AgentEvent, PromiseId};

    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    let stored = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let parent = agent_id!("ConstructorProbeParent", "rpc-constructor-gate-parent");
    executor.start_agent(&stored.id, parent.clone()).await?;
    let child = AgentId::from_agent_id(
        stored.id,
        &agent_id!("ConstructorProbe", "rpc-constructor-gate", false, true),
    )
    .map_err(anyhow::Error::msg)?;
    let create = executor.invoke_and_await_agent(
        &stored,
        &parent,
        "create_child",
        data_value!("rpc-constructor-gate", false, true),
    );
    tokio::pin!(create);
    let stored_child = async {
        loop {
            if executor.get_worker_metadata_opt(&child).await?.is_some() {
                return Ok::<_, anyhow::Error>(());
            }
        }
    };
    tokio::select! {
        biased;
        response = &mut create => anyhow::bail!("parent returned before child construction: {response:?}"),
        result = stored_child => result?,
    }
    let mut output = tokio::select! {
        biased;
        response = &mut create => anyhow::bail!("parent returned before its child constructor gate: {response:?}"),
        output = executor.capture_output(&child) => output?,
    };
    let mut stdout = String::new();
    let promise_index = loop {
        tokio::select! {
            biased;
            response = &mut create => anyhow::bail!("parent returned before child promise completion: {response:?}"),
            event = output.recv() => {
                let event = event.ok_or_else(|| anyhow::anyhow!("child output closed"))?;
                if let AgentEvent::StdOut { bytes, .. } = AgentEvent::try_from(event).map_err(anyhow::Error::msg)? {
                    stdout.push_str(&String::from_utf8(bytes)?);
                    if let Some(index) = stdout.lines().find_map(|line| {
                        line.strip_prefix("constructor-probe:promise:")
                            .and_then(|index| index.parse::<u64>().ok())
                    }) {
                        break index;
                    }
                }
            }
        }
    };
    assert!(poll!(create.as_mut()).is_pending());
    executor
        .complete_promise(
            &PromiseId {
                agent_id: child.clone(),
                fingerprint: executor.get_worker_metadata(&child).await?.fingerprint,
                oplog_idx: OplogIndex::from_u64(promise_index),
            },
            vec![],
        )
        .await?;
    assert_eq!(create.await?.into_typed::<String>()?, child.agent_id);
    let history = executor.stored_oplog(&child).await;
    assert_eq!(
        history
            .iter()
            .filter(|entry| matches!(entry, OplogEntry::AgentInvocationStarted { .. }))
            .count(),
        1,
        "client creation must execute only the constructor"
    );
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn rpc_create_reports_constructor_failure_before_a_child_method(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let parent = agent_id!("ConstructorProbeParent", "rpc-constructor-failure-parent");
    executor.start_agent(&component.id, parent.clone()).await?;
    let result = executor
        .invoke_and_await_agent(
            &component,
            &parent,
            "create_child",
            data_value!("rpc-constructor-failure".to_string(), true, false),
        )
        .await;
    let error = result
        .expect_err("creating a child with a failed constructor must fail before any child method");
    assert!(
        error
            .to_string()
            .contains("constructor-probe:rejected:rpc-constructor-failure"),
        "{error}"
    );
    let child = AgentId::from_agent_id(
        component.id,
        &agent_id!("ConstructorProbe", "rpc-constructor-failure", true, false),
    )
    .map_err(anyhow::Error::msg)?;
    executor
        .wait_for_status(
            &child,
            golem_common::model::AgentStatus::Failed,
            Duration::from_secs(20),
        )
        .await?;
    let history = executor.stored_oplog(&child).await;
    assert_eq!(
        history
            .iter()
            .filter(|entry| matches!(entry, OplogEntry::AgentInvocationStarted { .. }))
            .count(),
        1
    );
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn rpc_create_replays_completed_success_and_failure_without_contacting_the_child(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_common::model::{AgentEvent, PromiseId};
    use golem_worker_executor_test_utils::RecordingRpc;
    use std::sync::OnceLock;
    use std::sync::atomic::Ordering;

    for fail in [false, true] {
        let context = TestContext::new(last_unique_id);
        let executor =
            start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
        let stored = executor
            .component_dep(&context.default_environment_id, component)
            .store()
            .await?;
        let parent_name = agent_id!("ConstructorProbeParent", format!("recorded-create-{fail}"));
        let parent = executor
            .start_agent(&stored.id, parent_name.clone())
            .await?;
        let child_name = format!("recorded-child-{fail}");
        let child = AgentId::from_agent_id(
            stored.id,
            &agent_id!("ConstructorProbe", child_name.clone(), fail, false),
        )
        .map_err(anyhow::Error::msg)?;
        let mut events = executor.capture_output(&parent).await?;
        let key = IdempotencyKey::fresh();
        executor
            .invoke_agent_with_key(
                &stored,
                &parent_name,
                &key,
                "create_child_result_then_wait",
                data_value!(child_name.clone(), fail),
            )
            .await?;
        let mut stdout = String::new();
        let promise_index = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let event = events
                    .recv()
                    .await
                    .ok_or_else(|| anyhow::anyhow!("parent output closed"))?;
                if let AgentEvent::StdOut { bytes, .. } =
                    AgentEvent::try_from(event).map_err(anyhow::Error::msg)?
                {
                    stdout.push_str(&String::from_utf8(bytes)?);
                    if let Some(index) = stdout.lines().find_map(|line| {
                        line.strip_prefix("constructor-parent:promise:")
                            .and_then(|s| s.parse::<u64>().ok())
                    }) {
                        break Ok::<_, anyhow::Error>(index);
                    }
                }
            }
        })
        .await??;
        // Suspension commits the promise before the process is stopped, so its identity survives.
        executor
            .wait_for_status(
                &parent,
                golem_common::model::AgentStatus::Suspended,
                Duration::from_secs(30),
            )
            .await?;
        let child_history = executor.stored_oplog(&child).await;
        assert_eq!(
            child_history
                .iter()
                .filter(|entry| matches!(entry, OplogEntry::AgentInvocationStarted { .. }))
                .count(),
            1
        );
        executor.shutdown_and_wait_for_invocation_loops().await?;
        drop(events);
        drop(executor);

        let replay_call_count = Arc::new(OnceLock::new());
        let executor = start_with_overrides(
            deps,
            &context,
            TestExecutorOverrides {
                wrap_rpc: Some(Arc::new({
                    let replay_call_count = replay_call_count.clone();
                    move |rpc| {
                        let rpc = RecordingRpc::new(rpc, "", Arc::default());
                        replay_call_count.set(rpc.call_count()).unwrap();
                        Arc::new(rpc)
                    }
                })),
                ..Default::default()
            },
        )
        .await?;
        executor
            .complete_promise(
                &PromiseId {
                    agent_id: parent.clone(),
                    fingerprint: executor.get_worker_metadata(&parent).await?.fingerprint,
                    oplog_idx: OplogIndex::from_u64(promise_index),
                },
                vec![],
            )
            .await?;
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            executor.invoke_and_await_agent_with_key(
                &stored,
                &parent_name,
                &key,
                "create_child_result_then_wait",
                data_value!(child_name.clone(), fail),
            ),
        )
        .await??
        .into_typed::<Result<String, String>>()?;
        if fail {
            let error = result.expect_err("the recorded constructor rejection must survive replay");
            assert!(
                error.contains(&format!("constructor-probe:rejected:{child_name}")),
                "{error}"
            );
        } else {
            assert_eq!(result.map_err(anyhow::Error::msg)?, child.agent_id);
        }
        assert_eq!(executor.stored_oplog(&child).await, child_history);
        executor.shutdown_and_wait_for_invocation_loops().await?;
        assert_eq!(
            replay_call_count.get().unwrap().load(Ordering::Relaxed),
            0,
            "completed create replay must not contact the child (fail={fail})"
        );
        drop(executor);
    }
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn pre_create_single_flight_admits_each_caller_against_the_winner(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_worker_executor::worker::CreateIntent;

    let (executor, seed, faults) = setup(last_unique_id, deps, component).await?;
    for (scenario, first_exclusive, second_exclusive, keyed, identical) in [
        ("exclusive-exclusive", true, true, false, false),
        ("keyed-retry", true, true, true, true),
        ("keyed-conflict", true, true, true, false),
        ("exclusive-join", true, false, false, false),
        ("join-exclusive", false, true, false, false),
    ] {
        for reverse in [false, true] {
            let id = target(&seed, &format!("pre-create-{scenario}-{reverse}"));
            let first_value = if reverse { "right" } else { "left" };
            let second_value = if identical {
                first_value
            } else if reverse {
                "left"
            } else {
                "right"
            };
            let first_principal = if reverse {
                Principal::anonymous()
            } else {
                Principal::Agent(AgentPrincipal {
                    agent_id: seed.agent_id(),
                })
            };
            let key = keyed.then(IdempotencyKey::fresh);
            let create = |value: &'static str, exclusive: bool, principal: Principal| {
                let seed = seed.clone();
                let id = id.clone();
                let key = key.clone();
                async move {
                    Worker::get_or_create_suspended_with_freshness(
                        seed.all(),
                        &id,
                        Some(vec![("OWNER".into(), value.into())]),
                        vec![],
                        None,
                        None,
                        &InvocationContextStack::fresh(),
                        principal,
                        InvocationFreshnessDisposition::MayExist,
                        CreateIntent::Create {
                            exclusive,
                            idempotency_key: key,
                        },
                    )
                    .await
                }
            };
            let gate = faults.pause_next("read_cached_agent_mode");
            let mut first = Box::pin(create(
                first_value,
                first_exclusive,
                first_principal.clone(),
            ));
            assert!(poll!(&mut first).is_pending());
            gate.entered().await;
            assert!(executor.stored_oplog(&id.agent_id).await.is_empty());
            let second_principal = if identical {
                first_principal.clone()
            } else if reverse {
                Principal::Agent(AgentPrincipal {
                    agent_id: seed.agent_id(),
                })
            } else {
                Principal::anonymous()
            };
            let mut second = Box::pin(create(second_value, second_exclusive, second_principal));
            assert!(poll!(&mut second).is_pending());
            assert!(executor.stored_oplog(&id.agent_id).await.is_empty());
            gate.release();
            // Observe the follower before the construction leader's response.
            let second = second.await;
            let first = first.await?;
            let winner = first.get_initial_worker_metadata();
            match second {
                Ok(worker) => {
                    assert!(
                        !second_exclusive || identical,
                        "{scenario}, reverse={reverse}"
                    );
                    assert_eq!(
                        worker.get_initial_worker_metadata().fingerprint,
                        winner.fingerprint
                    );
                    assert_eq!(
                        worker
                            .get_initial_worker_metadata()
                            .create_request
                            .principal,
                        first_principal
                    );
                }
                Err(WorkerExecutorError::AgentAlreadyExists { .. }) => {
                    assert!(second_exclusive && !keyed, "{scenario}, reverse={reverse}");
                }
                Err(WorkerExecutorError::CreateIdempotencyKeyConflict { .. }) => {
                    assert!(
                        second_exclusive && keyed && !identical,
                        "{scenario}, reverse={reverse}"
                    );
                }
                Err(error) => anyhow::bail!("{scenario}, reverse={reverse}: {error:?}"),
            }
            let creates: Vec<_> = executor
                .stored_oplog(&id.agent_id)
                .await
                .into_iter()
                .filter_map(|entry| match entry {
                    OplogEntry::Create { parameters, .. } => Some(parameters),
                    _ => None,
                })
                .collect();
            assert_eq!(creates.len(), 1);
            assert_eq!(
                creates[0].env,
                vec![("OWNER".to_string(), first_value.to_string())]
            );
            assert_eq!(creates[0].create_request.principal, first_principal);
            assert_eq!(creates[0].create_request.idempotency_key, key);
            assert_eq!(
                creates[0].create_request.kind,
                if first_exclusive {
                    CreateRequestKind::Exclusive
                } else {
                    CreateRequestKind::GetOrCreate
                }
            );
            let joined = acquire(&seed, &id).await?;
            assert_eq!(
                joined.get_initial_worker_metadata().fingerprint,
                winner.fingerprint
            );
            assert_eq!(joined.get_initial_worker_metadata().env, winner.env);
            assert_eq!(
                joined
                    .get_initial_worker_metadata()
                    .create_request
                    .principal,
                first_principal
            );
        }
    }
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn grpc_create_reports_typed_conflicts_and_keeps_the_winner(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        CreateWorkerRequest, create_worker_response,
    };

    let (executor, seed, faults) = setup(last_unique_id, deps, component).await?;
    let metadata = seed.get_initial_worker_metadata();
    let id = target(&seed, "grpc-create-conflicts");
    let request = CreateWorkerRequest {
        agent_id: Some(id.agent_id.clone().into()),
        component_owner_account_id: Some(metadata.created_by.into()),
        environment_id: Some(metadata.environment_id.into()),
        auth_ctx: Some(AuthCtx::System.into()),
        idempotency_key: Some(IdempotencyKey::fresh().into()),
        env: [("OWNER".into(), "winner".into())].into(),
        ..Default::default()
    };
    let first = executor
        .client
        .clone()
        .prepare_worker(request.clone())
        .await?
        .into_inner();
    let fingerprint = match first.result {
        Some(create_worker_response::Result::Success(success)) => success.instance_id,
        other => anyhow::bail!("initial create failed: {other:?}"),
    };
    for cold in [false, true] {
        for case in ["same", "unkeyed", "other-key", "changed", "join"] {
            if cold {
                seed.active_agents().remove(&id).await;
            }
            let mut retry = request.clone();
            match case {
                "unkeyed" => retry.idempotency_key = None,
                "other-key" => retry.idempotency_key = Some(IdempotencyKey::fresh().into()),
                "changed" | "join" => {
                    retry.env.insert("OWNER".into(), "loser".into());
                    retry.ignore_already_existing = case == "join";
                }
                _ => {}
            }
            let response = executor
                .client
                .clone()
                .prepare_worker(retry)
                .await?
                .into_inner();
            match response.result {
                Some(create_worker_response::Result::Success(success)) => {
                    assert!(case == "same" || case == "join", "{case}, cold={cold}");
                    assert_eq!(success.instance_id, fingerprint);
                }
                Some(create_worker_response::Result::Failure(error)) => {
                    let error = WorkerExecutorError::try_from(error).map_err(anyhow::Error::msg)?;
                    assert!(
                        match error {
                            WorkerExecutorError::AgentAlreadyExists { .. } =>
                                case == "unkeyed" || case == "other-key",
                            WorkerExecutorError::CreateIdempotencyKeyConflict { .. } =>
                                case == "changed",
                            _ => false,
                        },
                        "{case}, cold={cold}"
                    );
                }
                None => anyhow::bail!("missing create result"),
            }
        }
    }
    let creates: Vec<_> = executor
        .stored_oplog(&id.agent_id)
        .await
        .into_iter()
        .filter_map(|entry| match entry {
            OplogEntry::Create { parameters, .. } => Some(parameters),
            _ => None,
        })
        .collect();
    assert_eq!(creates.len(), 1);
    assert_eq!(
        creates[0].env,
        vec![("OWNER".to_string(), "winner".to_string())]
    );
    let context = executor.context.clone();
    drop(seed);
    drop(executor);
    let executor = start_with_overrides(
        deps,
        &context,
        overrides(&faults, Duration::from_millis(50)),
    )
    .await?;
    let retry = executor
        .client
        .clone()
        .prepare_worker(request.clone())
        .await?
        .into_inner();
    match retry.result {
        Some(create_worker_response::Result::Success(success)) => {
            assert_eq!(success.instance_id, fingerprint)
        }
        other => anyhow::bail!("retry after restart failed: {other:?}"),
    }
    let mut changed = request;
    changed
        .env
        .insert("OWNER".into(), "after-restart-loser".into());
    let rejected = executor
        .client
        .clone()
        .prepare_worker(changed)
        .await?
        .into_inner();
    let Some(create_worker_response::Result::Failure(error)) = rejected.result else {
        anyhow::bail!("changed retry after restart succeeded");
    };
    assert!(matches!(
        WorkerExecutorError::try_from(error),
        Ok(WorkerExecutorError::CreateIdempotencyKeyConflict { .. })
    ));
    let after = executor.stored_oplog(&id.agent_id).await;
    assert_eq!(
        after
            .iter()
            .filter(|entry| matches!(entry, OplogEntry::Create { .. }))
            .count(),
        1
    );
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn exclusive_create_admission_checks_warm_and_cold_stored_identity(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_worker_executor::worker::CreateIntent;

    let (_executor, seed, _) = setup(last_unique_id, deps, component).await?;
    for keyed in [false, true] {
        let id = target(
            &seed,
            if keyed {
                "keyed-admission"
            } else {
                "unkeyed-admission"
            },
        );
        let key = keyed.then(IdempotencyKey::fresh);
        let create = |value: &'static str, exclusive: bool, key: Option<IdempotencyKey>| {
            let seed = seed.clone();
            let id = id.clone();
            async move {
                Worker::get_or_create_suspended_with_freshness(
                    seed.all(),
                    &id,
                    Some(vec![("OWNER".into(), value.into())]),
                    vec![],
                    None,
                    None,
                    &InvocationContextStack::fresh(),
                    Principal::anonymous(),
                    InvocationFreshnessDisposition::MayExist,
                    CreateIntent::Create {
                        exclusive,
                        idempotency_key: key,
                    },
                )
                .await
            }
        };
        let original = create("original", true, key.clone()).await?;
        let metadata = original.get_initial_worker_metadata();
        for cold in [false, true] {
            if cold {
                seed.active_agents().remove(&id).await;
            }
            let equal = create("original", true, key.clone()).await;
            if keyed {
                let equal = equal?;
                assert_eq!(
                    equal.get_initial_worker_metadata().fingerprint,
                    metadata.fingerprint
                );
                if !cold {
                    assert!(Arc::ptr_eq(&equal, &original));
                }
            } else {
                assert!(matches!(
                    equal,
                    Err(WorkerExecutorError::AgentAlreadyExists { .. })
                ));
            }
            let changed = create("changed", true, key.clone()).await;
            assert!(match changed {
                Err(WorkerExecutorError::CreateIdempotencyKeyConflict { .. }) => keyed,
                Err(WorkerExecutorError::AgentAlreadyExists { .. }) => !keyed,
                _ => false,
            });
            assert!(matches!(
                create("original", true, Some(IdempotencyKey::fresh())).await,
                Err(WorkerExecutorError::AgentAlreadyExists { .. })
            ));
            // A rejected caller cannot poison construction or replace the recorded overrides.
            let joined = create("ignored", false, Some(IdempotencyKey::fresh())).await?;
            let joined_metadata = joined.get_initial_worker_metadata();
            assert_eq!(joined_metadata.fingerprint, metadata.fingerprint);
            assert_eq!(joined_metadata.env, metadata.env);
            assert_eq!(
                joined_metadata.create_request.kind,
                metadata.create_request.kind
            );
            assert_eq!(
                joined_metadata.create_request.idempotency_key,
                metadata.create_request.idempotency_key
            );
            assert_eq!(
                joined_metadata.create_request.principal,
                metadata.create_request.principal
            );
        }
    }
    Ok(())
}

async fn setup(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    component: &PrecompiledComponent,
) -> anyhow::Result<(
    TestWorkerExecutor,
    Arc<Worker<TestWorkerCtx>>,
    KeyValueStorageFaults,
)> {
    setup_with_retry_interval(last_unique_id, deps, component, Duration::from_millis(50)).await
}

async fn setup_with_retry_interval(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    component: &PrecompiledComponent,
    retry_interval: Duration,
) -> anyhow::Result<(
    TestWorkerExecutor,
    Arc<Worker<TestWorkerCtx>>,
    KeyValueStorageFaults,
)> {
    let context = TestContext::new(last_unique_id);
    let faults = KeyValueStorageFaults::default();
    let executor = start_with_overrides(deps, &context, overrides(&faults, retry_interval)).await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let seed = seed_worker(&executor, &component).await?;
    Ok((executor, seed, faults))
}

fn overrides(faults: &KeyValueStorageFaults, retry_interval: Duration) -> TestExecutorOverrides {
    TestExecutorOverrides {
        configure: Some(Arc::new(move |config| {
            config.active_agents.ttl = CACHE_TTL;
            config.oplog.max_payload_size = 1;
            config.durable_stream.reconciliation_interval = retry_interval;
            config.agent_status_flush.enabled = false;
        })),
        wrap_key_value_storage: Some(Arc::new({
            let faults = faults.clone();
            move |storage| Arc::new(FaultInjectingKeyValueStorage::new(storage, faults.clone()))
        })),
        ..Default::default()
    }
}

/// Invokes the seed agent and returns its unloaded worker, whose dependencies give tests direct
/// access to the executor's acquisition API. Invocation also reacquires it after executor restart.
async fn seed_worker(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
) -> anyhow::Result<Arc<Worker<TestWorkerCtx>>> {
    let context = &executor.context;
    let name = agent_id!("Counter", "initialization-test-seed");
    let id = AgentId::from_agent_id(component.id, &name).map_err(anyhow::Error::msg)?;
    executor
        .invoke_and_await_agent(component, &name, "increment", data_value!())
        .await?;
    let seed = executor
        .active_agent(&OwnedAgentId::new(context.default_environment_id, &id))
        .await
        .unwrap()
        .primary();
    tokio::time::timeout(Duration::from_secs(20), async {
        while executor
            .worker_is_loaded(&OwnedAgentId::new(context.default_environment_id, &id))
            .await
        {
            seed.stop_if_idle().await;
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(seed)
}

fn target(seed: &Worker<TestWorkerCtx>, name: &str) -> OwnedAgentId {
    OwnedAgentId::new(
        seed.get_initial_worker_metadata().environment_id,
        &AgentId {
            component_id: seed.agent_id().component_id,
            agent_id: agent_id!("Counter", name.to_string()).to_string(),
        },
    )
}

async fn acquire(
    seed: &Arc<Worker<TestWorkerCtx>>,
    target: &OwnedAgentId,
) -> Result<Arc<Worker<TestWorkerCtx>>, WorkerExecutorError> {
    Worker::get_or_create_suspended(
        seed.all(),
        target,
        Some(vec![("OWNER".into(), "original".into())]),
        vec![],
        None,
        None,
        &InvocationContextStack::fresh(),
        Principal::anonymous(),
    )
    .await
}

async fn register_stream(worker: &Worker<TestWorkerCtx>) -> anyhow::Result<DurableStreamHandle> {
    let metadata = worker.get_initial_worker_metadata();
    let index = worker.oplog().current_oplog_index().await.next();
    let source_invocation = StreamInvocationId {
        callee_environment_id: metadata.environment_id,
        callee: metadata.agent_id.clone(),
        callee_fingerprint: metadata.fingerprint,
        idempotency_key: golem_common::model::InvocationId(uuid::Uuid::new_v4()),
    };
    let handle = DurableStreamHandle {
        format_version: 1,
        stream_id: StreamId::derive(
            metadata.environment_id,
            &metadata.agent_id,
            metadata.fingerprint,
            index,
        )?,
        producer_environment_id: metadata.environment_id,
        producer: metadata.agent_id,
        expected_producer_fingerprint: metadata.fingerprint,
        producer_generation: OplogIndex::NONE,
        source_invocation: source_invocation.clone(),
        component_revision: metadata.last_known_status.component_revision,
        element_schema_fingerprint: SchemaFingerprintV1([7; 32]),
    };
    assert_eq!(
        worker
            .add_and_commit_oplog(OplogEntry::StreamRegistered {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                summary: None,
                record: OplogPayload::Inline(Box::new(StreamRegisteredRecord {
                    format_version: 1,
                    coordinate: StreamRegistrationRecordCoordinate::Root {
                        invocation: StreamRegistrationInvocation::Local(
                            source_invocation.idempotency_key,
                        ),
                        root_kind: StreamRootKind::MethodResult,
                        recursive_value_path: vec![],
                    },
                    source_invocation: StreamRegistrationInvocation::Local(
                        handle.source_invocation.idempotency_key,
                    ),
                    component_revision: handle.component_revision,
                    element_schema_fingerprint: handle.element_schema_fingerprint,
                    source_kind: StreamSourceKind::InvocationOutput,
                    session_role: None,
                })),
            })
            .await
            .unwrap(),
        index
    );
    Ok(handle)
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn late_initialization_storage_failure_is_shared_then_retried(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let (executor, seed, faults) = setup(last_unique_id, deps, component).await?;
    let id = target(&seed, "late-storage-failure");
    let original = acquire(&seed, &id).await?;
    seed.active_agents().remove(&id).await;
    register_stream(&original).await?;
    let before = original.get_initial_worker_metadata();
    let status = faults.gate_next(
        "update_status",
        KeyValueStorageError::Other("old status write outage".into()),
    );
    let write = tokio::spawn({
        let original = original.clone();
        async move {
            original
                .add_and_commit_oplog(OplogEntry::grow_memory(0))
                .await
        }
    });
    status.entered().await;
    write.abort();
    assert!(write.await.unwrap_err().is_cancelled());
    drop(original);
    let failure = faults.gate_next(
        "lookup_producer",
        KeyValueStorageError::Other("late construction outage".into()),
    );
    let mut first = Box::pin(acquire(&seed, &id));
    assert!(poll!(&mut first).is_pending());
    failure.entered().await;
    assert!(!executor.worker_is_cached(&id).await);
    let mut waiters = vec![first];
    for _ in 0..3 {
        let mut waiter = Box::pin(acquire(&seed, &id));
        assert!(poll!(&mut waiter).is_pending());
        waiters.push(waiter);
    }
    failure.release();
    // Cancellation of the writer's caller must not let construction finish cleanup before
    // the old generation's actual status job completes.
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut waiters[0])
            .await
            .is_err()
    );
    let mut waiter = Box::pin(acquire(&seed, &id));
    assert!(poll!(&mut waiter).is_pending());
    waiters.push(waiter);
    status.release();
    let mut errors = Vec::new();
    for waiter in waiters {
        errors.push(waiter.await.err().expect("all original waiters must fail"));
    }
    assert!(errors[0].to_string().contains("late construction outage"));
    assert!(errors.iter().all(|error| error == &errors[0]));
    assert!(!executor.worker_is_cached(&id).await);

    let (retry, overlapping) = tokio::join!(acquire(&seed, &id), acquire(&seed, &id));
    let retry = retry?;
    assert!(Arc::ptr_eq(&retry, &overlapping?));
    let after = retry.get_initial_worker_metadata();
    assert_eq!(before.fingerprint, after.fingerprint);
    assert_eq!(before.env, after.env);
    assert_eq!(before.config, after.config);
    assert_eq!(
        before.last_known_status.component_revision,
        after.last_known_status.component_revision
    );
    let entries = retry
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            retry.oplog().current_oplog_index().await.as_u64(),
        )
        .await
        .unwrap();
    assert_eq!(
        entries
            .values()
            .filter(|entry| matches!(entry, OplogEntry::Create { .. }))
            .count(),
        1
    );
    assert_eq!(retry.pending_invocations().await.len(), 1);
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn exclusive_retry_after_partial_construction_uses_the_persisted_identity(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_worker_executor::worker::CreateIntent;

    let (executor, seed, faults) = setup(last_unique_id, deps, component).await?;
    for keyed in [false, true] {
        let id = target(
            &seed,
            if keyed {
                "partial-exclusive-keyed"
            } else {
                "partial-exclusive-unkeyed"
            },
        );
        let key = keyed.then(IdempotencyKey::fresh);
        let create = |env: &'static str| {
            let seed = seed.clone();
            let id = id.clone();
            let key = key.clone();
            async move {
                Worker::get_or_create_suspended_with_freshness(
                    seed.all(),
                    &id,
                    Some(vec![("OWNER".into(), env.into())]),
                    vec![],
                    None,
                    None,
                    &InvocationContextStack::fresh(),
                    Principal::anonymous(),
                    InvocationFreshnessDisposition::MayExist,
                    CreateIntent::Create {
                        exclusive: true,
                        idempotency_key: key,
                    },
                )
                .await
            }
        };
        faults.fail(
            "update_status",
            1,
            KeyValueStorageError::Other("post-Create outage".into()),
        );
        let error = create("original")
            .await
            .err()
            .expect("construction must fail after Create");
        assert!(error.to_string().contains("post-Create outage"));
        let recorded = Worker::get_latest_metadata(seed.all(), &id)
            .await?
            .expect("Create persisted");
        let changed = create("changed").await;
        assert!(match changed {
            Err(WorkerExecutorError::CreateIdempotencyKeyConflict { .. }) => keyed,
            Err(WorkerExecutorError::AgentAlreadyExists { .. }) => !keyed,
            _ => false,
        });
        let retry = create("original").await;
        if keyed {
            assert_eq!(
                retry?.get_initial_worker_metadata().fingerprint,
                recorded.fingerprint
            );
        } else {
            assert!(matches!(
                retry,
                Err(WorkerExecutorError::AgentAlreadyExists { .. })
            ));
        }
        let after = Worker::get_latest_metadata(seed.all(), &id).await?.unwrap();
        assert_eq!(after.fingerprint, recorded.fingerprint);
        assert_eq!(after.env, recorded.env);
        assert_eq!(
            executor
                .stored_oplog(&id.agent_id)
                .await
                .iter()
                .filter(|entry| matches!(entry, OplogEntry::Create { .. }))
                .count(),
            1
        );
    }
    Ok(())
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn partial_creation_reloads_identity_and_original_initialization(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let (executor, seed, faults) = setup(last_unique_id, deps, component).await?;
    let all = seed.all().clone();
    for agent_type in ["Counter", "EphemeralCounter"] {
        let mut id = target(&seed, "partial-creation");
        id.agent_id.agent_id = agent_id!(agent_type, "partial-creation").to_string();
        faults.fail(
            "read_cached_agent_mode",
            1,
            KeyValueStorageError::Other("before creation outage".into()),
        );
        assert!(
            acquire(&seed, &id)
                .await
                .err()
                .unwrap()
                .to_string()
                .contains("before creation outage")
        );
        assert!(Worker::get_latest_metadata(&all, &id).await?.is_none());
        let context = InvocationContextStack::fresh();
        let principal = Principal::Agent(AgentPrincipal {
            agent_id: seed.agent_id(),
        });
        let payload_directory = deps
            .blob_storage_root()
            .join("oplog_payload")
            .join("ephemeral")
            .join(id.environment_id.to_string())
            .join(agent_path_segment(&id.agent_id));
        let expected_error = if agent_type == "Counter" {
            faults.fail(
                "update_status",
                1,
                KeyValueStorageError::Other("initial status outage".into()),
            );
            "initial status outage"
        } else {
            tokio::fs::create_dir_all(payload_directory.parent().unwrap()).await?;
            tokio::fs::write(&payload_directory, b"block payload directory creation").await?;
            "Failed to upload invocation payload"
        };
        let failure = Worker::get_or_create_suspended(
            &all,
            &id,
            Some(vec![("OWNER".into(), "first-creator".into())]),
            vec![],
            None,
            None,
            &context,
            principal.clone(),
        )
        .await
        .err()
        .expect("creation must report the injected failure");
        assert!(failure.to_string().contains(expected_error));
        assert!(!executor.worker_is_cached(&id).await);
        let before = Worker::get_latest_metadata(&all, &id).await?.unwrap();
        assert_eq!(before.last_known_status.oplog_idx, OplogIndex::INITIAL);
        let sentinel_id = target(&seed, &format!("expiry-sentinel-{agent_type}"));
        let sentinel = acquire(&seed, &sentinel_id).await?;
        let sentinel_weak = Arc::downgrade(&sentinel);
        drop(sentinel);
        tokio::time::timeout(Duration::from_secs(20), async {
            while sentinel_weak.upgrade().is_some() {
                tokio::time::sleep(CACHE_TTL).await;
            }
        })
        .await?;
        if agent_type == "EphemeralCounter" {
            tokio::fs::remove_file(&payload_directory).await?;
        }

        // A different caller retries. Create and initialization provenance belong to the first one.
        let worker = acquire(&seed, &id).await?;
        let after = worker.get_initial_worker_metadata();
        assert_eq!(before.fingerprint, after.fingerprint);
        assert_eq!(before.env, after.env);
        assert_eq!(before.config, after.config);
        assert_eq!(
            before.last_known_status.component_revision,
            after.last_known_status.component_revision
        );
        let oplog = worker.oplog();
        let entries = oplog
            .read_exact(
                OplogIndex::INITIAL,
                oplog.current_oplog_index().await.as_u64(),
            )
            .await
            .unwrap();
        assert_eq!(
            entries
                .values()
                .filter(|entry| matches!(entry, OplogEntry::Create { .. }))
                .count(),
            1
        );
        let pending = entries
            .values()
            .filter_map(|entry| match entry {
                OplogEntry::PendingAgentInvocation {
                    payload, trace_id, ..
                } => Some((payload, trace_id)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(pending.len(), 1);
        let AgentInvocationPayload::AgentInitialization {
            principal: recorded,
            ..
        } = oplog
            .download_payload(pending[0].0.clone())
            .await
            .map_err(anyhow::Error::msg)?
        else {
            panic!("expected initialization payload")
        };
        assert_eq!(recorded, principal);
        assert_eq!(pending[0].1, &context.trace_id);
        if agent_type == "EphemeralCounter" {
            // No guest was admitted by the failed creation, so this is not a fail-stop reload.
            Worker::start_if_needed(worker.clone()).await?;
        }
    }
    Ok(())
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn restart_initializes_a_partially_created_agent_as_its_recorded_creator(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let faults = KeyValueStorageFaults::default();
    let executor = start_with_overrides(
        deps,
        &context,
        overrides(&faults, Duration::from_millis(50)),
    )
    .await?;
    let stored = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let seed = seed_worker(&executor, &stored).await?;
    let creator_context = InvocationContextStack::fresh();
    let creator = Principal::Agent(AgentPrincipal {
        agent_id: seed.agent_id(),
    });
    let loaders = ["get-or-create", "existing-only"];
    for loader in loaders {
        let id = target(&seed, &format!("restart-creator-{loader}"));
        faults.fail(
            "update_status",
            1,
            KeyValueStorageError::Other("initial status outage".into()),
        );
        let failure = Worker::get_or_create_suspended(
            seed.all(),
            &id,
            None,
            vec![],
            None,
            None,
            &creator_context,
            creator.clone(),
        )
        .await
        .err()
        .expect("creation must report the injected failure");
        assert!(
            failure.to_string().contains("initial status outage"),
            "{loader}"
        );
        assert_eq!(
            Worker::get_latest_metadata(seed.all(), &id)
                .await?
                .unwrap()
                .last_known_status
                .oplog_idx,
            OplogIndex::INITIAL,
            "{loader}"
        );
    }
    drop(seed);
    drop(executor);

    // The new executor has no resident state for these agents; only their Create entries record
    // the creating request. Each agent is loaded first by a different anonymous request.
    let executor = start_with_overrides(
        deps,
        &context,
        overrides(&faults, Duration::from_millis(50)),
    )
    .await?;
    let seed = seed_worker(&executor, &stored).await?;
    for loader in loaders {
        let id = target(&seed, &format!("restart-creator-{loader}"));
        if loader == "existing-only" {
            Worker::interrupt(seed.all(), &id, false, Principal::anonymous()).await?;
        }
        let worker = acquire(&seed, &id).await?;
        let entries = worker
            .oplog()
            .read_exact(
                OplogIndex::INITIAL,
                worker.oplog().current_oplog_index().await.as_u64(),
            )
            .await
            .unwrap();
        let creates = entries
            .values()
            .filter_map(|entry| match entry {
                OplogEntry::Create { parameters, .. } => Some(parameters.create_request.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(creates.len(), 1, "{loader}");
        let create = &creates[0];
        assert_eq!(create.kind, CreateRequestKind::GetOrCreate, "{loader}");
        assert_eq!(create.idempotency_key, None, "{loader}");
        assert_eq!(create.principal, creator, "{loader}");
        assert_eq!(create.trace_id, creator_context.trace_id, "{loader}");
        assert_eq!(
            create.trace_states, creator_context.trace_states,
            "{loader}"
        );
        let span_ids = |spans: &[SpanData]| {
            spans
                .iter()
                .map(|span| match span {
                    SpanData::LocalSpan { span_id, .. } | SpanData::ExternalSpan { span_id } => {
                        span_id.clone()
                    }
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            span_ids(&create.invocation_context),
            span_ids(&creator_context.to_oplog_data()),
            "{loader}"
        );
        let pending = entries
            .values()
            .filter_map(|entry| match entry {
                OplogEntry::PendingAgentInvocation {
                    payload,
                    trace_id,
                    trace_states,
                    invocation_context,
                    ..
                } => Some((payload, trace_id, trace_states, invocation_context)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(pending.len(), 1, "{loader}");
        let (payload, trace_id, trace_states, invocation_context) = pending[0];
        let AgentInvocationPayload::AgentInitialization {
            principal: recorded,
            ..
        } = worker
            .oplog()
            .download_payload(payload.clone())
            .await
            .map_err(anyhow::Error::msg)?
        else {
            panic!("expected initialization payload")
        };
        assert_eq!(recorded, creator, "{loader}");
        assert_eq!(trace_id, &create.trace_id, "{loader}");
        assert_eq!(trace_states, &create.trace_states, "{loader}");
        assert_eq!(invocation_context, &create.invocation_context, "{loader}");
    }
    Ok(())
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn cancelled_creator_keeps_original_context_and_serializes_existing_only(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let (executor, seed, _) = setup(last_unique_id, deps, component).await?;
    let id = target(&seed, "cancelled-creator");
    let all = seed.all().clone();
    Worker::interrupt(&all, &id, false, Principal::anonymous()).await?;
    assert!(!executor.worker_is_cached(&id).await);
    assert!(
        !seed
            .oplog_service()
            .exists(&id, AgentMode::Durable)
            .await
            .unwrap()
    );
    let mut enqueue = executor
        .gate_next_agent_initialization_enqueue(&id.agent_id)
        .await;
    let context = InvocationContextStack::fresh();
    let original_principal = Principal::Agent(AgentPrincipal {
        agent_id: seed.agent_id(),
    });
    let creating = tokio::spawn({
        let all = all.clone();
        let id = id.clone();
        let context = context.clone();
        let principal = original_principal.clone();
        async move {
            Worker::get_or_create_suspended(
                &all,
                &id,
                Some(vec![("OWNER".into(), "creator".into())]),
                vec![],
                None,
                None,
                &context,
                principal,
            )
            .await
        }
    });
    tokio::time::timeout(Duration::from_secs(20), enqueue.entered())
        .await
        .expect("creator must reach the initialization enqueue");
    creating.abort();
    assert!(creating.await.err().unwrap().is_cancelled());
    let mut existing = Box::pin(Worker::interrupt(&all, &id, false, Principal::anonymous()));
    let mut competing = Box::pin(acquire(&seed, &id));
    assert!(poll!(&mut existing).is_pending());
    assert!(poll!(&mut competing).is_pending());
    assert!(!executor.worker_is_cached(&id).await);
    drop(enqueue);
    let (existing, worker) = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(existing, competing)
    })
    .await
    .expect("acquisitions must finish after initialization enqueue resumes");
    existing?;
    let worker = worker?;
    assert_eq!(
        worker.get_initial_worker_metadata().env,
        vec![("OWNER".into(), "creator".into())]
    );
    let oplog = worker.oplog();
    let entries = oplog
        .read_exact(
            OplogIndex::INITIAL,
            oplog.current_oplog_index().await.as_u64(),
        )
        .await
        .unwrap();
    assert_eq!(
        entries
            .values()
            .filter(|entry| matches!(entry, OplogEntry::Create { .. }))
            .count(),
        1
    );
    let mut initializations = 0;
    for entry in entries.values() {
        if let OplogEntry::PendingAgentInvocation {
            payload, trace_id, ..
        } = entry
            && let AgentInvocationPayload::AgentInitialization { principal, .. } = oplog
                .download_payload(payload.clone())
                .await
                .map_err(anyhow::Error::msg)?
        {
            initializations += 1;
            assert_eq!(principal, original_principal);
            assert_eq!(trace_id, &context.trace_id);
        }
    }
    assert_eq!(initializations, 1);
    Ok(())
}

async fn prepare_foreign_topology(
    worker: &Worker<TestWorkerCtx>,
    source: &DurableStreamHandle,
    consumer_invocation: StreamInvocationId,
    transport_stream_id: u64,
) -> anyhow::Result<(StreamAttachmentKey, StreamSessionMappingRecord)> {
    let metadata = worker.get_initial_worker_metadata();
    let session_key = source.source_invocation.clone();
    let attachment = StreamAttachmentKey {
        attachment_id: AttachmentId::primary(
            session_key.callee_environment_id,
            &session_key.callee,
            &session_key.idempotency_key,
        )?,
        stream_id: source.stream_id,
        epoch: 1,
        session_key: session_key.clone(),
        producer_environment_id: source.producer_environment_id,
        producer: source.producer.clone(),
        expected_producer_fingerprint: source.expected_producer_fingerprint,
        consumer_environment_id: metadata.environment_id,
        consumer: metadata.agent_id,
        expected_consumer_fingerprint: metadata.fingerprint,
        consumer_invocation,
    };
    let mapping = StreamSessionMappingRecord {
        transport_stream_id,
        handle: source.clone(),
        role: SessionStreamRole::Output,
    };
    worker
        .add_and_commit_oplog(OplogEntry::StreamSession {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            summary: None,
            record: OplogPayload::Inline(Box::new(StreamSessionRecord::TopologyPrepared(
                StreamTopologyPreparedRecord {
                    format_version: 1,
                    session_key,
                    attachment: attachment.clone(),
                    mapping: mapping.clone(),
                },
            ))),
        })
        .await
        .unwrap();
    Ok((attachment, mapping))
}

async fn session_records(
    worker: &Worker<TestWorkerCtx>,
) -> anyhow::Result<Vec<StreamSessionRecord>> {
    let oplog = worker.oplog();
    let entries = oplog
        .read_exact(
            OplogIndex::INITIAL,
            oplog.current_oplog_index().await.as_u64(),
        )
        .await?;
    let mut result = Vec::new();
    for entry in entries.into_values() {
        if let OplogEntry::StreamSession { record, .. } = entry {
            result.push(
                oplog
                    .download_payload(record)
                    .await
                    .map_err(anyhow::Error::msg)?,
            );
        }
    }
    Ok(result)
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn active_consumer_read_repairs_lost_producer_activation_without_a_timer(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let (_executor, seed, _faults) =
        setup_with_retry_interval(last_unique_id, deps, component, Duration::from_secs(3600))
            .await?;
    async {
        let producer = acquire(&seed, &target(&seed, "read-repair-source")).await?;
        let consumer_id = target(&seed, "read-repair-consumer");
        let consumer = acquire(&seed, &consumer_id).await?;
        // Keep the consumer cold: only its durable authority, not consumer recovery, can
        // authorize and repair the producer's missing activation.
        seed.active_agents().remove(&consumer_id).await;
        let source = register_stream(&producer).await?;
        prepare_session(&producer, source.source_invocation.idempotency_key, false).await?;
        let consumer_stream = register_stream(&consumer).await?;
        let (attachment, mapping) =
            prepare_foreign_topology(&consumer, &source, consumer_stream.source_invocation, 17)
                .await?;
        let now = Timestamp::now_utc().to_millis();
        producer
            .add_and_commit_oplog(OplogEntry::stream_session(
                None,
                OplogPayload::Inline(Box::new(StreamSessionRecord::AttachmentPrepared(
                    StreamAttachmentPreparedRecord {
                        format_version: 1,
                        key: attachment.clone(),
                        prepared_at_millis: now,
                        lease_expires_at_millis: now + STREAM_ATTACHMENT_LEASE_TTL_MILLIS,
                    },
                ))),
                None,
            ))
            .await?;
        let request =
            DurableStreamReadRequest::AttachedConsumer(Box::new(AttachedStreamSegmentRequest {
                format_version: 1,
                attachment: attachment.clone(),
                mapping: mapping.clone(),
                after: None,
                through: None,
                wait_for_events: false,
            }));
        assert!(
            seed.rpc()
                .read_durable_stream_segment(request.clone(), &AuthCtx::System)
                .await
                .is_err()
        );
        assert!(
            !session_records(&producer)
                .await?
                .iter()
                .any(|record| matches!(record, StreamSessionRecord::AttachmentActivated(_)))
        );

        // Publish only the consumer half: no activation RPC reaches the producer.
        consumer
            .add_and_commit_oplog(OplogEntry::stream_session(
                None,
                OplogPayload::Inline(Box::new(StreamSessionRecord::TopologyActivated(
                    StreamTopologyActivatedRecord {
                        format_version: 1,
                        session_key: source.source_invocation,
                        attachment: attachment.clone(),
                        mapping,
                    },
                ))),
                None,
            ))
            .await?;
        for _ in 0..2 {
            tokio::time::timeout(
                Duration::from_secs(10),
                seed.rpc()
                    .read_durable_stream_segment(request.clone(), &AuthCtx::System),
            )
            .await?
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
            assert_eq!(session_records(&producer).await?.iter().filter(|record| matches!(
            record, StreamSessionRecord::AttachmentActivated(record) if record.key == attachment
        )).count(), 1);
        }
        Ok::<_, anyhow::Error>(())
    }
    .await
}

async fn prepare_session(
    worker: &Worker<TestWorkerCtx>,
    idempotency_key: golem_common::model::InvocationId,
    completed: bool,
) -> anyhow::Result<StreamSessionKey> {
    let oplog = worker.oplog();
    let pending_index = oplog.current_oplog_index().await.next().next();
    let payload = OplogPayload::Inline(Box::new(AgentInvocationPayload::AgentMethod {
        method_name: "increment".into(),
        input: data_value!().value().clone(),
        principal: Principal::anonymous(),
        scope_card: None,
    }));
    let context = InvocationContextStack::fresh();
    let invocation_context = context.to_oplog_data();
    let trace_id = context.trace_id;
    let trace_states = context.trace_states;
    let metadata = worker.get_initial_worker_metadata();
    let session_key = StreamInvocationId {
        callee_environment_id: metadata.environment_id,
        callee: metadata.agent_id.clone(),
        callee_fingerprint: metadata.fingerprint,
        idempotency_key,
    };
    let attachment_id = AttachmentId::primary(
        metadata.environment_id,
        &metadata.agent_id,
        &idempotency_key,
    )?;
    let attempt_id = AttemptId::fresh();
    for record in [
        StreamSessionRecord::Prepared(StreamSessionPreparedRecord {
            format_version: 1,
            public_session_id: idempotency_key.to_string(),
            route_scope: None,
            session_key: idempotency_key,
            expiry_policy: StreamSessionExpiryPolicy::None,
            expiry_deadline_millis: None,
            attempt: StartAttemptDescriptor {
                format_version: 1,
                session_key: session_key.clone(),
                attachment_id,
                expected_callee_fingerprint: metadata.fingerprint,
                attempt_id,
                invocation: PersistedStreamInvocationDescriptor {
                    format_version: 1,
                    session_key: session_key.clone(),
                    target_component_revision: metadata.last_known_status.component_revision,
                    target: PersistedInvocationTarget::AgentMethod {
                        method_name: "increment".into(),
                    },
                    invocation_value: vec![],
                    stream_handles: vec![],
                    execution_config: vec![],
                    effective_identity: vec![],
                },
                effective_identity: vec![],
                live_join_buffer_events: 1,
            },
            stream_mappings: vec![],
        }),
        StreamSessionRecord::Attached(StreamSessionAttachedRecord {
            format_version: 1,
            session_key: idempotency_key,
            attachment_id,
            attempt_id,
            epoch: 1,
            pending_invocation_oplog_index: pending_index,
        }),
    ] {
        let prepared = matches!(record, StreamSessionRecord::Prepared(_));
        worker
            .add_and_commit_oplog(OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                summary: None,
                record: OplogPayload::Inline(Box::new(record)),
            })
            .await
            .unwrap();
        if prepared {
            assert_eq!(
                worker
                    .add_and_commit_oplog(OplogEntry::pending_agent_invocation(
                        None,
                        idempotency_key,
                        payload.clone(),
                        trace_id.clone(),
                        trace_states.clone(),
                        invocation_context.clone(),
                    ))
                    .await
                    .unwrap(),
                pending_index
            );
        }
    }
    if completed {
        worker
            .add_and_commit_oplog(OplogEntry::AgentInvocationStarted {
                timestamp: Timestamp::now_utc(),
                invocation_id: idempotency_key,
                payload,
                trace_id,
                trace_states,
                invocation_context,
                wallet_pin: Box::new(InvocationWalletPin {
                    wallet_token: WalletVersionToken {
                        wallet_id_hash: [0; 32],
                        generation: 0,
                    },
                    pinned_card_ids: Vec::new(),
                    scope_card_id: None,
                }),
            })
            .await
            .unwrap();
        worker
            .add_and_commit_oplog(OplogEntry::AgentInvocationFinished {
                timestamp: Timestamp::now_utc(),
                invocation_id: idempotency_key,
                result: OplogPayload::Inline(Box::new(AgentInvocationResult::AgentMethod {
                    output: data_value!(1u32).value().clone(),
                })),
                method_name: Some("increment".into()),
                consumed_fuel: 0,
                component_revision: metadata.last_known_status.component_revision,
            })
            .await
            .unwrap();
    }
    Ok(session_key)
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn completion_receipt_precedes_fifo_status_fold(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let (_executor, worker, faults) = setup(last_unique_id, deps, component).await?;
    let metadata = worker.get_initial_worker_metadata();
    let idempotency_key = golem_common::model::InvocationId(uuid::Uuid::new_v4());
    let payload = OplogPayload::Inline(Box::new(AgentInvocationPayload::AgentMethod {
        method_name: "increment".into(),
        input: data_value!().value().clone(),
        principal: Principal::anonymous(),
        scope_card: None,
    }));
    let context = InvocationContextStack::fresh();
    let invocation_context = context.to_oplog_data();

    worker
        .add_and_commit_oplog(OplogEntry::pending_agent_invocation(
            None,
            idempotency_key,
            payload.clone(),
            context.trace_id.clone(),
            context.trace_states.clone(),
            invocation_context.clone(),
        ))
        .await
        .unwrap();
    worker
        .add_and_commit_oplog(OplogEntry::AgentInvocationStarted {
            timestamp: Timestamp::now_utc(),
            invocation_id: idempotency_key,
            payload,
            trace_id: context.trace_id,
            trace_states: context.trace_states,
            invocation_context,
            wallet_pin: Box::new(InvocationWalletPin {
                wallet_token: WalletVersionToken {
                    wallet_id_hash: [0; 32],
                    generation: 0,
                },
                pinned_card_ids: Vec::new(),
                scope_card_id: None,
            }),
        })
        .await
        .unwrap();
    let finished_index = worker
        .add_to_oplog(OplogEntry::AgentInvocationFinished {
            timestamp: Timestamp::now_utc(),
            invocation_id: idempotency_key,
            result: OplogPayload::Inline(Box::new(AgentInvocationResult::AgentMethod {
                output: data_value!(1u32).value().clone(),
            })),
            method_name: Some("increment".into()),
            consumed_fuel: 0,
            component_revision: metadata.last_known_status.component_revision,
        })
        .await
        .unwrap();

    // Finishing changes Running to Idle, so the synchronous recovery-index update removes this
    // worker. Holding that write deterministically pauses the actor after its durable commit and
    // in-memory fold, but before the FIFO commit+fold job itself is complete.
    let fold = faults.pause_next("remove");
    let completion = tokio::spawn({
        let worker = worker.clone();
        async move {
            worker
                .commit_oplog_before_status_update(
                    golem_worker_executor::services::oplog::CommitLevel::Always,
                )
                .await
                .unwrap()
        }
    });
    tokio::time::timeout(Duration::from_secs(20), fold.entered()).await?;
    tokio::time::timeout(Duration::from_secs(1), completion)
        .await
        .expect("completion receipt must not wait for status folding")?;
    let persisted = worker
        .oplog_service()
        .read_exact(
            &OwnedAgentId::new(metadata.environment_id, &metadata.agent_id),
            AgentMode::Durable,
            finished_index,
            1,
        )
        .await
        .unwrap();
    assert!(matches!(
        persisted.get(&finished_index),
        Some(OplogEntry::AgentInvocationFinished { .. })
    ));

    // The receipt caller is gone, but actor-owned folding must continue. Both production reads
    // are queued behind it rather than observing the already-swapped status out of FIFO order.
    let status = tokio::spawn({
        let worker = worker.clone();
        async move { worker.get_last_known_status().await }
    });
    let result = tokio::spawn({
        let worker = worker.clone();
        async move { worker.lookup_invocation_result(&idempotency_key).await }
    });
    let metadata_read = tokio::spawn({
        let worker = worker.clone();
        let owned_agent_id = OwnedAgentId::new(metadata.environment_id, &metadata.agent_id);
        async move { Worker::get_latest_metadata(worker.all(), &owned_agent_id).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!status.is_finished(), "status read bypassed the FIFO fold");
    assert!(
        !result.is_finished(),
        "result lookup bypassed the FIFO fold"
    );
    assert!(
        !metadata_read.is_finished(),
        "metadata read bypassed the FIFO fold"
    );

    fold.release();
    let status = tokio::time::timeout(Duration::from_secs(20), status).await??;
    assert_eq!(status.status, golem_common::model::AgentStatus::Idle);
    assert_eq!(status.oplog_idx, finished_index);
    let metadata = tokio::time::timeout(Duration::from_secs(20), metadata_read).await???;
    assert_eq!(metadata.unwrap().last_known_status, *status);
    let LookupResult::Complete(Ok(output)) =
        tokio::time::timeout(Duration::from_secs(20), result).await??
    else {
        panic!("expected the completed invocation result");
    };
    assert_eq!(output.oplog_index, Some(finished_index));
    assert_eq!(
        output.result,
        AgentInvocationResult::AgentMethod {
            output: data_value!(1u32).value().clone(),
        }
    );
    Ok(())
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn pending_failure_waits_for_completion_projection_without_blocking_success(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::{AgentInvocationOutput, regions::OplogRegion};
    use golem_worker_executor::services::HasEvents;
    use golem_worker_executor::services::events::Event;
    use golem_worker_executor::services::oplog::CommitLevel;

    let (executor, worker, faults) = setup(last_unique_id, deps, component).await?;
    let key = golem_common::model::InvocationId(uuid::Uuid::new_v4());
    let context = InvocationContextStack::fresh();
    // Make the actor-owned invocation prefix fall inside the later Jump, forcing checkpoint
    // fallback while leaving the invocation's completion itself in retained history.
    let invalidated_prefix = worker
        .add_and_commit_oplog(OplogEntry::grow_memory(1))
        .await?;
    worker
        .add_and_commit_oplog(OplogEntry::AgentInvocationStarted {
            timestamp: Timestamp::now_utc(),
            invocation_id: key,
            payload: OplogPayload::Inline(Box::new(AgentInvocationPayload::AgentMethod {
                method_name: "increment".into(),
                input: data_value!().value().clone(),
                principal: Principal::anonymous(),
                scope_card: None,
            })),
            invocation_context: context.to_oplog_data(),
            trace_id: context.trace_id,
            trace_states: context.trace_states,
            wallet_pin: Box::new(InvocationWalletPin {
                wallet_token: WalletVersionToken {
                    wallet_id_hash: [0; 32],
                    generation: 0,
                },
                pinned_card_ids: Vec::new(),
                scope_card_id: None,
            }),
        })
        .await?;
    worker
        .add_and_commit_oplog(OplogEntry::grow_memory(37))
        .await?;
    worker
        .add_to_oplog(OplogEntry::jump(
            None,
            OplogRegion {
                start: invalidated_prefix,
                end: invalidated_prefix.next(),
            },
        ))
        .await?;
    let result = AgentInvocationResult::AgentMethod {
        output: data_value!(7u32).value().clone(),
    };
    let finished = worker
        .add_to_oplog(OplogEntry::AgentInvocationFinished {
            timestamp: Timestamp::now_utc(),
            invocation_id: key,
            result: OplogPayload::Inline(Box::new(result.clone())),
            method_name: Some("increment".into()),
            consumed_fuel: 0,
            component_revision: worker
                .get_initial_worker_metadata()
                .last_known_status
                .component_revision,
        })
        .await?;
    // Actor-owned reconstruction must reach the checkpoint before publishing the repaired status.
    let fold = faults.pause_next("read_cached_status");
    let receipt = tokio::spawn({
        let worker = worker.clone();
        async move {
            worker
                .commit_oplog_before_status_update(CommitLevel::Always)
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(20), fold.entered()).await?;
    tokio::time::timeout(Duration::from_secs(1), receipt).await???;
    let mut events = worker.events().subscribe();
    let mut cleanup = Box::pin(worker.test_fail_pending_invocations());
    assert!(poll!(cleanup.as_mut()).is_pending());
    // No hydrated success existed when cleanup began. Publishing it must not await the fold.
    tokio::time::timeout(
        Duration::from_secs(1),
        worker.store_invocation_success(
            &key,
            AgentInvocationOutput {
                result: result.clone(),
                consumed_fuel: None,
                invocation_status: None,
                component_revision: None,
                agent_id: None,
                idempotency_key: None,
                oplog_index: Some(finished),
                agent_fingerprint: None,
            },
        ),
    )
    .await?;
    fold.release();
    tokio::time::timeout(Duration::from_secs(20), cleanup).await?;
    let completion = |event: &Event| match event {
        Event::InvocationCompleted {
            idempotency_key,
            result,
            ..
        } if idempotency_key == &key => Some(result.as_ref().clone()),
        _ => None,
    };
    let observed =
        tokio::time::timeout(Duration::from_secs(1), events.wait_for(&completion)).await???;
    assert_eq!(observed.result, result);
    assert!(matches!(
        worker.lookup_invocation_result(&key).await,
        LookupResult::Complete(Ok(_))
    ));

    // A late stop continuation after deletion must not await stopped actors or publish a failure.
    executor.delete_worker(&worker.agent_id()).await?;
    tokio::time::timeout(
        Duration::from_secs(1),
        worker.test_fail_pending_invocations(),
    )
    .await?;
    assert!(
        tokio::time::timeout(Duration::from_millis(50), events.wait_for(&completion))
            .await
            .is_err()
    );
    Ok(())
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn jump_projection_finishes_without_its_producer(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::regions::OplogRegion;
    use golem_worker_executor::services::oplog::CommitLevel;

    let (_executor, worker, faults) = setup(last_unique_id, deps, component).await?;
    let metadata = worker.get_initial_worker_metadata();
    let owned = OwnedAgentId::new(metadata.environment_id, &metadata.agent_id);
    let initial = worker.get_last_known_status().await;
    let direct = worker.add_to_oplog(OplogEntry::grow_memory(7)).await?;
    // Consume a durable commit receipt outside the status actor. Its precommit sample must
    // recover the missing suffix even when the subsequent actor commit returns no entries.
    worker.oplog().commit(CommitLevel::Always).await?;
    worker
        .commit_oplog_and_update_state(CommitLevel::DurableOnly)
        .await?;
    let caught_up = worker.get_last_known_status().await;
    assert_eq!(caught_up.oplog_idx, direct);
    assert_eq!(
        caught_up.total_linear_memory_size,
        initial.total_linear_memory_size + 7
    );
    for cancel_full_waiter in [false, true] {
        let before = worker.get_last_known_status().await;
        let discarded = worker
            .add_and_commit_oplog(OplogEntry::grow_memory(11))
            .await?;
        let key = golem_common::model::InvocationId(uuid::Uuid::new_v4());
        let context = InvocationContextStack::fresh();
        let started = worker
            .add_and_commit_oplog(OplogEntry::AgentInvocationStarted {
                timestamp: Timestamp::now_utc(),
                invocation_id: key,
                payload: OplogPayload::Inline(Box::new(AgentInvocationPayload::AgentMethod {
                    method_name: "increment".into(),
                    input: data_value!().value().clone(),
                    principal: Principal::anonymous(),
                    scope_card: None,
                })),
                invocation_context: context.to_oplog_data(),
                trace_id: context.trace_id,
                trace_states: context.trace_states,
                wallet_pin: Box::new(InvocationWalletPin {
                    wallet_token: WalletVersionToken {
                        wallet_id_hash: [0; 32],
                        generation: 0,
                    },
                    pinned_card_ids: Vec::new(),
                    scope_card_id: None,
                }),
            })
            .await?;
        worker.add_to_oplog(OplogEntry::grow_memory(23)).await?;
        let jump_index = worker.oplog().current_oplog_index().await.next();
        worker
            .add_to_oplog(OplogEntry::jump(
                None,
                OplogRegion {
                    start: discarded,
                    end: jump_index,
                },
            ))
            .await?;
        // The checkpoint lookup is inside actor-owned reconstruction, after the commit receipt.
        let repair = faults.pause_next("read_cached_status");
        let producer = tokio::spawn({
            let worker = worker.clone();
            async move {
                if cancel_full_waiter {
                    worker
                        .commit_oplog_and_update_state(CommitLevel::Always)
                        .await
                        .map(|_| ())
                } else {
                    worker
                        .commit_oplog_before_status_update(CommitLevel::Always)
                        .await
                }
            }
        });
        tokio::time::timeout(Duration::from_secs(20), repair.entered()).await?;
        if cancel_full_waiter {
            assert!(!producer.is_finished());
            producer.abort();
            assert!(producer.await.unwrap_err().is_cancelled());
        } else {
            tokio::time::timeout(Duration::from_secs(1), producer).await???;
        }
        let sentinel = worker.add_to_oplog(OplogEntry::grow_memory(101)).await?;
        let status = tokio::spawn({
            let worker = worker.clone();
            async move { worker.get_last_known_status().await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!status.is_finished(), "status read bypassed reconstruction");
        repair.release();
        let status = tokio::time::timeout(Duration::from_secs(20), status).await??;
        assert_eq!(status.oplog_idx, jump_index);
        assert_eq!(
            status.total_linear_memory_size,
            before.total_linear_memory_size
        );
        assert!(status.skipped_regions.is_in_deleted_region(discarded));
        assert!(status.skipped_regions.is_in_deleted_region(started));
        // A second authoritative read neither commits the sentinel nor redoes reconstruction.
        let checkpoint_reads = faults.calls("read_cached_status");
        assert_eq!(*worker.get_last_known_status().await, *status);
        assert_eq!(faults.calls("read_cached_status"), checkpoint_reads);
        assert_eq!(
            worker
                .oplog_service()
                .get_last_index(&owned, AgentMode::Durable)
                .await
                .unwrap(),
            jump_index
        );
        worker
            .commit_oplog_and_update_state(CommitLevel::Always)
            .await?;
        let status = worker.get_last_known_status().await;
        assert_eq!(status.oplog_idx, sentinel);
        assert_eq!(
            status.total_linear_memory_size,
            before.total_linear_memory_size + 101
        );
        assert_eq!(
            worker
                .oplog_service()
                .get_last_index(&owned, AgentMode::Durable)
                .await
                .unwrap(),
            sentinel
        );
    }
    Ok(())
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn failure_handler_status_read_follows_concurrent_jump(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::regions::OplogRegion;
    use golem_worker_executor::services::oplog::CommitLevel;

    let context = TestContext::new(last_unique_id);
    let faults = KeyValueStorageFaults::default();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| config.retry.max_attempts = 0)),
            wrap_key_value_storage: Some(Arc::new({
                let faults = faults.clone();
                move |storage| Arc::new(FaultInjectingKeyValueStorage::new(storage, faults.clone()))
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let name = agent_id!("FailingCounter", "failure-status-jump");
    let id = executor.start_agent(&component.id, name.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &name, "add", data_value!(5u64))
        .await?;
    let owned = OwnedAgentId::new(context.default_environment_id, &id);
    let worker = executor.active_agent(&owned).await.unwrap().primary();
    assert_eq!(
        worker.get_last_known_status().await.status,
        golem_common::model::AgentStatus::Idle
    );
    // Failure untracks the worker after the handler's first status read and Error commit.
    let failure_fold = faults.pause_next("remove");
    let (invocation, coordination) = tokio::join!(
        executor.invoke_and_await_agent(&component, &name, "add", data_value!(50u64)),
        async {
            tokio::time::timeout(Duration::from_secs(20), failure_fold.entered()).await?;
            let error_index = worker.oplog().current_oplog_index().await;
            assert!(matches!(
                worker.oplog().read(error_index).await.unwrap(),
                OplogEntry::Error { .. }
            ));
            let jump_index = worker
                .add_to_oplog(OplogEntry::jump(
                    None,
                    OplogRegion {
                        start: error_index,
                        end: error_index.next(),
                    },
                ))
                .await?;
            // Polling enqueues this commit while the failure handler is still waiting on its
            // prior commit. Thus the Jump is ahead of the handler's second FIFO status read.
            let mut jump = Box::pin(worker.commit_oplog_and_update_state(CommitLevel::Always));
            assert!(poll!(jump.as_mut()).is_pending());
            failure_fold.release();
            jump.await?;
            let status = worker.get_last_known_status().await;
            assert_eq!(status.oplog_idx, jump_index);
            assert!(status.skipped_regions.is_in_deleted_region(error_index));
            Ok::<_, anyhow::Error>(())
        }
    );
    coordination?;
    let error = invocation.unwrap_err().to_string();
    assert!(error.contains("value is too large"), "{error}");
    assert!(!error.contains("detached"), "{error}");
    Ok(())
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn reciprocal_cold_topologies_recover_without_initialization_cycle(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let (executor, seed, faults) = setup(last_unique_id, deps, component).await?;
    let a_id = target(&seed, "reciprocal-a");
    let b_id = target(&seed, "reciprocal-b");
    let a = acquire(&seed, &a_id).await?;
    let b = acquire(&seed, &b_id).await?;
    seed.active_agents().remove(&a_id).await;
    seed.active_agents().remove(&b_id).await;
    let a_stream = register_stream(&a).await?;
    let b_stream = register_stream(&b).await?;
    let (a_attachment, a_mapping) =
        prepare_foreign_topology(&a, &b_stream, a_stream.source_invocation.clone(), 11).await?;
    let a_fingerprint = a.get_initial_worker_metadata().fingerprint;
    let (b_attachment, _) =
        prepare_foreign_topology(&b, &a_stream, b_stream.source_invocation.clone(), 29).await?;
    prepare_session(&a, a_stream.source_invocation.idempotency_key, false).await?;
    prepare_session(&b, b_stream.source_invocation.idempotency_key, false).await?;
    let completed_session = prepare_session(
        &a,
        golem_common::model::InvocationId(uuid::Uuid::new_v4()),
        true,
    )
    .await?;
    drop(a);
    drop(b);
    assert!(!executor.worker_is_cached(&a_id).await);
    assert!(!executor.worker_is_cached(&b_id).await);

    seed.worker_service()
        .lookup_durable_stream_recovery_metadata(&a_id, AgentMode::Durable, a_fingerprint)
        .await
        .map_err(anyhow::Error::msg)?;
    // Hold A's recovery after publication. B may acquire A while recovering its own attachment,
    // but a stream read may not treat A's merely prepared topology as an active attachment.
    let recovery = faults.gate_next(
        "read_recovery",
        KeyValueStorageError::Other("recovery storage outage".into()),
    );
    let a = tokio::time::timeout(Duration::from_secs(20), acquire(&seed, &a_id)).await??;
    recovery.entered().await;
    assert!(executor.worker_is_cached(&a_id).await);
    let b = tokio::time::timeout(Duration::from_secs(20), acquire(&seed, &b_id)).await??;
    let read = seed
        .rpc()
        .read_durable_stream_segment(
            DurableStreamReadRequest::AttachedConsumer(Box::new(AttachedStreamSegmentRequest {
                format_version: 1,
                attachment: a_attachment.clone(),
                mapping: a_mapping,
                after: None,
                through: None,
                wait_for_events: false,
            })),
            &AuthCtx::System,
        )
        .await;
    assert!(
        read.is_err(),
        "prepared topology must not authorize a stream read"
    );
    let before = session_records(&a).await?;
    assert!(!before.iter().any(|record| matches!(
        record,
        StreamSessionRecord::TopologyActivated(_) | StreamSessionRecord::Finished(_)
    )));
    recovery.release();

    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let a_records = session_records(&a).await?;
            let b_records = session_records(&b).await?;
            let a_activated = a_records.iter().filter(|record| matches!(record, StreamSessionRecord::TopologyActivated(record) if record.attachment == a_attachment)).count();
            let b_activated = b_records.iter().filter(|record| matches!(record, StreamSessionRecord::TopologyActivated(record) if record.attachment == b_attachment)).count();
            let finished = a_records.iter().filter(|record| matches!(record, StreamSessionRecord::Finished(record) if record.session_key == StreamRegistrationInvocation::Local(completed_session.idempotency_key))).count();
            if a_activated != 0 && b_activated != 0 && finished != 0 {
                assert_eq!(a_activated, 1);
                assert_eq!(b_activated, 1);
                assert_eq!(finished, 1);
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await??;
    assert_eq!(
        a.get_initial_worker_metadata().fingerprint,
        a_stream.expected_producer_fingerprint
    );
    assert_eq!(
        b.get_initial_worker_metadata().fingerprint,
        b_stream.expected_producer_fingerprint
    );

    // Shutdown must wake and stop both reconcilers even when healthy attachments leave them
    // parked without a retry timer.
    drop(executor);
    tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(
            a.wait_for_durable_stream_attachment_reconciler(),
            b.wait_for_durable_stream_attachment_reconciler()
        );
    })
    .await
    .map_err(|_| anyhow::anyhow!("durable stream reconcilers did not stop within 10s"))?;
    // Shutdown closes the captured oplog actors. Read persisted indexes after drainage instead
    // of submitting metadata queries to those retired actors.
    seed.active_agents()
        .invocation_loops()
        .wait_for_exit()
        .await
        .map_err(anyhow::Error::msg)?;
    let oplogs = seed.oplog_service();
    let a_stopped_at = oplogs.get_last_index(&a_id, AgentMode::Durable).await?;
    let b_stopped_at = oplogs.get_last_index(&b_id, AgentMode::Durable).await?;
    tokio::time::sleep(CACHE_TTL * 3).await;
    assert_eq!(
        oplogs.get_last_index(&a_id, AgentMode::Durable).await?,
        a_stopped_at
    );
    assert_eq!(
        oplogs.get_last_index(&b_id, AgentMode::Durable).await?,
        b_stopped_at
    );
    Ok(())
}
