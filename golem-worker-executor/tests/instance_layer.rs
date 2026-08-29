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

//! White-box integration tests for the entity execution substrate beneath tool invocation.
//!
//! Component storage, owner startup, and ordinary agent calls use [`TestDsl`]. Tests whose
//! expectations concern an entity Store, owner lane, invocation scope, or pinned activation then
//! cross the public DSL boundary deliberately: those concepts are executor internals and cannot be
//! exercised through a public tool call without also testing the routing adapter above them.

use crate::Tracing;
use async_trait::async_trait;
use golem_common::base_model::agent::{AgentPrincipal, Principal};
use golem_common::base_model::json::NormalizedJsonValue;
use golem_common::model::account::AccountEmail;
use golem_common::model::agent::{AgentTypeName, ParsedAgentId};
use golem_common::model::agent_secret::{
    AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
};
use golem_common::model::component::{
    AgentFilePath, AgentFilePermissions, ComponentId, ComponentName, ComponentRevision,
    InitialAgentFile,
};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::entity::{
    AgentEntity, EntityActivation, EntityActivationPolicy, EntityInvocationId,
    EntityInvocationScope, ExecutableTarget, FilesystemCapability, InvocationExecutionMode,
    OwnedAgentEntityId, ToolMiddlewareName,
};
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::regions::{DeletedRegions, OplogRegion};
use golem_common::model::retry_policy::NamedRetryPolicy;
use golem_common::model::tool::{
    CompiledToolBinding, SecretKeyScope, ToolFilesystemAccess, ToolName, ToolProvisionConfig,
    ToolSource,
};
use golem_common::model::{AgentInvocation, AgentInvocationResult, IdempotencyKey, OwnedAgentId};
use golem_common::schema::SchemaGraph;
use golem_common::schema::schema_type::SchemaType;
use golem_common::schema::schema_value::SchemaValue;
use golem_common::{agent_id, data_value, widen_infallible};
use golem_schema::schema::wit::encode_graph;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::AgentDeploymentDetails;
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::replayable_stream::ReplayableStream;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::durable_host::DurableWorkerCtxView;
use golem_worker_executor::preview2::golem::agent::host::Host as AgentHost;
use golem_worker_executor::services::HasComponentService;
use golem_worker_executor::services::active_agents::ActiveAgent;
use golem_worker_executor::services::environment_state::EnvironmentStateService;
use golem_worker_executor::services::oplog::CommitLevel;
use golem_worker_executor::worker::EvictionClass;
use golem_worker_executor::worker::invocation::{
    InvocationMode, InvokeResult, invoke_observed_and_traced, lower_invocation,
};
use golem_worker_executor::worker::owner_lane::{EntityCallMode, OwnerInvocationId};
use golem_worker_executor::workerctx::{
    EntityInvocationManagement, InvocationManagement, WorkerCtx,
};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerCtx,
    WorkerExecutorTestDependencies, start, start_with_overrides,
};
use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use test_r::{inherit_test_dep, test, timeout};
use wasmtime::component::Instance;
use wasmtime::{Store, StoreMemory};
use wasmtime_wasi::p2::bindings::filesystem::preopens::Host as PreopensHost;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_sdk_rust")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

struct FixedSecretsEnvironmentStateService {
    secrets: HashMap<CanonicalAgentSecretPath, AgentSecret>,
}

#[async_trait]
impl EnvironmentStateService for FixedSecretsEnvironmentStateService {
    async fn get_agent_deployment(
        &self,
        _environment: golem_common::model::environment::EnvironmentId,
        _agent_type: &AgentTypeName,
    ) -> Result<Option<AgentDeploymentDetails>, WorkerExecutorError> {
        Ok(None)
    }

    async fn get_agent_secrets(
        &self,
        _environment: golem_common::model::environment::EnvironmentId,
    ) -> Result<HashMap<CanonicalAgentSecretPath, AgentSecret>, WorkerExecutorError> {
        Ok(self.secrets.clone())
    }

    async fn get_agent_secret_revision(
        &self,
        _environment: golem_common::model::environment::EnvironmentId,
        secret_id: AgentSecretId,
        path: CanonicalAgentSecretPath,
        revision: AgentSecretRevision,
    ) -> Result<Option<AgentSecret>, WorkerExecutorError> {
        Ok(self
            .secrets
            .get(&path)
            .filter(|secret| secret.id == secret_id && secret.revision == revision)
            .cloned())
    }

    async fn get_retry_policies(
        &self,
        _environment: golem_common::model::environment::EnvironmentId,
    ) -> Result<Vec<NamedRetryPolicy>, WorkerExecutorError> {
        Ok(Vec::new())
    }
}

#[derive(Debug)]
struct CompletionSignal(Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for CompletionSignal {
    fn drop(&mut self) {
        if let Some(completed) = self.0.take() {
            let _ = completed.send(());
        }
    }
}

async fn owner_component_metadata(
    active_agent: &ActiveAgent<TestWorkerCtx>,
    component_id: ComponentId,
    component_revision: ComponentRevision,
) -> Result<Arc<golem_service_base::model::component::Component>, WorkerExecutorError> {
    active_agent
        .primary()
        .component_service()
        .get_metadata(component_id, Some(component_revision))
        .await
        .map(Arc::new)
}

async fn initialize_entity(
    instance: &Instance,
    store: &mut Store<TestWorkerCtx>,
    parsed_agent_id: &ParsedAgentId,
    principal: Principal,
) -> Result<(), golem_service_base::error::worker_executor::WorkerExecutorError> {
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
        input: parsed_agent_id.parameters.value().clone(),
        invocation_context: InvocationContextStack::fresh(),
        principal,
    };
    let lowered = lower_invocation(init.clone(), &metadata, Some(parsed_agent_id))?;
    let result =
        invoke_observed_and_traced(lowered, store, instance, InvocationMode::Replay).await?;
    match result {
        InvokeResult::Succeeded {
            result: AgentInvocationResult::AgentInitialization,
            ..
        } => Ok(()),
        result => Err(
            golem_service_base::error::worker_executor::WorkerExecutorError::runtime(format!(
                "entity initialization did not succeed: {result:?}"
            )),
        ),
    }
}

async fn invoke_sleep_p3(
    instance: &Instance,
    store: &mut Store<TestWorkerCtx>,
    parsed_agent_id: &ParsedAgentId,
    principal: Principal,
    seconds: u64,
) -> Result<(), golem_service_base::error::worker_executor::WorkerExecutorError> {
    let metadata = store.data().component_metadata().metadata.clone();
    let method_key = IdempotencyKey::fresh();
    store
        .data_mut()
        .set_current_idempotency_key(method_key.clone())
        .await;
    let method = AgentInvocation::AgentMethod {
        idempotency_key: method_key,
        method_name: "sleep_p3".to_string(),
        input: data_value!(seconds).value().clone(),
        invocation_context: InvocationContextStack::fresh(),
        principal,
        scope_card: None,
    };
    let lowered = lower_invocation(method.clone(), &metadata, Some(parsed_agent_id))?;
    let result =
        invoke_observed_and_traced(lowered, store, instance, InvocationMode::Replay).await?;
    assert!(matches!(
        result,
        InvokeResult::Succeeded {
            result: AgentInvocationResult::AgentMethod { .. },
            ..
        }
    ));
    Ok(())
}

async fn invoke_entity_method(
    instance: &Instance,
    store: &mut Store<TestWorkerCtx>,
    parsed_agent_id: &ParsedAgentId,
    principal: Principal,
    method_name: &str,
    input: golem_common::schema::SchemaValue,
) -> Result<AgentInvocationResult, golem_service_base::error::worker_executor::WorkerExecutorError>
{
    let metadata = store.data().component_metadata().metadata.clone();
    let method_key = IdempotencyKey::fresh();
    store
        .data_mut()
        .set_current_idempotency_key(method_key.clone())
        .await;
    let method = AgentInvocation::AgentMethod {
        idempotency_key: method_key,
        method_name: method_name.to_string(),
        input,
        invocation_context: InvocationContextStack::fresh(),
        principal,
        scope_card: None,
    };
    let lowered = lower_invocation(method.clone(), &metadata, Some(parsed_agent_id))?;
    match invoke_observed_and_traced(lowered, store, instance, InvocationMode::Replay).await? {
        InvokeResult::Succeeded { result, .. } => Ok(result),
        result => Err(
            golem_service_base::error::worker_executor::WorkerExecutorError::runtime(format!(
                "entity method did not succeed: {result:?}"
            )),
        ),
    }
}

fn activation(
    executable: ExecutableTarget,
    component_name: &str,
    agent_type_name: AgentTypeName,
    tool_name: ToolName,
    account_id: golem_common::model::account::AccountId,
    filesystem: FilesystemCapability,
) -> EntityActivation {
    activation_with_provision(
        executable,
        component_name,
        agent_type_name,
        tool_name,
        account_id,
        filesystem,
        ToolProvisionConfig::default(),
    )
}

fn activation_with_provision(
    executable: ExecutableTarget,
    component_name: &str,
    agent_type_name: AgentTypeName,
    tool_name: ToolName,
    account_id: golem_common::model::account::AccountId,
    filesystem: FilesystemCapability,
    provision: ToolProvisionConfig,
) -> EntityActivation {
    activation_with_policy(
        executable,
        component_name,
        agent_type_name,
        tool_name,
        account_id,
        filesystem,
        provision,
        SecretKeyScope::All,
        SecretKeyScope::All,
    )
}

fn activation_with_secret_policy(
    executable: ExecutableTarget,
    component_name: &str,
    agent_type_name: AgentTypeName,
    tool_name: ToolName,
    account_id: golem_common::model::account::AccountId,
    secret_keys_readable: SecretKeyScope,
    secret_keys_revealable: SecretKeyScope,
) -> EntityActivation {
    activation_with_policy(
        executable,
        component_name,
        agent_type_name,
        tool_name,
        account_id,
        FilesystemCapability::Incapable,
        ToolProvisionConfig::default(),
        secret_keys_readable,
        secret_keys_revealable,
    )
}

#[allow(clippy::too_many_arguments)]
fn activation_with_policy(
    executable: ExecutableTarget,
    component_name: &str,
    agent_type_name: AgentTypeName,
    tool_name: ToolName,
    account_id: golem_common::model::account::AccountId,
    filesystem: FilesystemCapability,
    provision: ToolProvisionConfig,
    secret_keys_readable: SecretKeyScope,
    secret_keys_revealable: SecretKeyScope,
) -> EntityActivation {
    let deployment_revision = DeploymentRevision::try_from(1_u64).unwrap();
    let source = ToolSource::Component {
        component_id: executable.component_id,
        component_revision: executable.component_revision,
        component_name: ComponentName(component_name.to_string()),
    };
    let binding = CompiledToolBinding {
        deployment_revision,
        agent_type_name,
        tool_name,
        version: "1.0.0".to_string(),
        metadata_version: "0.1.0".to_string(),
        account_id,
        account_email: AccountEmail::new("test@golem"),
        parameters: NormalizedJsonValue::new(serde_json::json!({})),
        secret_keys_readable,
        secret_keys_revealable,
        filesystem_access: match filesystem {
            FilesystemCapability::Capable => {
                golem_common::model::tool::ToolFilesystemAccess::Allowed
            }
            FilesystemCapability::Incapable => {
                golem_common::model::tool::ToolFilesystemAccess::Denied
            }
        },
        source,
    };

    EntityActivation::new(
        executable,
        deployment_revision,
        EntityActivationPolicy::Tool {
            provision,
            binding: Box::new(binding),
        },
        filesystem,
    )
    .unwrap()
}

/// Runs one synthetic synchronous entity call beneath the public tool-routing layer.
///
/// The helper models an already-admitted primary invocation dispatching an entity: it establishes
/// the parent lane position, installs a live entity scope, starts a fresh entity Store, and waits
/// for its result. Tests retain the invocation closure so assertions about Store-local host state
/// remain visible at the call site.
async fn run_synchronous_entity_invocation<R, F>(
    active_agent: &ActiveAgent<TestWorkerCtx>,
    owner_metadata: Arc<golem_service_base::model::component::Component>,
    owner_id: &OwnedAgentId,
    entity: &AgentEntity,
    activation: Arc<EntityActivation>,
    invoke: F,
) -> Result<R, WorkerExecutorError>
where
    R: Send + 'static,
    F: Send + 'static,
    F: for<'a> FnOnce(
        &'a Instance,
        &'a mut Store<TestWorkerCtx>,
        Principal,
    )
        -> Pin<Box<dyn Future<Output = Result<R, WorkerExecutorError>> + Send + 'a>>,
{
    let lane = active_agent.execution().lane();
    let parent_start = active_agent
        .execution()
        .oplog()
        .current_oplog_index()
        .await
        .next();
    let parent_id = OwnerInvocationId::Agent(parent_start);
    let parent = lane
        .enter_primary(parent_start)
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
        .acquire()
        .await
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
    let principal = Principal::Agent(AgentPrincipal {
        agent_id: owner_id.agent_id.clone(),
    });
    let scope = invocation_scope(
        owner_id,
        entity,
        parent_start.next(),
        parent_start,
        activation,
        principal.clone(),
    );
    let body = active_agent.start_entity_invocation(
        parent_id.clone(),
        scope,
        owner_metadata,
        EntityCallMode::Synchronous,
        move |instance, store| invoke(instance, store, principal),
        std::future::ready,
    )?;

    let result = body.await_result(&parent_id).await;
    drop(parent);
    result
}

fn middleware_activation(
    executable: ExecutableTarget,
    middleware_name: ToolMiddlewareName,
) -> EntityActivation {
    EntityActivation::new(
        executable,
        DeploymentRevision::try_from(1_u64).unwrap(),
        EntityActivationPolicy::ToolMiddleware {
            middleware_name,
            provision: ToolProvisionConfig::default(),
            secret_keys_readable: SecretKeyScope::All,
            secret_keys_revealable: SecretKeyScope::All,
            filesystem_access: ToolFilesystemAccess::Denied,
        },
        FilesystemCapability::Incapable,
    )
    .unwrap()
}

fn invocation_scope(
    owner_id: &OwnedAgentId,
    entity: &AgentEntity,
    start_index: golem_common::model::oplog::OplogIndex,
    parent_start_index: golem_common::model::oplog::OplogIndex,
    activation: Arc<EntityActivation>,
    principal: Principal,
) -> EntityInvocationScope {
    EntityInvocationScope::new(
        EntityInvocationId::new(
            OwnedAgentEntityId {
                owner: owner_id.clone(),
                entity: entity.clone(),
            },
            start_index,
        )
        .unwrap(),
        parent_start_index,
        activation,
        principal,
        InvocationExecutionMode::Live,
    )
    .unwrap()
}

fn replay_invocation_scope(
    owner_id: &OwnedAgentId,
    entity: &AgentEntity,
    start_index: golem_common::model::oplog::OplogIndex,
    parent_start_index: golem_common::model::oplog::OplogIndex,
    activation: Arc<EntityActivation>,
    principal: Principal,
) -> EntityInvocationScope {
    EntityInvocationScope::new(
        EntityInvocationId::new(
            OwnedAgentEntityId {
                owner: owner_id.clone(),
                entity: entity.clone(),
            },
            start_index,
        )
        .unwrap(),
        parent_start_index,
        activation,
        principal,
        InvocationExecutionMode::ReplayingCompleted,
    )
    .unwrap()
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn transient_entity_store_uses_owner_execution_and_scoped_cleanup(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let owner_component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let alternate_component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Clock", "entity-owner");
    let worker_id = executor
        .start_agent(&owner_component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&owner_component, &agent_id, "healthcheck", data_value!())
        .await?;

    let owner_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let active_agent = executor
        .active_agent(&owner_id)
        .await
        .expect("owner must remain in the active-agent registry");
    let owner_metadata =
        owner_component_metadata(&active_agent, owner_component.id, owner_component.revision)
            .await?;
    assert_eq!(active_agent.owner_id(), &owner_id);
    assert_eq!(active_agent.execution().owner_id(), &owner_id);
    assert!(Arc::ptr_eq(
        &active_agent.execution(),
        &active_agent.primary().owner_execution()
    ));
    assert!(Arc::ptr_eq(
        &active_agent.resources(),
        &active_agent.primary().owner_runtime_resources()
    ));

    let parsed_agent_id = agent_id.clone();
    let tool_name = ToolName::try_from("phase-two-test").unwrap();
    let alternate_activation = activation(
        ExecutableTarget::new(alternate_component.id, alternate_component.revision),
        "test:alternate-entity",
        parsed_agent_id.agent_type.clone(),
        tool_name.clone(),
        context.account_id,
        FilesystemCapability::Incapable,
    );
    let alternate_host =
        active_agent.entity_instance_host(&alternate_activation, owner_metadata.clone())?;
    let (_, alternate_metadata) = alternate_host.activate().await?;
    assert_eq!(alternate_metadata.id, alternate_component.id);
    assert_ne!(alternate_metadata.id, owner_component.id);
    drop(alternate_host.instantiate_entity().await?);

    let owner_activation = Arc::new(activation(
        ExecutableTarget::new(owner_component.id, owner_component.revision),
        "test:owner-entity",
        parsed_agent_id.agent_type.clone(),
        tool_name.clone(),
        context.account_id,
        FilesystemCapability::Incapable,
    ));
    let entity = AgentEntity::Tool(tool_name);
    let host = active_agent.entity_instance_host(&owner_activation, owner_metadata.clone())?;
    let owner_oplog = active_agent.execution().oplog();
    let before = owner_oplog.current_oplog_index().await;
    let hosted = host.instantiate_entity().await?;
    let outer_start = before.next();
    let invocation_id = EntityInvocationId::new(
        OwnedAgentEntityId {
            owner: owner_id.clone(),
            entity,
        },
        outer_start,
    )
    .unwrap();
    let principal = Principal::Agent(AgentPrincipal {
        agent_id: owner_id.agent_id.clone(),
    });
    let scope = EntityInvocationScope::new(
        invocation_id,
        before,
        owner_activation,
        principal.clone(),
        InvocationExecutionMode::Live,
    )
    .unwrap();

    let invoked_scope = scope.clone();
    hosted
        .invoke_scoped(scope.clone(), move |instance, store| {
            Box::pin(async move {
                assert_eq!(store.data().entity_invocation_scope(), Some(&invoked_scope));
                assert!(
                    PreopensHost::get_directories(store.data_mut().durable_ctx_mut())
                        .await
                        .map_err(|error| {
                            golem_service_base::error::worker_executor::WorkerExecutorError::runtime(
                                error.to_string(),
                            )
                        })?
                        .is_empty(),
                    "filesystem-incapable entity Stores must have no preopens"
                );
                let parsed_agent_id = store
                    .data()
                    .parsed_agent_id()
                    .expect("entity context keeps the owner routing identity");
                initialize_entity(instance, store, &parsed_agent_id, principal.clone()).await?;
                invoke_sleep_p3(instance, store, &parsed_agent_id, principal, 0).await?;
                let initial_bytes = store.data().durable_ctx().total_linear_memory_size();
                let memory = store
                    .linear_memories()
                    .iter()
                    .find_map(|memory| match memory {
                        StoreMemory::Unshared(memory) => Some(*memory),
                        StoreMemory::Shared(_) => None,
                    })
                    .expect("test component must have unshared linear memory");
                for _ in 0..2 {
                    memory.grow_async(&mut *store, 1).await.map_err(|error| {
                        golem_service_base::error::worker_executor::WorkerExecutorError::runtime(
                            format!("failed to grow entity test memory: {error}"),
                        )
                    })?;
                }
                assert_eq!(
                    store.data().durable_ctx().total_linear_memory_size(),
                    initial_bytes + 2 * 65_536,
                    "post-instantiation entity growth must use the reconciled memory total"
                );
                Ok(())
            })
        })
        .await?;

    let after = owner_oplog.current_oplog_index().await;
    assert!(after > before);
    let appended = owner_oplog
        .read_exact(before.next(), u64::from(after) - u64::from(before))
        .await;
    assert!(
        appended
            .values()
            .any(|entry| matches!(entry, OplogEntry::Start { .. })),
        "the entity export's durable clock call must append to the owner oplog"
    );
    assert!(
        appended
            .values()
            .any(|entry| matches!(entry, OplogEntry::End { .. })),
        "the entity export's durable clock call must finish in the owner oplog"
    );
    assert!(
        appended
            .values()
            .all(|entry| !matches!(entry, OplogEntry::GrowMemory { .. })),
        "entity memory growth must not mutate the primary's durable startup-memory state"
    );

    let second_host =
        active_agent.entity_instance_host(scope.activation(), owner_metadata.clone())?;
    let second_hosted = second_host.instantiate_entity().await?;
    let second_start = after.next();
    let second_scope = EntityInvocationScope::new(
        EntityInvocationId::new(
            OwnedAgentEntityId {
                owner: owner_id.clone(),
                entity: scope.invocation_id().entity().clone(),
            },
            second_start,
        )
        .unwrap(),
        outer_start,
        scope.activation().clone(),
        scope.calling_principal().clone(),
        InvocationExecutionMode::Live,
    )
    .unwrap();
    let expected_error = second_hosted
        .invoke_scoped(second_scope.clone(), move |_, store| {
            Box::pin(async move {
                assert_eq!(store.data().entity_invocation_scope(), Some(&second_scope));
                Err::<(), _>(
                    golem_service_base::error::worker_executor::WorkerExecutorError::runtime(
                        "synthetic entity export failed",
                    ),
                )
            })
        })
        .await
        .unwrap_err();
    assert!(
        expected_error
            .to_string()
            .contains("synthetic entity export failed")
    );

    let cancellation_host =
        active_agent.entity_instance_host(scope.activation(), owner_metadata.clone())?;
    let cancellation_hosted = cancellation_host.instantiate_entity().await?;
    let cancellation_start = second_start.next();
    let cancellation_scope = EntityInvocationScope::new(
        EntityInvocationId::new(
            OwnedAgentEntityId {
                owner: owner_id.clone(),
                entity: scope.invocation_id().entity().clone(),
            },
            cancellation_start,
        )
        .unwrap(),
        second_start,
        scope.activation().clone(),
        scope.calling_principal().clone(),
        InvocationExecutionMode::Live,
    )
    .unwrap();
    let (sleep_started, sleep_started_rx) = tokio::sync::oneshot::channel();
    let (completed, completed_rx) = tokio::sync::oneshot::channel();
    let invoked_scope = cancellation_scope.clone();
    let cancellation_principal = scope.calling_principal().clone();
    let cancellation_oplog = owner_oplog.clone();
    let caller = tokio::spawn(cancellation_hosted.invoke_scoped(
        cancellation_scope,
        move |instance, store| {
            Box::pin(async move {
                assert_eq!(store.data().entity_invocation_scope(), Some(&invoked_scope));
                let parsed_agent_id = store
                    .data()
                    .parsed_agent_id()
                    .expect("entity context keeps the owner routing identity");
                initialize_entity(
                    instance,
                    store,
                    &parsed_agent_id,
                    cancellation_principal.clone(),
                )
                .await?;
                let before_sleep = cancellation_oplog.current_oplog_index().await;
                let _ = sleep_started.send(before_sleep);
                invoke_sleep_p3(instance, store, &parsed_agent_id, cancellation_principal, 1)
                    .await?;
                Ok(CompletionSignal(Some(completed)))
            })
        },
    ));
    let before_sleep = tokio::time::timeout(std::time::Duration::from_secs(5), sleep_started_rx)
        .await
        .expect("entity sleep must start")?;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let latest = owner_oplog.current_oplog_index().await;
            if latest > before_sleep {
                let entries = owner_oplog
                    .read_exact(
                        before_sleep.next(),
                        u64::from(latest) - u64::from(before_sleep),
                    )
                    .await;
                if entries
                    .values()
                    .any(|entry| matches!(entry, OplogEntry::Start { .. }))
                {
                    break;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("entity durable sleep must append its Start before caller cancellation");
    assert!(!caller.is_finished());
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    tokio::time::timeout(std::time::Duration::from_secs(5), completed_rx)
        .await
        .expect("caller-independent entity execution must finish")
        .expect("entity execution must return its completion signal");

    let panic_host = active_agent.entity_instance_host(scope.activation(), owner_metadata)?;
    let panic_hosted = panic_host.instantiate_entity().await?;
    let panic_start = cancellation_start.next();
    let panic_scope = EntityInvocationScope::new(
        EntityInvocationId::new(
            OwnedAgentEntityId {
                owner: owner_id,
                entity: scope.invocation_id().entity().clone(),
            },
            panic_start,
        )
        .unwrap(),
        cancellation_start,
        scope.activation().clone(),
        scope.calling_principal().clone(),
        InvocationExecutionMode::Live,
    )
    .unwrap();
    let invoked_scope = panic_scope.clone();
    let panic_error = panic_hosted
        .invoke_scoped(panic_scope, move |_, store| {
            Box::pin(async move {
                assert_eq!(store.data().entity_invocation_scope(), Some(&invoked_scope));
                panic!("synthetic entity export panic");
                #[allow(unreachable_code)]
                Ok::<(), golem_service_base::error::worker_executor::WorkerExecutorError>(())
            })
        })
        .await
        .unwrap_err();
    assert!(panic_error.to_string().contains("panicked"));

    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn entity_slots_lane_and_all_live_call_modes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Clock", "phase-three-owner");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "healthcheck", data_value!())
        .await?;

    let owner_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let active_agent = executor
        .active_agent(&owner_id)
        .await
        .expect("owner must be active");
    let owner_metadata =
        owner_component_metadata(&active_agent, component.id, component.revision).await?;
    let entity = AgentEntity::Tool(ToolName::try_from("phase-three-tool").unwrap());
    let principal = Principal::Agent(AgentPrincipal {
        agent_id: owner_id.agent_id.clone(),
    });
    let incapable_activation = Arc::new(activation(
        ExecutableTarget::new(component.id, component.revision),
        "test:phase-three-incapable",
        agent_id.agent_type.clone(),
        ToolName::try_from("phase-three-tool").unwrap(),
        context.account_id,
        FilesystemCapability::Incapable,
    ));
    let capable_activation = Arc::new(activation(
        ExecutableTarget::new(component.id, component.revision),
        "test:phase-three-capable",
        agent_id.agent_type.clone(),
        ToolName::try_from("phase-three-tool").unwrap(),
        context.account_id,
        FilesystemCapability::Capable,
    ));
    let lane = active_agent.execution().lane();
    let root_start = active_agent
        .execution()
        .oplog()
        .current_oplog_index()
        .await
        .next();
    let root_id = OwnerInvocationId::Agent(root_start);
    let root = lane.enter_primary(root_start)?.acquire().await?;
    let slot = active_agent.entity_slot(&entity);

    // Incapable synchronous, asynchronous, and fire-and-forget calls all begin immediately while
    // the primary still owns the filesystem lane, and same-entity calls use distinct Stores.
    let overlap_started = Arc::new(tokio::sync::Barrier::new(4));
    let overlap_release = Arc::new(tokio::sync::Barrier::new(4));
    let sync_scope = invocation_scope(
        &owner_id,
        &entity,
        root_start.next(),
        root_start,
        incapable_activation.clone(),
        principal.clone(),
    );
    let sync_started = overlap_started.clone();
    let sync_release = overlap_release.clone();
    let sync = active_agent.start_entity_invocation(
        root_id.clone(),
        sync_scope,
        owner_metadata.clone(),
        EntityCallMode::Synchronous,
        move |_, store| {
            Box::pin(async move {
                let store_address = store as *mut Store<TestWorkerCtx> as usize;
                sync_started.wait().await;
                sync_release.wait().await;
                Ok(store_address)
            })
        },
        std::future::ready,
    )?;
    let async_scope = invocation_scope(
        &owner_id,
        &entity,
        root_start.next().next(),
        root_start,
        incapable_activation.clone(),
        principal.clone(),
    );
    let async_started = overlap_started.clone();
    let async_release = overlap_release.clone();
    let asynchronous = active_agent.start_entity_invocation(
        root_id.clone(),
        async_scope,
        owner_metadata.clone(),
        EntityCallMode::Asynchronous,
        move |_, store| {
            Box::pin(async move {
                let store_address = store as *mut Store<TestWorkerCtx> as usize;
                async_started.wait().await;
                async_release.wait().await;
                Ok(store_address)
            })
        },
        std::future::ready,
    )?;
    let fire_scope = invocation_scope(
        &owner_id,
        &entity,
        root_start.next().next().next(),
        root_start,
        incapable_activation.clone(),
        principal.clone(),
    );
    let fire_started = overlap_started.clone();
    let fire_release = overlap_release.clone();
    let fire_and_forget = active_agent.start_entity_invocation(
        root_id.clone(),
        fire_scope,
        owner_metadata.clone(),
        EntityCallMode::FireAndForget,
        move |_, store| {
            Box::pin(async move {
                let store_address = store as *mut Store<TestWorkerCtx> as usize;
                fire_started.wait().await;
                fire_release.wait().await;
                Ok(store_address)
            })
        },
        std::future::ready,
    )?;

    tokio::time::timeout(std::time::Duration::from_secs(10), overlap_started.wait())
        .await
        .expect("all filesystem-incapable bodies must overlap");
    assert_eq!(slot.active_invocation_count(), 3);
    assert_eq!(
        active_agent.primary().eviction_class().await,
        None,
        "an owner with active entity Stores must not be evictable"
    );
    assert!(
        !active_agent
            .primary()
            .stop_if_evictable(EvictionClass::WarmRunnable)
            .await,
        "eviction must not stop an owner while any entity Store is active"
    );
    let duplicate_scope = invocation_scope(
        &owner_id,
        &entity,
        root_start.next(),
        root_start,
        incapable_activation.clone(),
        principal.clone(),
    );
    let duplicate = active_agent.start_entity_invocation(
        root_id.clone(),
        duplicate_scope,
        owner_metadata.clone(),
        EntityCallMode::Synchronous,
        move |_, _| Box::pin(async move { Ok(()) }),
        std::future::ready,
    );
    assert!(duplicate.is_err());
    assert_eq!(
        slot.active_invocation_count(),
        3,
        "rejecting a duplicate runtime ID must preserve the original registration"
    );
    assert!(
        slot.charged_linear_memory_bytes() > 0,
        "every resident entity Store must contribute its actual linear-memory charge"
    );
    overlap_release.wait().await;
    let sync_store = sync.await_result(&root_id).await?;
    let async_store = asynchronous.await_result(&root_id).await?;
    let fire_store = fire_and_forget.join().await?;
    assert_ne!(sync_store, async_store);
    assert_ne!(sync_store, fire_store);
    assert_ne!(async_store, fire_store);
    assert_eq!(slot.active_invocation_count(), 0);
    assert_eq!(slot.charged_linear_memory_bytes(), 0);

    // Two synchronous capable calls to the same slot serialize on the owner lane, not the slot.
    let first_start = root_start.next().next().next().next();
    let first_scope = invocation_scope(
        &owner_id,
        &entity,
        first_start,
        root_start,
        capable_activation.clone(),
        principal.clone(),
    );
    let (first_started, first_started_rx) = tokio::sync::oneshot::channel();
    let (release_first, release_first_rx) = tokio::sync::oneshot::channel();
    let (first_finalizing, first_finalizing_rx) = tokio::sync::oneshot::channel();
    let (release_first_finalizer, release_first_finalizer_rx) = tokio::sync::oneshot::channel();
    let first = active_agent.start_entity_invocation(
        root_id.clone(),
        first_scope,
        owner_metadata.clone(),
        EntityCallMode::Synchronous,
        move |_, store| {
            Box::pin(async move {
                assert_eq!(
                    PreopensHost::get_directories(store.data_mut().durable_ctx_mut())
                        .await
                        .map_err(|error| {
                            golem_service_base::error::worker_executor::WorkerExecutorError::runtime(
                                error.to_string(),
                            )
                        })?
                        .len(),
                    2,
                    "filesystem-capable entity Stores retain the standard preopens"
                );
                let _ = first_started.send(());
                let _ = release_first_rx.await;
                Ok(())
            })
        },
        move |result| async move {
            let _ = first_finalizing.send(());
            let _ = release_first_finalizer_rx.await;
            result
        },
    )?;
    let second_scope = invocation_scope(
        &owner_id,
        &entity,
        first_start.next(),
        root_start,
        capable_activation.clone(),
        principal.clone(),
    );
    let (second_started, second_started_rx) = tokio::sync::oneshot::channel();
    let second = active_agent.start_entity_invocation(
        root_id.clone(),
        second_scope,
        owner_metadata.clone(),
        EntityCallMode::Synchronous,
        move |_, _| {
            Box::pin(async move {
                let _ = second_started.send(());
                Ok(())
            })
        },
        std::future::ready,
    )?;
    tokio::time::timeout(std::time::Duration::from_secs(10), first_started_rx)
        .await
        .expect("first capable body must start")?;
    let mut second_started_rx = Box::pin(second_started_rx);
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            &mut second_started_rx
        )
        .await
        .is_err(),
        "the second capable body must wait for the first terminal"
    );
    let _ = release_first.send(());
    let first_root = root_id.clone();
    let first_waiter = tokio::spawn(async move { first.await_result(&first_root).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), first_finalizing_rx)
        .await
        .expect("outer-terminal finalizer must run after the body")?;
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            &mut second_started_rx
        )
        .await
        .is_err(),
        "the lane must remain held until the durable outer-terminal finalizer completes"
    );
    let _ = release_first_finalizer.send(());
    first_waiter.await??;
    tokio::time::timeout(std::time::Duration::from_secs(10), &mut second_started_rx)
        .await
        .expect("second capable body must start after first terminal")?;
    second.await_result(&root_id).await?;

    // A body panic is converted to a terminal error while the registration and lane remain held
    // through the durable outer-terminal finalizer.
    let panic_start = first_start.next().next();
    let panic_scope = invocation_scope(
        &owner_id,
        &entity,
        panic_start,
        root_start,
        capable_activation.clone(),
        principal.clone(),
    );
    let panic_invocation = OwnerInvocationId::Entity(panic_scope.invocation_id().clone());
    let (panic_finalizing, panic_finalizing_rx) = tokio::sync::oneshot::channel();
    let (release_panic_finalizer, release_panic_finalizer_rx) = tokio::sync::oneshot::channel();
    let panicking = active_agent.start_entity_invocation(
        root_id.clone(),
        panic_scope,
        owner_metadata.clone(),
        EntityCallMode::Synchronous,
        move |_, _| {
            Box::pin(async move {
                panic!("phase-three entity body panic");
                #[allow(unreachable_code)]
                Ok(())
            })
        },
        move |result| async move {
            assert!(result.is_err());
            let _ = panic_finalizing.send(());
            let _ = release_panic_finalizer_rx.await;
            result
        },
    )?;
    let panic_root = root_id.clone();
    let panic_waiter = tokio::spawn(async move { panicking.await_result(&panic_root).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), panic_finalizing_rx)
        .await
        .expect("a body panic must reach the outer-terminal finalizer")?;
    assert_eq!(slot.active_invocation_count(), 1);
    assert_eq!(lane.holder(), Some(panic_invocation));
    let _ = release_panic_finalizer.send(());
    assert!(panic_waiter.await?.is_err());

    // A capable async body is launch-deferred until get/await establishes the causal lane grant.
    let async_capable_scope = invocation_scope(
        &owner_id,
        &entity,
        panic_start.next(),
        root_start,
        capable_activation.clone(),
        principal.clone(),
    );
    let (async_started, async_started_rx) = tokio::sync::oneshot::channel();
    let (release_async, release_async_rx) = tokio::sync::oneshot::channel();
    let async_capable = active_agent.start_entity_invocation(
        root_id.clone(),
        async_capable_scope,
        owner_metadata.clone(),
        EntityCallMode::Asynchronous,
        move |_, _| {
            Box::pin(async move {
                let _ = async_started.send(());
                let _ = release_async_rx.await;
                Ok(())
            })
        },
        std::future::ready,
    )?;
    let mut async_started_rx = Box::pin(async_started_rx);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut async_started_rx)
            .await
            .is_err(),
        "capable async body must not start at Start"
    );
    assert_eq!(slot.active_invocation_count(), 1);
    assert_eq!(
        slot.charged_linear_memory_bytes(),
        0,
        "a launch-deferred invocation has no Store charge before its lane grant"
    );
    let wait_root = root_id.clone();
    let async_waiter = tokio::spawn(async move { async_capable.await_result(&wait_root).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), &mut async_started_rx)
        .await
        .expect("await must grant the capable body the lane")?;
    assert!(slot.charged_linear_memory_bytes() > 0);
    let _ = release_async.send(());
    async_waiter.await??;

    // A dropped capable fire-and-forget handle does not suppress its body; it becomes eligible at
    // the launching invocation's terminal and still drops its Store when done.
    let fire_capable_scope = invocation_scope(
        &owner_id,
        &entity,
        panic_start.next().next(),
        root_start,
        capable_activation,
        principal,
    );
    let (fire_started, fire_started_rx) = tokio::sync::oneshot::channel();
    let (fire_completed, fire_completed_rx) = tokio::sync::oneshot::channel();
    let fire_capable = active_agent.start_entity_invocation(
        root_id,
        fire_capable_scope,
        owner_metadata.clone(),
        EntityCallMode::FireAndForget,
        move |_, _| {
            Box::pin(async move {
                let _ = fire_started.send(());
                Ok(CompletionSignal(Some(fire_completed)))
            })
        },
        std::future::ready,
    )?;
    let mut fire_started_rx = Box::pin(fire_started_rx);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut fire_started_rx)
            .await
            .is_err()
    );
    drop(fire_capable);
    drop(root);
    tokio::time::timeout(std::time::Duration::from_secs(10), &mut fire_started_rx)
        .await
        .expect("primary terminal must grant a queued fire-and-forget body")?;
    tokio::time::timeout(std::time::Duration::from_secs(10), fire_completed_rx)
        .await
        .expect("dropped fire-and-forget handle must not abort its body")?;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while slot.active_invocation_count() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("every completed entity Store must unregister and drop");
    assert_eq!(slot.charged_linear_memory_bytes(), 0);

    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn middleware_and_nested_tool_invocations_use_generic_slots_scopes_and_metadata(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Clock", "middleware-owner");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "healthcheck", data_value!())
        .await?;

    let owner_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let active_agent = executor
        .active_agent(&owner_id)
        .await
        .expect("owner must be active");
    let owner_metadata =
        owner_component_metadata(&active_agent, component.id, component.revision).await?;
    let common_name = "chain-layer";
    let middleware_entity = AgentEntity::ToolMiddleware(
        ToolMiddlewareName::try_from(common_name).expect("valid middleware name"),
    );
    let tool_name = ToolName::try_from(common_name).expect("valid tool name");
    let tool_entity = AgentEntity::Tool(tool_name.clone());
    let executable = ExecutableTarget::new(component.id, component.revision);
    let middleware_activation = Arc::new(middleware_activation(
        executable.clone(),
        ToolMiddlewareName::try_from(common_name).unwrap(),
    ));
    let tool_activation = Arc::new(activation(
        executable,
        "test:nested-tool",
        agent_id.agent_type.clone(),
        tool_name,
        context.account_id,
        FilesystemCapability::Incapable,
    ));
    let principal = Principal::Agent(AgentPrincipal {
        agent_id: owner_id.agent_id.clone(),
    });
    let lane = active_agent.execution().lane();
    let root_start = active_agent
        .execution()
        .oplog()
        .current_oplog_index()
        .await
        .next();
    let root_id = OwnerInvocationId::Agent(root_start);
    let root = lane.enter_primary(root_start)?.acquire().await?;
    let middleware_start = root_start.next();
    let middleware_scope = invocation_scope(
        &owner_id,
        &middleware_entity,
        middleware_start,
        root_start,
        middleware_activation,
        principal.clone(),
    );
    let middleware_invocation = middleware_scope.invocation_id().clone();
    let nested_start = middleware_start.next();
    let nested_scope = invocation_scope(
        &owner_id,
        &tool_entity,
        nested_start,
        middleware_start,
        tool_activation.clone(),
        principal,
    );
    let expected_middleware_scope = middleware_scope.clone();
    let expected_nested_scope = nested_scope.clone();
    let nested_parent = OwnerInvocationId::Entity(middleware_invocation.clone());
    let nested_parent_for_body = nested_parent.clone();
    let nested_active_agent = active_agent.clone();
    let nested_owner_metadata = owner_metadata.clone();
    let (nested_started, nested_started_rx) = tokio::sync::oneshot::channel();
    let (release_nested, release_nested_rx) = tokio::sync::oneshot::channel();
    let middleware = active_agent.start_entity_invocation(
        root_id.clone(),
        middleware_scope,
        owner_metadata.clone(),
        EntityCallMode::Synchronous,
        move |_, store| {
            Box::pin(async move {
                assert_eq!(
                    store.data().entity_invocation_scope(),
                    Some(&expected_middleware_scope)
                );
                let nested = nested_active_agent.start_entity_invocation(
                    nested_parent_for_body.clone(),
                    nested_scope,
                    nested_owner_metadata,
                    EntityCallMode::Synchronous,
                    move |_, store| {
                        Box::pin(async move {
                            let installed = store
                                .data()
                                .entity_invocation_scope()
                                .expect("nested scope must be installed");
                            assert_eq!(installed, &expected_nested_scope);
                            assert_eq!(installed.parent_start_index(), middleware_start);
                            let _ = nested_started.send(());
                            let _ = release_nested_rx.await;
                            Ok(())
                        })
                    },
                    std::future::ready,
                )?;
                nested.await_result(&nested_parent_for_body).await
            })
        },
        std::future::ready,
    )?;

    tokio::time::timeout(std::time::Duration::from_secs(10), nested_started_rx)
        .await
        .expect("nested tool invocation must start")?;
    let metadata = active_agent.entity_metadata();
    assert_eq!(metadata.owner_id, owner_id);
    assert!(metadata.accepting_entities);
    assert_eq!(metadata.slots.len(), 2);
    assert!(metadata.slots.iter().any(|slot| {
        slot.entity_id.entity == middleware_entity
            && slot.invocations.len() == 1
            && slot.invocations[0].invocation_id == middleware_invocation
    }));
    assert!(metadata.slots.iter().any(|slot| {
        slot.entity_id.entity == tool_entity
            && slot.invocations.len() == 1
            && slot.invocations[0].invocation_id.start_index() == nested_start
            && slot.invocations[0].executable == *tool_activation.executable()
    }));
    assert!(
        active_agent
            .entity_slot_metadata(&OwnedAgentEntityId {
                owner: owner_id.clone(),
                entity: AgentEntity::Tool(ToolName::try_from("unknown-tool").unwrap()),
            })
            .is_none(),
        "inspection must not create unknown slots"
    );
    assert_eq!(active_agent.entity_metadata().slots.len(), 2);

    let _ = release_nested.send(());
    middleware.await_result(&root_id).await?;
    drop(root);
    assert!(
        active_agent
            .entity_metadata()
            .slots
            .iter()
            .all(|slot| slot.invocations.is_empty()),
        "completed Stores leave only vacant in-memory slots, never durable child status"
    );

    executor
        .interrupt_with_optional_recovery(&worker_id, true)
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "healthcheck", data_value!())
        .await?;
    assert!(
        active_agent.entity_metadata().accepting_entities,
        "a new primary generation must reopen entity admission after an in-place restart"
    );
    executor
        .interrupt_with_optional_recovery(&worker_id, false)
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "healthcheck", data_value!())
        .await?;
    assert!(
        active_agent.entity_metadata().accepting_entities,
        "a resumed primary generation must reopen entity admission after a terminal interrupt"
    );

    let delete_parent_start = nested_start.next();
    let delete_parent = OwnerInvocationId::Agent(delete_parent_start);
    let delete_parent_permit = lane.enter_primary(delete_parent_start)?.acquire().await?;
    let delete_start = delete_parent_start.next();
    let delete_scope = invocation_scope(
        &owner_id,
        &tool_entity,
        delete_start,
        delete_parent_start,
        tool_activation.clone(),
        Principal::Agent(AgentPrincipal {
            agent_id: owner_id.agent_id.clone(),
        }),
    );
    let (delete_body_started, delete_body_started_rx) = tokio::sync::oneshot::channel();
    let deleting = active_agent.start_entity_invocation(
        delete_parent,
        delete_scope,
        owner_metadata.clone(),
        EntityCallMode::FireAndForget,
        move |_, _| {
            Box::pin(async move {
                let _ = delete_body_started.send(());
                std::future::pending::<
                    Result<(), golem_service_base::error::worker_executor::WorkerExecutorError>,
                >()
                .await
            })
        },
        std::future::ready,
    )?;
    tokio::time::timeout(std::time::Duration::from_secs(10), delete_body_started_rx)
        .await
        .expect("entity body must start before owner deletion")?;
    assert!(
        !active_agent.primary().stop_if_idle().await,
        "automatic owner unload must not detach an active entity body"
    );
    assert!(active_agent.entity_metadata().accepting_entities);
    drop(delete_parent_permit);

    executor.delete_worker(&worker_id).await?;
    let deletion_error = deleting
        .join()
        .await
        .expect_err("owner deletion must cancel active entity bodies");
    assert!(deletion_error.to_string().contains("cancelled"));
    assert!(!active_agent.entity_metadata().accepting_entities);
    let rejected_scope = invocation_scope(
        &owner_id,
        &tool_entity,
        delete_start.next(),
        delete_start,
        tool_activation,
        Principal::Agent(AgentPrincipal {
            agent_id: owner_id.agent_id.clone(),
        }),
    );
    let rejected = active_agent.start_entity_invocation(
        OwnerInvocationId::Entity(
            EntityInvocationId::new(
                OwnedAgentEntityId {
                    owner: owner_id.clone(),
                    entity: tool_entity,
                },
                delete_start,
            )
            .expect("valid rejected invocation id"),
        ),
        rejected_scope,
        owner_metadata.clone(),
        EntityCallMode::Synchronous,
        |_, _| Box::pin(async { Ok(()) }),
        std::future::ready,
    );
    let rejection_error = match rejected {
        Ok(_) => panic!("deleted owner must reject new entity bodies"),
        Err(error) => error,
    };
    assert!(rejection_error.to_string().contains("fenced"));
    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn entity_filesystem_streams_share_root_and_block_executor_inspection(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("FileSystem", "entity-filesystem-owner");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "run_directories", data_value!())
        .await?;

    let owner_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let active_agent = executor
        .active_agent(&owner_id)
        .await
        .expect("owner must be active");
    let owner_metadata =
        owner_component_metadata(&active_agent, component.id, component.revision).await?;
    let tool_name = ToolName::try_from("filesystem-stream-tool").unwrap();
    let entity = AgentEntity::Tool(tool_name.clone());
    let principal = Principal::Agent(AgentPrincipal {
        agent_id: owner_id.agent_id.clone(),
    });
    let activation = Arc::new(activation(
        ExecutableTarget::new(component.id, component.revision),
        "test:filesystem-stream-tool",
        agent_id.agent_type.clone(),
        tool_name,
        context.account_id,
        FilesystemCapability::Capable,
    ));
    let lane = active_agent.execution().lane();
    let root_start = active_agent
        .execution()
        .oplog()
        .current_oplog_index()
        .await
        .next();
    let root_id = OwnerInvocationId::Agent(root_start);
    let root = lane.enter_primary(root_start)?.acquire().await?;
    let scope = invocation_scope(
        &owner_id,
        &entity,
        root_start.next(),
        root_start,
        activation,
        principal.clone(),
    );
    let (body_started, body_started_rx) = tokio::sync::oneshot::channel();
    let (release_body, release_body_rx) = tokio::sync::oneshot::channel();
    let body = active_agent.start_entity_invocation(
        root_id.clone(),
        scope,
        owner_metadata.clone(),
        EntityCallMode::Synchronous,
        move |instance, store| {
            Box::pin(async move {
                let parsed_agent_id = store
                    .data()
                    .parsed_agent_id()
                    .expect("entity context keeps the owner routing identity");
                initialize_entity(instance, store, &parsed_agent_id, principal.clone()).await?;
                let result = invoke_entity_method(
                    instance,
                    store,
                    &parsed_agent_id,
                    principal,
                    "write_zeroes_to_file_via_stream",
                    data_value!("/entity-stream.bin", 131_072_u64)
                        .value()
                        .clone(),
                )
                .await?;
                assert!(matches!(result, AgentInvocationResult::AgentMethod { .. }));
                let _ = body_started.send(());
                let _ = release_body_rx.await;
                Ok(())
            })
        },
        std::future::ready,
    )?;
    tokio::time::timeout(std::time::Duration::from_secs(10), body_started_rx)
        .await
        .expect("filesystem-capable entity body must start")?;

    let inspecting_executor = executor.clone();
    let inspecting_worker = worker_id.clone();
    let mut inspection = tokio::spawn(async move {
        inspecting_executor
            .get_file_contents(&inspecting_worker, "/entity-stream.bin")
            .await
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut inspection)
            .await
            .is_err(),
        "executor filesystem inspection must not overlap a capable entity body"
    );

    let _ = release_body.send(());
    body.await_result(&root_id).await?;
    drop(root);
    let contents = tokio::time::timeout(std::time::Duration::from_secs(10), inspection)
        .await
        .expect("inspection must resume after the entity returns the lane")??;
    assert_eq!(contents.len(), 131_072);
    assert!(contents.iter().all(|byte| *byte == 0));
    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn filesystem_capable_entity_stream_replays_on_owner_filesystem(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("FileSystem", "entity-filesystem-replay-owner");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "run_directories", data_value!())
        .await?;
    let owner_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let active_agent = executor
        .active_agent(&owner_id)
        .await
        .expect("owner must be active");
    let owner_metadata =
        owner_component_metadata(&active_agent, component.id, component.revision).await?;
    let tool_name = ToolName::try_from("filesystem-replay-tool").unwrap();
    let entity = AgentEntity::Tool(tool_name.clone());
    let principal = Principal::Agent(AgentPrincipal {
        agent_id: owner_id.agent_id.clone(),
    });
    let activation = Arc::new(activation(
        ExecutableTarget::new(component.id, component.revision),
        "test:filesystem-replay-tool",
        agent_id.agent_type.clone(),
        tool_name,
        context.account_id,
        FilesystemCapability::Capable,
    ));
    let lane = active_agent.execution().lane();
    let parent_start = active_agent.execution().oplog().current_oplog_index().await;
    let entity_start = parent_start.next();
    let parent_id = OwnerInvocationId::Agent(parent_start);

    let primary = lane.enter_primary(parent_start)?.acquire().await?;
    let live_scope = invocation_scope(
        &owner_id,
        &entity,
        entity_start,
        parent_start,
        activation.clone(),
        principal.clone(),
    );
    let live_principal = principal.clone();
    let live = active_agent.start_entity_invocation(
        parent_id.clone(),
        live_scope,
        owner_metadata.clone(),
        EntityCallMode::Synchronous,
        move |instance, store| {
            Box::pin(async move {
                let parsed_agent_id = store
                    .data()
                    .parsed_agent_id()
                    .expect("entity context keeps the owner routing identity");
                initialize_entity(instance, store, &parsed_agent_id, live_principal.clone())
                    .await?;
                invoke_entity_method(
                    instance,
                    store,
                    &parsed_agent_id,
                    live_principal.clone(),
                    "write_zeroes_to_file_via_stream",
                    data_value!("/entity-replayed-stream.bin", 1024_u64)
                        .value()
                        .clone(),
                )
                .await?;
                invoke_entity_method(
                    instance,
                    store,
                    &parsed_agent_id,
                    live_principal,
                    "read_file",
                    data_value!("/entity-replayed-stream.bin").value().clone(),
                )
                .await
            })
        },
        std::future::ready,
    )?;
    let live_result = live.await_result(&parent_id).await?;
    drop(primary);
    active_agent.execution().commit(CommitLevel::Always).await;

    active_agent
        .execution()
        .install_replay_generation(
            DeletedRegions::from_regions([OplogRegion::from_index_range(
                OplogIndex::INITIAL.next()..=parent_start,
            )]),
            None,
        )
        .await?;

    let replay_primary = lane.enter_primary(parent_start)?.acquire().await?;
    let replay_scope = replay_invocation_scope(
        &owner_id,
        &entity,
        entity_start,
        parent_start,
        activation,
        principal.clone(),
    );
    let replay = active_agent.start_entity_invocation(
        parent_id.clone(),
        replay_scope,
        owner_metadata.clone(),
        EntityCallMode::Synchronous,
        move |instance, store| {
            Box::pin(async move {
                let parsed_agent_id = store
                    .data()
                    .parsed_agent_id()
                    .expect("entity context keeps the owner routing identity");
                initialize_entity(instance, store, &parsed_agent_id, principal.clone()).await?;
                invoke_entity_method(
                    instance,
                    store,
                    &parsed_agent_id,
                    principal.clone(),
                    "write_zeroes_to_file_via_stream",
                    data_value!("/entity-replayed-stream.bin", 1024_u64)
                        .value()
                        .clone(),
                )
                .await?;
                invoke_entity_method(
                    instance,
                    store,
                    &parsed_agent_id,
                    principal,
                    "read_file",
                    data_value!("/entity-replayed-stream.bin").value().clone(),
                )
                .await
            })
        },
        std::future::ready,
    )?;
    let replay_result = replay.await_result(&parent_id).await?;
    drop(replay_primary);

    assert!(live_result.replay_equivalent(&replay_result));
    let replayed_contents = executor
        .get_file_contents(&worker_id, "/entity-replayed-stream.bin")
        .await?;
    assert_eq!(
        replayed_contents.len(),
        1024,
        "replaying the entity body must retain its stream-written local effect"
    );
    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn entity_provisioning_is_lane_scoped_idempotent_and_conflict_checked(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("FileSystem", "entity-provision-owner");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "run_directories", data_value!())
        .await?;

    let initial_contents = b"entity activation contents".to_vec();
    let conflicting_contents = b"conflicting activation contents".to_vec();
    let initial_hash = deps
        .initial_agent_files_service
        .put_if_not_exists(
            context.default_environment_id,
            initial_contents
                .clone()
                .map_error(widen_infallible::<anyhow::Error>)
                .map_item(|item| item.map_err(widen_infallible::<anyhow::Error>)),
        )
        .await?;
    let conflicting_hash = deps
        .initial_agent_files_service
        .put_if_not_exists(
            context.default_environment_id,
            conflicting_contents
                .clone()
                .map_error(widen_infallible::<anyhow::Error>)
                .map_item(|item| item.map_err(widen_infallible::<anyhow::Error>)),
        )
        .await?;
    let provision_path =
        AgentFilePath::from_abs_str("/entity-provisioned.txt").map_err(anyhow::Error::msg)?;
    let provisioned_file = InitialAgentFile {
        content_hash: initial_hash,
        path: provision_path.clone(),
        permissions: AgentFilePermissions::ReadWrite,
        size: initial_contents.len() as u64,
    };
    let conflicting_file = InitialAgentFile {
        content_hash: conflicting_hash,
        path: provision_path,
        permissions: AgentFilePermissions::ReadWrite,
        size: conflicting_contents.len() as u64,
    };

    let owner_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let active_agent = executor
        .active_agent(&owner_id)
        .await
        .expect("owner must be active");
    let owner_metadata =
        owner_component_metadata(&active_agent, component.id, component.revision).await?;
    let tool_name = ToolName::try_from("provisioning-tool").unwrap();
    let entity = AgentEntity::Tool(tool_name.clone());
    let principal = Principal::Agent(AgentPrincipal {
        agent_id: owner_id.agent_id.clone(),
    });
    let activation = Arc::new(activation_with_provision(
        ExecutableTarget::new(component.id, component.revision),
        "test:provisioning-tool",
        agent_id.agent_type.clone(),
        tool_name.clone(),
        context.account_id,
        FilesystemCapability::Capable,
        ToolProvisionConfig {
            files: vec![provisioned_file],
            ..ToolProvisionConfig::default()
        },
    ));
    let conflicting_activation = Arc::new(activation_with_provision(
        ExecutableTarget::new(component.id, component.revision),
        "test:provisioning-tool-conflict",
        agent_id.agent_type.clone(),
        tool_name,
        context.account_id,
        FilesystemCapability::Capable,
        ToolProvisionConfig {
            files: vec![conflicting_file],
            ..ToolProvisionConfig::default()
        },
    ));
    let lane = active_agent.execution().lane();
    let root_start = active_agent
        .execution()
        .oplog()
        .current_oplog_index()
        .await
        .next();
    let root_id = OwnerInvocationId::Agent(root_start);
    let root = lane.enter_primary(root_start)?.acquire().await?;

    let first_scope = invocation_scope(
        &owner_id,
        &entity,
        root_start.next(),
        root_start,
        activation.clone(),
        principal.clone(),
    );
    let first = active_agent.start_entity_invocation(
        root_id.clone(),
        first_scope,
        owner_metadata.clone(),
        EntityCallMode::Synchronous,
        move |instance, store| {
            Box::pin(async move {
                let parsed_agent_id = store
                    .data()
                    .parsed_agent_id()
                    .expect("entity context keeps the owner routing identity");
                initialize_entity(instance, store, &parsed_agent_id, principal.clone()).await?;
                let result = invoke_entity_method(
                    instance,
                    store,
                    &parsed_agent_id,
                    principal,
                    "write_file",
                    data_value!("/entity-provisioned.txt", "modified by first invocation")
                        .value()
                        .clone(),
                )
                .await?;
                assert!(matches!(result, AgentInvocationResult::AgentMethod { .. }));
                Ok(())
            })
        },
        std::future::ready,
    )?;
    first.await_result(&root_id).await?;

    let second_scope = invocation_scope(
        &owner_id,
        &entity,
        root_start.next().next(),
        root_start,
        activation,
        Principal::Agent(AgentPrincipal {
            agent_id: owner_id.agent_id.clone(),
        }),
    );
    let second = active_agent.start_entity_invocation(
        root_id.clone(),
        second_scope,
        owner_metadata.clone(),
        EntityCallMode::Synchronous,
        move |_, _| Box::pin(async move { Ok(()) }),
        std::future::ready,
    )?;
    second.await_result(&root_id).await?;

    let conflict_scope = invocation_scope(
        &owner_id,
        &entity,
        root_start.next().next().next(),
        root_start,
        conflicting_activation,
        Principal::Agent(AgentPrincipal {
            agent_id: owner_id.agent_id.clone(),
        }),
    );
    let conflicting = active_agent.start_entity_invocation(
        root_id.clone(),
        conflict_scope,
        owner_metadata.clone(),
        EntityCallMode::Synchronous,
        move |_, _| {
            Box::pin(async move {
                panic!("a conflicting activation must fail before invoking its export");
                #[allow(unreachable_code)]
                Ok(())
            })
        },
        std::future::ready,
    )?;
    let conflict = conflicting.await_result(&root_id).await;
    assert!(
        conflict
            .unwrap_err()
            .to_string()
            .contains("conflicting owner filesystem provision declarations")
    );

    drop(root);
    let contents = executor
        .get_file_contents(&worker_id, "/entity-provisioned.txt")
        .await?;
    assert_eq!(contents, "modified by first invocation");
    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn entity_secret_policy_denies_unreadable_owner_secret(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_id = context.default_environment_id;
    let secret_path = CanonicalAgentSecretPath(vec!["secret".to_string()]);
    let nested_secret_path =
        CanonicalAgentSecretPath(vec!["nested".to_string(), "nestedSecret".to_string()]);
    // Inject deterministic backing values for the two secret declarations in ConfigAgent. The
    // entity policy below controls whether the guest receives opaque handles and, independently,
    // whether those handles can reveal these values.
    let secrets = HashMap::from([
        (
            secret_path.clone(),
            AgentSecret {
                id: AgentSecretId::new(),
                environment_id,
                path: secret_path,
                revision: AgentSecretRevision::INITIAL,
                secret_type: SchemaGraph::anonymous(SchemaType::string()),
                secret_value: Some(SchemaValue::String("owner-secret".to_string())),
            },
        ),
        (
            nested_secret_path.clone(),
            AgentSecret {
                id: AgentSecretId::new(),
                environment_id,
                path: nested_secret_path,
                revision: AgentSecretRevision::INITIAL,
                secret_type: SchemaGraph::anonymous(SchemaType::s32()),
                secret_value: Some(SchemaValue::S32(42)),
            },
        ),
    ]);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(Arc::new(FixedSecretsEnvironmentStateService {
                secrets,
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&environment_id, agent_sdk_rust)
        .with_agent_config(
            "ConfigAgent",
            vec![
                golem_common::model::worker::AgentConfigEntryDto {
                    path: vec!["foo".to_string()],
                    value: serde_json::json!(7).into(),
                },
                golem_common::model::worker::AgentConfigEntryDto {
                    path: vec!["bar".to_string()],
                    value: serde_json::json!("owner-config").into(),
                },
                golem_common::model::worker::AgentConfigEntryDto {
                    path: vec!["nested".to_string(), "a".to_string()],
                    value: serde_json::json!(true).into(),
                },
                golem_common::model::worker::AgentConfigEntryDto {
                    path: vec!["nested".to_string(), "b".to_string()],
                    value: serde_json::json!([1, 2]).into(),
                },
            ],
        )
        .store()
        .await?;
    let agent_id = agent_id!("ConfigAgent", "entity-secret-owner");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    // A normal owner invocation has no tool restriction and proves that the component's complete
    // config, including both secrets, is otherwise valid before exercising entity policy.
    executor
        .invoke_and_await_agent(&component, &agent_id, "echo_local_config", data_value!())
        .await?;

    let owner_id = OwnedAgentId::new(environment_id, &worker_id);
    let active_agent = executor
        .active_agent(&owner_id)
        .await
        .expect("owner must be active");
    let owner_metadata =
        owner_component_metadata(&active_agent, component.id, component.revision).await?;
    let tool_name = ToolName::try_from("restricted-secret-tool").unwrap();
    let entity = AgentEntity::Tool(tool_name.clone());
    // An empty readable scope must stop the entity at config resolution: the owner has valid
    // secrets and the executable knows their declarations, but no opaque handle may be minted for
    // this invocation.
    let activation = Arc::new(activation_with_secret_policy(
        ExecutableTarget::new(component.id, component.revision),
        "test:restricted-secret-tool",
        agent_id.agent_type.clone(),
        tool_name.clone(),
        context.account_id,
        SecretKeyScope::Keys(BTreeSet::new()),
        SecretKeyScope::Keys(BTreeSet::new()),
    ));
    let result = run_synchronous_entity_invocation(
        &active_agent,
        owner_metadata.clone(),
        &owner_id,
        &entity,
        activation,
        move |instance, store, principal| {
            Box::pin(async move {
                let parsed_agent_id = store
                    .data()
                    .parsed_agent_id()
                    .expect("entity context keeps the owner routing identity");
                initialize_entity(instance, store, &parsed_agent_id, principal.clone()).await?;
                invoke_entity_method(
                    instance,
                    store,
                    &parsed_agent_id,
                    principal,
                    "echo_local_config",
                    data_value!().value().clone(),
                )
                .await
            })
        },
    )
    .await;
    assert!(
        result.is_err(),
        "an entity with an empty readable-secret scope must not receive the owner's secret handles"
    );

    // Reading and revealing are separate permissions. This activation may resolve both owner
    // secrets into opaque handles, but the empty revealable scope must reject the guest method
    // when it tries to extract their values.
    let reveal_activation = Arc::new(activation_with_secret_policy(
        ExecutableTarget::new(component.id, component.revision),
        "test:restricted-secret-tool",
        agent_id.agent_type.clone(),
        tool_name,
        context.account_id,
        SecretKeyScope::Keys(BTreeSet::from([
            CanonicalAgentSecretPath(vec!["secret".to_string()]),
            CanonicalAgentSecretPath(vec!["nested".to_string(), "nestedSecret".to_string()]),
        ])),
        SecretKeyScope::Keys(BTreeSet::new()),
    ));
    let config_loaded = Arc::new(AtomicBool::new(false));
    let config_loaded_in_call = config_loaded.clone();
    let reveal_result = run_synchronous_entity_invocation(
        &active_agent,
        owner_metadata.clone(),
        &owner_id,
        &entity,
        reveal_activation,
        move |instance, store, principal| {
            Box::pin(async move {
                let parsed_agent_id = store
                    .data()
                    .parsed_agent_id()
                    .expect("entity context keeps the owner routing identity");
                initialize_entity(instance, store, &parsed_agent_id, principal.clone()).await?;
                config_loaded_in_call.store(true, Ordering::SeqCst);
                invoke_entity_method(
                    instance,
                    store,
                    &parsed_agent_id,
                    principal,
                    "echo_local_config",
                    data_value!().value().clone(),
                )
                .await
            })
        },
    )
    .await;
    let reveal_error =
        reveal_result.expect_err("an empty revealable-secret scope must reject secret reveal");
    assert!(
        config_loaded.load(Ordering::SeqCst),
        "readable secret handles must load successfully before reveal policy rejects them: {reveal_error}"
    );
    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn entity_agent_config_uses_owner_component_declarations(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    // Keep the declaring owner and the executable tool in separate components so consulting the
    // wrong component metadata cannot accidentally produce the expected value.
    let owner_component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .with_agent_config(
            "LocalConfigAgent",
            vec![
                golem_common::model::worker::AgentConfigEntryDto {
                    path: vec!["foo".to_string()],
                    value: serde_json::json!(7).into(),
                },
                golem_common::model::worker::AgentConfigEntryDto {
                    path: vec!["bar".to_string()],
                    value: serde_json::json!("owner-config").into(),
                },
                golem_common::model::worker::AgentConfigEntryDto {
                    path: vec!["nested".to_string(), "a".to_string()],
                    value: serde_json::json!(true).into(),
                },
                golem_common::model::worker::AgentConfigEntryDto {
                    path: vec!["nested".to_string(), "b".to_string()],
                    value: serde_json::json!([1, 2]).into(),
                },
            ],
        )
        .store()
        .await?;
    let entity_component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("LocalConfigAgent", "entity-owner-config");
    let worker_id = executor
        .start_agent(&owner_component.id, agent_id.clone())
        .await?;
    // Establish that the owner declaration and configured value are valid through the public DSL
    // before asking the separate entity Store to resolve that same owner-scoped value.
    executor
        .invoke_and_await_agent(
            &owner_component,
            &agent_id,
            "echo_local_config",
            data_value!(),
        )
        .await?;

    let owner_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let active_agent = executor
        .active_agent(&owner_id)
        .await
        .expect("owner must be active");
    let owner_metadata =
        owner_component_metadata(&active_agent, owner_component.id, owner_component.revision)
            .await?;
    let tool_name = ToolName::try_from("owner-config-reader").unwrap();
    let entity = AgentEntity::Tool(tool_name.clone());
    let activation = Arc::new(activation(
        ExecutableTarget::new(entity_component.id, entity_component.revision),
        "test:owner-config-reader",
        agent_id.agent_type.clone(),
        tool_name,
        context.account_id,
        FilesystemCapability::Incapable,
    ));
    // The executable deliberately has no LocalConfigAgent declaration. A successful lookup proves
    // the entity host resolves configuration through the owner component and owner agent state,
    // rather than accidentally consulting the executable component's metadata.
    let expected = encode_graph(&SchemaGraph::anonymous(SchemaType::s32()))?;
    run_synchronous_entity_invocation(
        &active_agent,
        owner_metadata,
        &owner_id,
        &entity,
        activation,
        move |_instance, store, _principal| {
            Box::pin(async move {
                AgentHost::get_config_value(
                    store.data_mut().durable_ctx_mut(),
                    vec!["foo".to_string()],
                    expected,
                )
                .await
                .map(|_| ())
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))
            })
        },
    )
    .await?;
    Ok(())
}
