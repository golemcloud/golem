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
use golem_common::model::agent_config::CanonicalAgentConfigPath;
use golem_common::model::agent_secret::{
    AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
};
use golem_common::model::card::{CardId, CardManagedByRuntimeDerived, StoredCard};
use golem_common::model::component::{
    AgentFilePath, AgentFilePermissions, ComponentId, ComponentName, ComponentRevision,
    InitialAgentFile,
};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::entity::{
    AgentEntity, EntityActivation, EntityActivationPolicy, EntityInvocationDescriptor,
    EntityInvocationId, EntityInvocationPlan, EntityInvocationPlanLayer,
    EntityInvocationPlanReference, EntityInvocationRequest, EntityInvocationScope,
    ExecutableTarget, FilesystemCapability, InvocationExecutionMode, OwnedAgentEntityId,
    ToolInvocationDescriptor, ToolMiddlewareName, ToolOutputContract,
};
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::payload::{
    HostRequest, HostRequestEntityInvocation, HostResponse, HostResponseEntityInvocation,
};
use golem_common::model::oplog::{DurableFunctionType, OplogEntry, OplogIndex, OplogPayload};
use golem_common::model::regions::{DeletedRegions, OplogRegion};
use golem_common::model::retry_policy::NamedRetryPolicy;
use golem_common::model::tool::{
    CompiledToolBinding, ConfigKeyScope, SecretKeyScope, ToolBindingOwner, ToolFilesystemAccess,
    ToolName, ToolProvisionConfig, ToolSource,
};
use golem_common::model::{AgentInvocation, AgentInvocationResult, IdempotencyKey, OwnedAgentId};
use golem_common::schema::schema_type::SchemaType;
use golem_common::schema::schema_value::SchemaValue;
use golem_common::schema::{SchemaGraph, SecretValuePayload};
use golem_common::{agent_id, data_value, widen_infallible};
use golem_schema::schema::wit::{SecretResolver, decode_value, decode_value_with, encode_graph};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::AgentDeploymentDetails;
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::replayable_stream::ReplayableStream;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::durable_host::DurableWorkerCtxView;
use golem_worker_executor::preview2::golem::agent::host::Host as AgentHost;
use golem_worker_executor::preview2::golem::secrets::reveal::Host as SecretRevealHost;
use golem_worker_executor::preview2::golem_api_1_x::host::Host as GolemApiHost;
use golem_worker_executor::services::HasComponentService;
use golem_worker_executor::services::active_agents::ActiveAgent;
use golem_worker_executor::services::card::{CardService, CardState};
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
    LastUniqueId, PrecompiledComponent, TestCardService, TestContext, TestExecutorOverrides,
    TestWorkerCtx, WorkerExecutorTestDependencies, registry_test_card, start, start_with_overrides,
};
use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
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
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
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

struct FailingCheckCardService {
    fail_checks: Arc<AtomicBool>,
}

#[async_trait]
impl CardService for FailingCheckCardService {
    async fn record_revoked_cards(&self, card_ids: &[CardId]) {
        TestCardService.record_revoked_cards(card_ids).await;
    }

    async fn create_runtime_card(
        &self,
        card: StoredCard,
        provenance: CardManagedByRuntimeDerived,
    ) -> Result<StoredCard, WorkerExecutorError> {
        TestCardService.create_runtime_card(card, provenance).await
    }

    async fn check_cards(
        &self,
        card_ids: Vec<CardId>,
    ) -> Result<HashMap<CardId, CardState>, WorkerExecutorError> {
        if self.fail_checks.load(Ordering::SeqCst) {
            Err(WorkerExecutorError::runtime(
                "synthetic card authority failure",
            ))
        } else {
            TestCardService.check_cards(card_ids).await
        }
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

pub(crate) async fn owner_component_metadata(
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

#[derive(Clone, Copy, Debug)]
enum WalletChange {
    Restriction,
    Grant,
    Transfer,
    SiblingScope,
}

#[test]
#[timeout("120s")]
async fn gol838_derivation_pairs_wallet_with_live_or_historical_namespace(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_common::model::card::{PolymorphicPermissionPattern, parse_polymorphic_permission};
    use golem_worker_executor::preview2::golem::permissions::{derive, types, wallet};
    use wasmtime::component::Resource;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .update_agent_provision_config("RpcCounter", |config| {
            let grants = &mut config.initial_permissions.lower_bound.positive;
            grants.retain(|grant| !matches!(grant, PolymorphicPermissionPattern::Config(_)));
            grants
                .push(parse_polymorphic_permission("config(?agent) @ * : read : target").unwrap());
        })
        .store()
        .await?;
    let agent = agent_id!("RpcCounter", "derivation-namespace");
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &agent, "get_value", data_value!())
        .await?;
    let owner_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let active = executor.active_agent(&owner_id).await.unwrap();
    let canonical = owner_component_metadata(&active, component.id, component.revision).await?;
    let mut pinned = (*canonical).clone();
    pinned.application_name = golem_common::model::application::ApplicationName("old-app".into());
    pinned.environment_name = golem_common::model::environment::EnvironmentName("old-env".into());
    assert_ne!(canonical.application_name, pinned.application_name);
    assert_ne!(canonical.environment_name, pinned.environment_name);
    let grant = |metadata: &golem_service_base::model::component::Component| types::PatternGrant {
        class: "config".into(),
        owner: format!(
            "{}/{}/{}/{}/{}",
            metadata.account_email,
            metadata.application_name,
            metadata.environment_name,
            metadata.component_name,
            worker_id.agent_id
        ),
        recipient: "*".into(),
        verb: "read".into(),
        resource_id: "target".into(),
    };
    let canonical_grant = grant(&canonical);
    let pinned_grant = grant(&pinned);
    let pinned = Arc::new(pinned);
    let entity = AgentEntity::ToolMiddleware(ToolMiddlewareName::try_from("namespace").unwrap());
    let activation = Arc::new(middleware_activation(
        ExecutableTarget::new(component.id, component.revision),
        ToolMiddlewareName::try_from("namespace").unwrap(),
    ));
    // Deliberately distinct dispatch metadata models a retained entity pin. These synthetic
    // scopes test namespace selection only; genuine reconstruction is covered separately.
    let wallet_cards = active.primary().get_wallet_cards().await?;
    for historical in [false, true] {
        let parent_start = active.execution().oplog().current_oplog_index().await;
        let parent_id = OwnerInvocationId::Agent(parent_start);
        let parent = active
            .execution()
            .lane()
            .enter_primary(parent_start)?
            .acquire()
            .await?;
        let principal = Principal::Agent(AgentPrincipal {
            agent_id: worker_id.clone(),
        });
        let scope = if historical {
            replay_invocation_scope(
                &owner_id,
                &entity,
                parent_start.next(),
                parent_start,
                activation.clone(),
                principal,
            )
        } else {
            invocation_scope(
                &owner_id,
                &entity,
                parent_start.next(),
                parent_start,
                activation.clone(),
                principal,
            )
        }
        .with_authority_wallet(wallet_cards.clone());
        stage_entity_boundary(&active, &scope).await?;
        let claim = if historical {
            finish_entity_boundary(&active, parent_start.next()).await?;
            active
                .execution()
                .commit(CommitLevel::Always)
                .await
                .unwrap();
            active
                .execution()
                .install_replay_generation(
                    DeletedRegions::from_regions([OplogRegion::from_index_range(
                        OplogIndex::INITIAL.next()..=parent_start,
                    )]),
                    None,
                )
                .await?;
            let claim = active
                .execution()
                .test_claim_entity_boundary(parent_start)
                .await?;
            assert_eq!(claim.start_idx(), parent_start.next());
            Some(claim)
        } else {
            None
        };
        let expected = if historical {
            pinned_grant.clone()
        } else {
            canonical_grant.clone()
        };
        let unexpected = if historical {
            canonical_grant.clone()
        } else {
            pinned_grant.clone()
        };
        let body = active.start_entity_invocation(
            parent_id.clone(),
            scope,
            pinned.clone(),
            EntityCallMode::Synchronous,
            move |_, store| {
                Box::pin(async move {
                    let ctx = store.data_mut().durable_ctx_mut();
                    let parents = wallet::Host::self_wallet(ctx).await?;
                    let borrowed = || {
                        parents
                            .iter()
                            .map(|p| Resource::new_borrow(p.rep()))
                            .collect()
                    };
                    let scope = derive::Host::derive_scope(
                        ctx,
                        borrowed(),
                        vec![expected.clone()],
                        vec![],
                        vec![],
                        vec![],
                    )
                    .await?;
                    assert!(
                        scope.is_ok(),
                        "expected namespace scope: historical={historical}, {scope:?}"
                    );
                    let wrong = derive::Host::derive_scope(
                        ctx,
                        borrowed(),
                        vec![unexpected.clone()],
                        vec![],
                        vec![],
                        vec![],
                    )
                    .await?;
                    assert!(
                        matches!(wrong, Err(types::PermissionError::LowerBoundTooBroad(_))),
                        "other namespace scope must be refused: historical={historical}, {wrong:?}"
                    );
                    // A wrong namespace must be refused before persistent creation, including replay.
                    let wrong = derive::Host::derive_from_wallet(
                        ctx,
                        vec![unexpected],
                        vec![],
                        vec![],
                        vec![],
                        None,
                    )
                    .await
                    .expect_err("other namespace wallet must refuse before persistent creation");
                    assert!(
                        matches!(
                            wrong.downcast_ref::<types::PermissionError>(),
                            Some(types::PermissionError::LowerBoundTooBroad(_))
                        ),
                        "other namespace wallet must be refused: historical={historical}, {wrong:?}"
                    );
                    if !historical {
                        let derived = derive::Host::derive_from_wallet(
                            ctx,
                            vec![expected],
                            vec![],
                            vec![],
                            vec![],
                            None,
                        )
                        .await?;
                        assert!(
                            derived.is_ok(),
                            "live canonical wallet derivation: {derived:?}"
                        );
                    }
                    Ok(())
                })
            },
            std::future::ready,
        )?;
        body.await_result(&parent_id).await?;
        if let Some(claim) = claim {
            active
                .execution()
                .test_settle_entity_boundary(claim)
                .await?;
        } else {
            finish_entity_boundary(&active, parent_start.next()).await?;
        }
        drop(parent);
    }
    Ok(())
}

/// Reads an undeclared optional key: success is exactly None, not a resolver error mistaken for
/// a denial. Both keys use the real config host authorization boundary.
async fn authority_config_read(
    ctx: &mut golem_worker_executor::durable_host::DurableWorkerCtx<TestWorkerCtx>,
    key: &str,
) -> Result<bool, WorkerExecutorError> {
    let expected = encode_graph(&SchemaGraph::anonymous(SchemaType::option(
        SchemaType::s32(),
    )))
    .map_err(anyhow::Error::from)?;
    match AgentHost::get_config_value(ctx, vec![key.to_string()], expected).await? {
        Ok(value) => {
            assert_eq!(
                decode_value(&value).unwrap(),
                SchemaValue::Option { inner: None }
            );
            Ok(true)
        }
        Err(
            golem_worker_executor::preview2::golem::agent::host::ConfigValueError::PermissionDenied,
        ) => Ok(false),
    }
}

async fn observed_wallet(store: &mut Store<TestWorkerCtx>) -> anyhow::Result<(Vec<CardId>, u64)> {
    use golem_worker_executor::preview2::golem::permissions::{types, wallet};
    let ctx = store.data_mut().durable_ctx_mut();
    let mut ids = Vec::new();
    for handle in wallet::Host::self_wallet(ctx).await? {
        let id =
            types::Host::id(ctx, wasmtime::component::Resource::new_borrow(handle.rep())).await?;
        ids.push(CardId(golem_schema::model::CardId::from(id).uuid));
        golem_schema::schema::wit::PermissionCardResolver::drop_permission_card_handle(ctx, handle);
    }
    Ok((ids, wallet::Host::self_version(ctx).await?.generation))
}

async fn cross_store_wallet_change(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    component: &PrecompiledComponent,
    change: WalletChange,
    cancel_consumer: bool,
) -> anyhow::Result<()> {
    use golem_common::model::card::{
        Card, PolymorphicPermissionPattern, ScopeCard, parse_permission_fields,
        parse_polymorphic_permission,
    };
    use golem_worker_executor::preview2::golem::permissions::{types, wallet};

    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            // Only delivery is stubbed. Source authorization, membership removal, durable intent,
            // confirmation, and the sibling's later authorization use the production paths.
            wrap_worker_proxy: Some(Arc::new(crate::api::accepting_card_transfer_proxy)),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .update_agent_provision_config("RpcCounter", |config| {
            let grants = &mut config.initial_permissions.lower_bound.positive;
            grants.retain(|grant| !matches!(grant, PolymorphicPermissionPattern::Config(_)));
            grants
                .push(parse_polymorphic_permission("config(?agent) @ * : read : control").unwrap());
            if matches!(
                change,
                WalletChange::Restriction | WalletChange::SiblingScope
            ) {
                grants.push(
                    parse_polymorphic_permission("config(?agent) @ * : read : target").unwrap(),
                );
            }
        })
        .store()
        .await?;
    let agent = agent_id!("RpcCounter", format!("cross-store-{change:?}"));
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &agent, "get_value", data_value!())
        .await?;
    let owner_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let active = executor.active_agent(&owner_id).await.unwrap();
    let metadata = owner_component_metadata(&active, component.id, component.revision).await?;
    let grant = parse_permission_fields("config", "*/*/*/*/*", "*", "read", "target").unwrap();
    let card = StoredCard::Concrete(Card {
        card_id: CardId::new(),
        parent_ids: Vec::new(),
        lower_positive: if matches!(change, WalletChange::Grant | WalletChange::Transfer) {
            vec![grant.clone()]
        } else {
            Vec::new()
        },
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: if matches!(change, WalletChange::Restriction) {
            vec![grant.clone()]
        } else {
            Vec::new()
        },
        created_at: chrono::Utc::now(),
        expires_at: None,
        system_card: false,
        managed_by: None,
    });
    TestCardService
        .create_runtime_card(
            card.clone(),
            CardManagedByRuntimeDerived {
                environment_id: context.default_environment_id,
                agent_id: worker_id.clone(),
                invocation_key: IdempotencyKey::fresh(),
                oplog_index: OplogIndex::INITIAL,
            },
        )
        .await?;
    if matches!(change, WalletChange::Transfer) {
        executor
            .queue_card_install(&worker_id, card.clone())
            .await?;
    }
    // Read the wallet before either entity starts. Never ask the primary to serve a wallet
    // query while an entity is executing.
    let wallet = active.primary().get_wallet_cards().await?;
    let root = CardId::new();
    let inherited = matches!(change, WalletChange::SiblingScope).then(|| ScopeCard {
        scope_card_id: CardId::new(),
        root_card_ids: vec![root],
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: vec![grant],
    });
    let parent_start = active
        .execution()
        .oplog()
        .current_oplog_index()
        .await
        .next();
    let parent_id = OwnerInvocationId::Agent(parent_start);
    let parent = active
        .execution()
        .lane()
        .enter_primary(parent_start)?
        .acquire()
        .await?;
    let principal = Principal::Agent(AgentPrincipal {
        agent_id: worker_id.clone(),
    });
    let entity =
        AgentEntity::ToolMiddleware(ToolMiddlewareName::try_from("wallet-observer").unwrap());
    let activation = Arc::new(middleware_activation(
        ExecutableTarget::new(component.id, component.revision),
        ToolMiddlewareName::try_from("wallet-observer").unwrap(),
    ));
    let observer_scope = invocation_scope(
        &owner_id,
        &entity,
        parent_start.next(),
        parent_start,
        activation.clone(),
        principal.clone(),
    )
    .with_authority_wallet(wallet.clone())
    .with_invocation_scope_card(inherited);
    let changed_card_id = card.card_id();
    let (primed, primed_rx) = tokio::sync::oneshot::channel();
    let (observe, observe_rx) = tokio::sync::oneshot::channel();
    let observer = active.start_entity_invocation(
        parent_id.clone(),
        observer_scope,
        metadata.clone(),
        EntityCallMode::Synchronous,
        move |_instance, store| {
            Box::pin(async move {
                let (before_ids, before_generation) = observed_wallet(store).await?;
                assert_eq!(
                    before_ids.contains(&changed_card_id),
                    matches!(change, WalletChange::Transfer)
                );
                let before =
                    authority_config_read(store.data_mut().durable_ctx_mut(), "target").await?;
                assert_eq!(
                    before,
                    matches!(change, WalletChange::Restriction | WalletChange::Transfer)
                );
                assert!(
                    authority_config_read(store.data_mut().durable_ctx_mut(), "control").await?,
                    "the independent grant must allow"
                );
                primed.send(()).unwrap();
                observe_rx.await.unwrap();
                // This is the same Store, after the other Store has consumed the change completely.
                let after =
                    authority_config_read(store.data_mut().durable_ctx_mut(), "target").await?;
                let control =
                    authority_config_read(store.data_mut().durable_ctx_mut(), "control").await?;
                let (after_ids, after_generation) = observed_wallet(store).await?;
                assert_eq!(
                    after_ids.contains(&changed_card_id),
                    !matches!(change, WalletChange::Transfer)
                );
                assert_eq!(after_generation, before_generation + 1);
                Ok((after, control))
            })
        },
        std::future::ready,
    )?;
    primed_rx.await?;
    let tracked_before = executor.active_agents().tracked_card_ids().await;
    if matches!(change, WalletChange::SiblingScope) {
        assert!(
            tracked_before.contains(&root),
            "the active observer's root must first be registered"
        );
    }
    if matches!(
        change,
        WalletChange::Restriction | WalletChange::Grant | WalletChange::SiblingScope
    ) {
        executor
            .queue_card_install(&worker_id, card.clone())
            .await?;
    }
    let consumer_scope = invocation_scope(
        &owner_id,
        &entity,
        parent_start.next().next(),
        parent_start,
        activation,
        principal,
    )
    .with_authority_wallet(wallet);
    let transfer_target = agent_id!("RpcCounter", "card-recipient").to_string();
    let card_id = card.card_id();
    let commit_gate = cancel_consumer.then(|| executor.gate_next_wallet_install_commit(&worker_id));
    let consumer = active.start_entity_invocation(
        parent_id.clone(),
        consumer_scope,
        metadata,
        EntityCallMode::Synchronous,
        move |_instance, store| {
            Box::pin(async move {
                let ctx = store.data_mut().durable_ctx_mut();
                if matches!(change, WalletChange::Transfer) {
                    let mut source = None;
                    for handle in wallet::Host::self_wallet(ctx).await? {
                        let id = types::Host::id(
                            ctx,
                            wasmtime::component::Resource::new_borrow(handle.rep()),
                        )
                        .await?;
                        if golem_schema::model::CardId::from(id).uuid == card_id.0 {
                            source = Some(handle);
                        }
                    }
                    let result = wallet::Host::install_card(
                        ctx,
                        source.expect("source is in the wallet"),
                        types::Holder::Agent(
                            golem_worker_executor::preview2::golem::agent::common::AgentId {
                                component_id: component.id.into(),
                                agent_id: transfer_target,
                            },
                        ),
                    )
                    .await?;
                    assert!(result.is_ok(), "source transfer must complete: {result:?}");
                } else {
                    wallet::Host::self_version(ctx).await?;
                }
                // A positive control in the consumer also proves it reached the new surface.
                assert!(
                    authority_config_read(store.data_mut().durable_ctx_mut(), "control").await?
                );
                let expected = matches!(change, WalletChange::Grant | WalletChange::SiblingScope);
                assert_eq!(
                    authority_config_read(store.data_mut().durable_ctx_mut(), "target").await?,
                    expected,
                    "the consumer must observe its own completed change"
                );
                Ok(())
            })
        },
        std::future::ready,
    )?;
    if let Some((committed, release)) = commit_gate {
        committed.await?;
        assert!(
            executor
                .active_agents()
                .tracked_card_ids()
                .await
                .contains(&card_id),
            "the committed candidate must remain routed before wallet publication"
        );
        consumer.abort_handle().abort();
        assert!(
            consumer.await_result(&parent_id).await.is_err(),
            "the producer must not return successfully"
        );
        assert!(
            executor
                .active_agents()
                .tracked_card_ids()
                .await
                .contains(&card_id),
            "producer cancellation must not drop the accepted transaction's candidate interest"
        );
        release.send(()).unwrap();
    } else {
        consumer.await_result(&parent_id).await?;
    }
    let oplog = active.execution().oplog();
    let tip = oplog.current_oplog_index().await;
    let entries = oplog.read_exact(OplogIndex::INITIAL, u64::from(tip)).await;
    if matches!(change, WalletChange::Transfer) {
        assert!(
            entries.values().any(|entry| matches!(entry,
            OplogEntry::CardTransferStarted { card_id: id, .. } if *id == card_id)),
            "source membership removal must have a durable transfer intent"
        );
        assert!(
            entries.values().any(|entry| matches!(entry,
            OplogEntry::CardTransferConfirmed { source_card_id, .. } if *source_card_id == card_id)),
            "the source transfer must be confirmed before the observer resumes"
        );
    } else {
        assert!(
            entries.values().any(|entry| matches!(entry,
            OplogEntry::CardInstalled { card, .. } if card.card_id() == card_id)),
            "the consumer must record installation before the observer resumes"
        );
    }
    let tracked_after = executor.active_agents().tracked_card_ids().await;
    if matches!(change, WalletChange::SiblingScope) {
        // Use the real owner-interest router, not a direct queue call that bypasses the bug.
        executor.active_agents().notify_revoked_cards(&[root]).await;
    }
    observe.send(()).unwrap();
    let (after, control) = observer.await_result(&parent_id).await?;
    drop(parent);
    assert!(
        control,
        "an unrelated config grant must survive every change"
    );
    if matches!(change, WalletChange::SiblingScope) {
        assert!(
            tracked_after.contains(&root),
            "a sibling refresh erased an active restrictive scope root; after routed revocation target allowed={after}"
        );
    }
    assert_eq!(
        after,
        matches!(change, WalletChange::Grant | WalletChange::SiblingScope),
        "the existing observer Store must authorize from the changed owner authority ({change:?})"
    );
    Ok(())
}

#[test]
#[timeout("120s")]
async fn gol838_upper_negative_install_reaches_existing_entity_store(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    cross_store_wallet_change(
        last_unique_id,
        deps,
        component,
        WalletChange::Restriction,
        false,
    )
    .await
}

#[test]
#[timeout("120s")]
async fn gol838_lower_grant_install_reaches_existing_entity_store(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    cross_store_wallet_change(last_unique_id, deps, component, WalletChange::Grant, false).await
}

#[test]
#[timeout("120s")]
async fn gol838_concrete_source_transfer_removes_existing_entity_grant(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    cross_store_wallet_change(
        last_unique_id,
        deps,
        component,
        WalletChange::Transfer,
        false,
    )
    .await
}

#[test]
#[timeout("120s")]
async fn gol838_sibling_refresh_preserves_active_scope_revocation_routing(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    cross_store_wallet_change(
        last_unique_id,
        deps,
        component,
        WalletChange::SiblingScope,
        false,
    )
    .await
}

#[test]
#[timeout("120s")]
async fn gol838_committed_grant_survives_producer_cancellation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    cross_store_wallet_change(last_unique_id, deps, component, WalletChange::Grant, true).await
}

#[test]
#[timeout("120s")]
async fn gol838_retained_replay_preparation_is_helped_or_fails_closed(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_service_base::error::worker_executor::InterruptKind;
    use golem_worker_executor::durable_host::TestReplayEvent;

    // The primary bootstrap context tests preparation ownership, not guest reconstruction.
    for (fail_fetch, refuse_admission) in [(false, false), (true, false), (false, true)] {
        let context = TestContext::new(last_unique_id);
        let outage = Arc::new(crate::hot_update::Outage::default());
        let wrapped_outage = outage.clone();
        let executor = start_with_overrides(
            deps,
            &context,
            TestExecutorOverrides {
                wrap_component_service: Some(Arc::new(move |inner| {
                    Arc::new(crate::hot_update::FlakyComponentService {
                        inner,
                        outage: wrapped_outage.clone(),
                    })
                })),
                ..Default::default()
            },
        )
        .await?;
        let component = executor
            .component_dep(&context.default_environment_id, fixture)
            .store()
            .await?;
        let agent = agent_id!("RpcCounter", "replay-preparation-help");
        let worker_id = golem_common::model::AgentId {
            component_id: component.id,
            agent_id: agent.to_string(),
        };
        let (entered, entered_rx) = tokio::sync::oneshot::channel();
        let (updated, updated_rx) = tokio::sync::oneshot::channel();
        let checked = executor.probe_next_primary_context(
            &worker_id,
            move |ctx| Box::pin(async move {
                ctx.process_pending_replay_events().await?;
                let _ = entered.send(());
                let (first_target, target): (golem_common::model::component::ComponentDto,
                    golem_common::model::component::ComponentDto) = updated_rx.await
                    .map_err(|_| WorkerExecutorError::runtime("test update sender dropped"))?;
                let agent_type = ctx.parsed_agent_id().unwrap().agent_type;
                let expected_changed = ctx.component_metadata()
                    .metadata.agent_type_initial_permission_card(&agent_type)
                    != target.metadata.agent_type_initial_permission_card(&agent_type);
                let (_, historical_before, canonical_before, _) = ctx.test_replay_application_state();
                let fetches_before = outage.metadata_fetches().len();
                if refuse_admission {
                    ctx.test_refuse_next_metadata_admission();
                    let result = ctx.test_stage_replay_applications(vec![
                        TestReplayEvent::UpdateReplayed { new_revision: first_target.revision },
                        TestReplayEvent::UpdateReplayed { new_revision: target.revision },
                    ]);
                    assert!(matches!(result, Err(WorkerExecutorError::Interrupted { kind: InterruptKind::Jump })));
                    let (applications, historical, canonical, current) = ctx.test_replay_application_state();
                    assert!(applications.is_empty(), "a refused application must not be published");
                    assert!(!current, "the transferred suffix requires reconstruction");
                    assert_eq!(historical, historical_before);
                    assert_eq!(canonical, canonical_before);
                    assert_eq!(outage.metadata_fetches().len(), fetches_before, "refusal must not poll either preparation");
                    return result;
                } else {
                    let (entered, release) = outage.gate_metadata(first_target.revision);
                    ctx.test_stage_replay_applications(vec![
                        TestReplayEvent::UpdateReplayed { new_revision: first_target.revision },
                        TestReplayEvent::UpdateReplayed { new_revision: target.revision },
                    ])?;
                    {
                        let helping = ctx.process_pending_replay_events();
                        tokio::pin!(helping);
                        tokio::select! {
                            result = &mut helping => panic!("preparation escaped the metadata gate: {result:?}"),
                            result = entered => result.unwrap(),
                        }
                    }
                    let (applications, historical, canonical, _) = ctx.test_replay_application_state();
                    assert_eq!(applications.len(), 2);
                    assert_eq!(historical, historical_before, "dropping the helper must retain the unapplied prefix");
                    assert_eq!(canonical, canonical_before);
                    assert_eq!(&outage.metadata_fetches()[fetches_before..], &[first_target.revision],
                        "the later preparation must not overtake the held predecessor");
                    if fail_fetch {
                        outage.begin(first_target.revision);
                    }
                    release.send(()).unwrap();
                    let decision = authority_config_read(ctx, "control").await;
                    if fail_fetch {
                        assert!(decision.is_err(), "a new config decision must not escape a failed prefix");
                        assert!(!ctx.test_replay_application_state().3);
                        assert!(!outage.metadata_fetches()[fetches_before..].contains(&target.revision),
                            "dependent preparation must not run past a failed predecessor");
                        return decision.map(|_| ());
                    } else {
                        assert!(decision?);
                        let (applications, historical, canonical, current) = ctx.test_replay_application_state();
                        assert!(applications.is_empty());
                        assert_eq!(historical, historical_before + u64::from(expected_changed));
                        assert_eq!(canonical, canonical_before, "historical application must not overwrite the live wallet");
                        assert!(current);
                        assert_eq!(ctx.component_metadata().revision, target.revision);
                        assert_eq!(&outage.metadata_fetches()[fetches_before..],
                            &[first_target.revision, target.revision]);
                    }
                }
                Ok(())
            }),
        );
        let startup_executor = executor.clone();
        let startup =
            tokio::spawn(async move { startup_executor.start_agent(&component.id, agent).await });
        tokio::time::timeout(Duration::from_secs(10), entered_rx).await??;
        let first_target = executor
            .update_component(&worker_id.component_id, &fixture.wasm_name)
            .await?;
        let target = executor
            .update_component(&worker_id.component_id, &fixture.wasm_name)
            .await?;
        updated
            .send((first_target, target))
            .map_err(|_| anyhow::anyhow!("primary probe dropped"))?;
        let result = tokio::time::timeout(Duration::from_secs(20), checked).await??;
        if fail_fetch || refuse_admission {
            assert!(
                result.is_err(),
                "poisoned bootstrap must propagate its failure"
            );
            executor.shutdown_and_wait_for_invocation_loops().await?;
            startup.abort();
            let _ = startup.await;
        } else {
            result?;
            tokio::time::timeout(Duration::from_secs(10), startup).await???;
        }
    }
    Ok(())
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
        InvokeResult::Succeeded { result, .. }
            if matches!(*result, AgentInvocationResult::AgentInitialization) =>
        {
            Ok(())
        }
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
        InvokeResult::Succeeded { result, .. }
            if matches!(*result, AgentInvocationResult::AgentMethod { .. })
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
        InvokeResult::Succeeded { result, .. } => Ok(*result),
        result => Err(
            golem_service_base::error::worker_executor::WorkerExecutorError::runtime(format!(
                "entity method did not succeed: {result:?}"
            )),
        ),
    }
}

pub(crate) fn activation(
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
        ConfigKeyScope::All,
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
        ConfigKeyScope::All,
        secret_keys_readable,
        secret_keys_revealable,
    )
}

fn activation_with_config_policy(
    executable: ExecutableTarget,
    component_name: &str,
    agent_type_name: AgentTypeName,
    tool_name: ToolName,
    account_id: golem_common::model::account::AccountId,
    config_keys_readable: ConfigKeyScope,
) -> EntityActivation {
    activation_with_policy(
        executable,
        component_name,
        agent_type_name,
        tool_name,
        account_id,
        FilesystemCapability::Incapable,
        ToolProvisionConfig::default(),
        config_keys_readable,
        SecretKeyScope::All,
        SecretKeyScope::All,
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
    config_keys_readable: ConfigKeyScope,
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
        release_id: None,
        owner: ToolBindingOwner::AgentType { agent_type_name },
        tool_name,
        version: "1.0.0".to_string(),
        metadata_version: "0.1.0".to_string(),
        metadata_digest: Default::default(),
        account_id,
        account_email: AccountEmail::new("test@golem"),
        parameters: NormalizedJsonValue::new(serde_json::json!({})),
        config_keys_readable,
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
            mcp_import: None,
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
pub(crate) async fn run_synchronous_entity_invocation<R, F>(
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
    let parent_start = active_agent.execution().oplog().current_oplog_index().await;
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
    )
    .with_authority_wallet(active_agent.primary().get_wallet_cards().await?);
    stage_entity_boundary(active_agent, &scope).await?;
    let body = active_agent.start_entity_invocation(
        parent_id.clone(),
        scope,
        owner_metadata,
        EntityCallMode::Synchronous,
        move |instance, store| invoke(instance, store, principal),
        std::future::ready,
    )?;

    let result = body.await_result(&parent_id).await;
    if result.is_ok() {
        finish_entity_boundary(active_agent, parent_start.next()).await?;
    }
    drop(parent);
    result
}

/// Records a boundary for a staged body, not an executor recovery or tool dispatch.
async fn stage_entity_boundary(
    active: &ActiveAgent<TestWorkerCtx>,
    scope: &EntityInvocationScope,
) -> Result<(), WorkerExecutorError> {
    use golem_common::schema::TypedSchemaValue;
    use golem_common::schema::tool::{CommandTree, Tool};
    let empty = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::tuple(vec![])),
        SchemaValue::Tuple { elements: vec![] },
    );
    let activation = scope.activation();
    let mut layers = Vec::new();
    let leaf = if matches!(
        activation.policy(),
        EntityActivationPolicy::ToolMiddleware { .. }
    ) {
        let leaf = activation_with_policy(
            activation.executable_opt().unwrap().clone(),
            "test:staged-leaf",
            AgentTypeName("StagedOwner".to_string()),
            ToolName::try_from("staged-leaf").unwrap(),
            golem_common::model::account::AccountId::new(),
            activation.filesystem(),
            ToolProvisionConfig::default(),
            ConfigKeyScope::All,
            SecretKeyScope::All,
            SecretKeyScope::All,
        );
        layers.push(EntityInvocationPlanLayer::Middleware {
            activation: activation.as_ref().clone(),
            parameters: empty.clone(),
            expected_definition: None,
            presented_definition: None,
            next_effective_definition: Tool {
                version: "1.0.0".to_string(),
                requires_filesystem: false,
                commands: CommandTree { nodes: vec![] },
                schema: SchemaGraph::empty(),
            },
            compatibility: None,
        });
        leaf
    } else {
        activation.as_ref().clone()
    };
    layers.push(EntityInvocationPlanLayer::Tool { activation: leaf });
    let metadata = EntityInvocationRequest {
        entity: scope.invocation_id().entity_id().entity.clone(),
        call_mode: EntityCallMode::Synchronous,
        operation: EntityInvocationDescriptor::Tool(ToolInvocationDescriptor {
            attempt_ordinal: 0,
            command_path: vec!["staged".to_string()],
            args: vec![],
            has_stdin: false,
            has_stdout: false,
            declares_stdout: false,
            has_stderr: false,
            declares_stderr: false,
            output_contract: ToolOutputContract {
                result: None,
                errors: vec![],
            },
        }),
        principal: scope.calling_principal().clone(),
        plan: EntityInvocationPlanReference::Root {
            plan: EntityInvocationPlan::new(layers).unwrap(),
        },
        assume_idempotence: scope.assume_idempotence(),
        authority_wallet: active.primary().get_wallet_cards().await?,
    };
    let index = active
        .execution()
        .oplog()
        .add(OplogEntry::Start {
            timestamp: golem_common::model::Timestamp::now_utc(),
            parent_start_index: Some(scope.parent_start_index()),
            function_name: HostFunctionName::GolemEntityInvoke,
            invocation_id: None,
            observational_owner: None,
            request: Some(OplogPayload::Inline(Box::new(
                HostRequest::EntityInvocation(HostRequestEntityInvocation {
                    metadata: desert_rust::serialize_to_byte_vec(&metadata).unwrap(),
                    input: empty,
                    stream_session_idempotency_key: scope.stream_session_idempotency_key().clone(),
                }),
            ))),
            durable_function_type: DurableFunctionType::WriteLocal,
            span_started: None,
        })
        .await?;
    assert_eq!(index, scope.invocation_id().start_index());
    Ok(())
}

async fn finish_entity_boundary(
    active: &ActiveAgent<TestWorkerCtx>,
    start_index: OplogIndex,
) -> Result<(), WorkerExecutorError> {
    active
        .execution()
        .oplog()
        .add(OplogEntry::End {
            timestamp: golem_common::model::Timestamp::now_utc(),
            start_index,
            response: Some(OplogPayload::Inline(Box::new(
                HostResponse::EntityInvocation(HostResponseEntityInvocation {
                    result: Ok(golem_common::schema::TypedSchemaValue::new(
                        SchemaGraph::anonymous(SchemaType::tuple(vec![])),
                        SchemaValue::Tuple { elements: vec![] },
                    )),
                }),
            ))),
            forced_commit: false,
            span_finished: None,
            span_attributes: None,
        })
        .await?;
    Ok(())
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
            config_keys_readable: Default::default(),
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
        IdempotencyKey::new("instance-layer-live-scope".to_string()),
        true,
        false,
        IdempotencyKey::new("instance-layer-live-streams".to_string()),
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
        IdempotencyKey::new("instance-layer-replay-scope".to_string()),
        true,
        false,
        IdempotencyKey::new("instance-layer-replay-streams".to_string()),
    )
    .unwrap()
}

async fn completed_entity_replay_rejects_positional_calls(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_rpc_rust: &PrecompiledComponent,
    record_noop: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let agent_id = agent_id!("RpcCounter", "completed-positional-tail");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "get_value", data_value!())
        .await?;

    let owner_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let active = executor.active_agent(&owner_id).await.unwrap();
    let metadata = owner_component_metadata(&active, component.id, component.revision).await?;
    let entity = AgentEntity::ToolMiddleware(
        ToolMiddlewareName::try_from("completed-positional-tail").unwrap(),
    );
    let principal = Principal::Agent(AgentPrincipal {
        agent_id: owner_id.agent_id.clone(),
    });
    let activation = Arc::new(middleware_activation(
        ExecutableTarget::new(component.id, component.revision),
        ToolMiddlewareName::try_from("completed-positional-tail").unwrap(),
    ));
    let parent_start = active.execution().oplog().current_oplog_index().await;
    let entity_start = parent_start.next();
    let parent_id = OwnerInvocationId::Agent(parent_start);
    let lane = active.execution().lane();
    let primary = lane.enter_primary(parent_start)?.acquire().await?;
    let live_scope = invocation_scope(
        &owner_id,
        &entity,
        entity_start,
        parent_start,
        activation.clone(),
        principal.clone(),
    );
    stage_entity_boundary(&active, &live_scope).await?;
    active
        .start_entity_invocation(
            parent_id.clone(),
            live_scope,
            metadata.clone(),
            EntityCallMode::Synchronous,
            move |_instance, store| {
                Box::pin(async move {
                    if record_noop {
                        GolemApiHost::get_oplog_index(store.data_mut().durable_ctx_mut()).await?;
                    }
                    Ok(())
                })
            },
            std::future::ready,
        )?
        .await_result(&parent_id)
        .await?;
    finish_entity_boundary(&active, entity_start).await?;
    drop(primary);
    active
        .execution()
        .commit(CommitLevel::Always)
        .await
        .unwrap();

    active
        .execution()
        .install_replay_generation(
            DeletedRegions::from_regions([OplogRegion::from_index_range(
                OplogIndex::INITIAL.next()..=parent_start,
            )]),
            None,
        )
        .await?;
    let replay_primary = lane.enter_primary(parent_start)?.acquire().await?;
    let claim = active
        .execution()
        .test_claim_entity_boundary(parent_start)
        .await?;
    let before = active.execution().oplog().current_oplog_index().await;
    let replay = active.start_entity_invocation(
        parent_id.clone(),
        replay_invocation_scope(
            &owner_id,
            &entity,
            entity_start,
            parent_start,
            activation,
            principal,
        ),
        metadata,
        EntityCallMode::Synchronous,
        move |_instance, store| {
            Box::pin(async move {
                let ctx = store.data_mut().durable_ctx_mut();
                let (first, second) = if record_noop {
                    (
                        GolemApiHost::mark_begin_operation(ctx)
                            .await
                            .expect_err("a mismatched positional call must fail"),
                        GolemApiHost::get_oplog_index(ctx)
                            .await
                            .expect_err("a failed mismatch must not make the entity live"),
                    )
                } else {
                    (
                        GolemApiHost::get_oplog_index(ctx)
                            .await
                            .expect_err("a completed entity must reject a new positional call"),
                        GolemApiHost::mark_begin_operation(ctx)
                            .await
                            .expect_err("a rejected positional call must not make the entity live"),
                    )
                };
                let first = first.to_string();
                let second = second.to_string();
                assert!(first.contains("NoOp"));
                assert!(second.contains("NoOp") || second.contains("BeginAtomicRegion"));
                if record_noop {
                    assert!(first.contains("BeginAtomicRegion"));
                } else {
                    assert!(second.contains("BeginAtomicRegion"));
                }
                Ok(())
            })
        },
        std::future::ready,
    )?;
    replay.await_result(&parent_id).await?;
    assert_eq!(
        before,
        active.execution().oplog().current_oplog_index().await
    );
    active
        .execution()
        .test_settle_entity_boundary(claim)
        .await?;
    drop(replay_primary);
    Ok(())
}

#[test]
#[timeout("120s")]
async fn completed_entity_replay_rejects_positional_calls_at_its_terminal(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
) -> anyhow::Result<()> {
    completed_entity_replay_rejects_positional_calls(last_unique_id, deps, agent_rpc_rust, false)
        .await
}

#[test]
#[timeout("120s")]
async fn completed_entity_replay_rejects_a_wrong_positional_entry(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
) -> anyhow::Result<()> {
    completed_entity_replay_rejects_positional_calls(last_unique_id, deps, agent_rpc_rust, true)
        .await
}

#[test]
#[timeout("120s")]
async fn incomplete_tool_config_tail_reauthorizes_without_rejecting_recorded_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let secret_id = AgentSecretId::new();
    let secret_path = CanonicalAgentSecretPath(vec!["walletSecret".to_string()]);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(Arc::new(FixedSecretsEnvironmentStateService {
                secrets: HashMap::from([(
                    secret_path.clone(),
                    AgentSecret {
                        id: secret_id,
                        environment_id: context.default_environment_id,
                        path: secret_path.clone(),
                        revision: AgentSecretRevision::INITIAL,
                        secret_type: SchemaGraph::anonymous(SchemaType::string()),
                        secret_value: Some(SchemaValue::String("not-observed".to_string())),
                    },
                )]),
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .update_agent_provision_config("RpcCounter", |config| {
            config.initial_permissions.lower_bound.negative.extend([
                golem_common::model::card::parse_polymorphic_permission(
                    "config(?agent) @ * : read : foo",
                )
                .unwrap(),
                golem_common::model::card::parse_polymorphic_permission(
                    "secret(?env) @ * : reveal : walletSecret",
                )
                .unwrap(),
            ]);
        })
        .store()
        .await?;
    let agent_id = agent_id!("RpcCounter", "config-tail-authorization");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "get_value", data_value!())
        .await?;

    let owner_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let active_agent = executor.active_agent(&owner_id).await.unwrap();
    let owner_metadata =
        owner_component_metadata(&active_agent, component.id, component.revision).await?;
    let middleware_name = ToolMiddlewareName::try_from("config-tail-authorization").unwrap();
    let entity = AgentEntity::ToolMiddleware(middleware_name.clone());
    let principal = Principal::Agent(AgentPrincipal {
        agent_id: owner_id.agent_id.clone(),
    });
    let activation = Arc::new(middleware_activation(
        ExecutableTarget::new(component.id, component.revision),
        middleware_name,
    ));
    let expected = encode_graph(&SchemaGraph::anonymous(SchemaType::option(
        SchemaType::s32(),
    )))?;
    let expected_plaintext = encode_graph(&SchemaGraph::anonymous(SchemaType::string()))?;
    let secret_snapshot = SecretValuePayload {
        secret_id: secret_id.0,
        config_key: Some(vec!["walletSecret".to_string()]),
        version: AgentSecretRevision::INITIAL.get(),
        resolved_at: chrono::Utc::now(),
        category: None,
    };
    let lane = active_agent.execution().lane();
    let parent_start = active_agent.execution().oplog().current_oplog_index().await;
    let entity_start = parent_start.next();
    let parent_id = OwnerInvocationId::Agent(parent_start);

    let primary = lane.enter_primary(parent_start)?.acquire().await?;
    stage_entity_boundary(
        &active_agent,
        &invocation_scope(
            &owner_id,
            &entity,
            entity_start,
            parent_start,
            activation.clone(),
            principal.clone(),
        ),
    )
    .await?;
    let live = active_agent.start_entity_invocation(
        parent_id.clone(),
        invocation_scope(
            &owner_id,
            &entity,
            entity_start,
            parent_start,
            activation.clone(),
            principal.clone(),
        ),
        owner_metadata.clone(),
        EntityCallMode::Synchronous,
        {
            let expected = expected.clone();
            let expected_plaintext = expected_plaintext.clone();
            let secret_snapshot = secret_snapshot.clone();
            move |_instance, store| {
                Box::pin(async move {
                    let ctx = store.data_mut().durable_ctx_mut();
                    let result = AgentHost::get_config_value(
                        ctx,
                        vec!["foo".to_string()],
                        expected,
                    )
                    .await?;
                    assert!(
                        matches!(
                            result,
                            Err(golem_worker_executor::preview2::golem::agent::host::ConfigValueError::PermissionDenied)
                        ),
                        "the wallet-negative config read must be durably denied"
                    );
                    let handle = ctx.secret_handle_from_snapshot(&secret_snapshot)?;
                    let reveal = SecretRevealHost::reveal(ctx, handle, expected_plaintext).await?;
                    assert!(
                        reveal.is_err(),
                        "the wallet-negative secret reveal must be durably denied"
                    );
                    GolemApiHost::get_oplog_index(ctx).await?;
                    Ok(())
                })
            }
        },
        std::future::ready,
    )?;
    live.await_result(&parent_id).await?;
    finish_entity_boundary(&active_agent, entity_start).await?;
    drop(primary);
    active_agent
        .execution()
        .commit(CommitLevel::Always)
        .await
        .unwrap();

    let live_tip = active_agent.execution().oplog().current_oplog_index().await;
    let live_entries = active_agent
        .execution()
        .oplog()
        .read_exact(
            parent_start.next(),
            u64::from(live_tip) - u64::from(parent_start),
        )
        .await;
    let config_start = live_entries
        .iter()
        .find_map(|(index, entry)| match entry {
            OplogEntry::Start { function_name, .. }
                if *function_name == HostFunctionName::GolemAgentGetConfigValue =>
            {
                Some(*index)
            }
            _ => None,
        })
        .expect("live config denial must have a Start");
    let config_end = live_entries
        .iter()
        .find_map(|(index, entry)| match entry {
            OplogEntry::End { start_index, .. } if *start_index == config_start => Some(*index),
            _ => None,
        })
        .expect("live config denial must have an End");
    let config_repair_expected = expected.clone();

    active_agent
        .execution()
        .install_replay_generation(
            DeletedRegions::from_regions([
                OplogRegion::from_index_range(OplogIndex::INITIAL.next()..=parent_start),
                OplogRegion::from_index_range(config_end..=live_tip),
            ]),
            None,
        )
        .await?;

    let replay_primary = lane.enter_primary(parent_start)?.acquire().await?;
    let claim = active_agent
        .execution()
        .test_claim_entity_boundary(parent_start)
        .await?;
    assert_eq!(claim.start_idx(), entity_start);
    let replay_scope = EntityInvocationScope::new(
        EntityInvocationId::new(
            OwnedAgentEntityId {
                owner: owner_id.clone(),
                entity: entity.clone(),
            },
            entity_start,
        )
        .unwrap(),
        parent_start,
        activation.clone(),
        principal.clone(),
        InvocationExecutionMode::ReplayingIncomplete,
        IdempotencyKey::new("instance-layer-incomplete-replay-scope".to_string()),
        true,
        false,
        IdempotencyKey::new("instance-layer-incomplete-replay-streams".to_string()),
    )
    .unwrap();
    let replay_parent_id = parent_id.clone();
    let replay = active_agent.start_entity_invocation(
        parent_id.clone(),
        replay_scope,
        owner_metadata.clone(),
        EntityCallMode::Synchronous,
        move |_instance, store| {
            Box::pin(async move {
                let ctx = store.data_mut().durable_ctx_mut();
                ctx.test_install_entity_tool_operation(
                    replay_parent_id,
                    EntityCallMode::Synchronous,
                )?;
                let recorded = AgentHost::get_config_value(
                    ctx,
                    vec!["foo".to_string()],
                    config_repair_expected,
                )
                .await?;
                GolemApiHost::get_oplog_index(ctx).await?;
                Ok(recorded)
            })
        },
        std::future::ready,
    )?;
    let recorded = replay.await_result(&parent_id).await?;
    active_agent
        .execution()
        .test_settle_entity_boundary(claim)
        .await?;
    drop(replay_primary);
    assert!(
        matches!(
            recorded,
            Err(golem_worker_executor::preview2::golem::agent::host::ConfigValueError::PermissionDenied)
        ),
        "a Start-only repair must rerun authorization instead of defaulting to allowed: {recorded:?}"
    );

    Ok(())
}

#[test]
#[timeout("120s")]
async fn incomplete_tool_reveal_tail_reauthorizes_wallet(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
) -> anyhow::Result<()> {
    run_incomplete_tool_reveal_tail_reauthorization(last_unique_id, deps, agent_rpc_rust, false)
        .await
}

#[test]
#[timeout("120s")]
async fn incomplete_tool_reveal_authority_error_preserves_durable_call_trap(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
) -> anyhow::Result<()> {
    run_incomplete_tool_reveal_tail_reauthorization(last_unique_id, deps, agent_rpc_rust, true)
        .await
}

async fn run_incomplete_tool_reveal_tail_reauthorization(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_rpc_rust: &PrecompiledComponent,
    fail_authority_check: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let secret_id = AgentSecretId::new();
    let secret_path = CanonicalAgentSecretPath(vec!["walletSecret".to_string()]);
    let fail_checks = Arc::new(AtomicBool::new(false));
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            create_card_service: Some(Arc::new({
                let fail_checks = fail_checks.clone();
                move || {
                    Arc::new(FailingCheckCardService {
                        fail_checks: fail_checks.clone(),
                    })
                }
            })),
            environment_state_service: Some(Arc::new(FixedSecretsEnvironmentStateService {
                secrets: HashMap::from([(
                    secret_path.clone(),
                    AgentSecret {
                        id: secret_id,
                        environment_id: context.default_environment_id,
                        path: secret_path,
                        revision: AgentSecretRevision::INITIAL,
                        secret_type: SchemaGraph::anonymous(SchemaType::string()),
                        secret_value: Some(SchemaValue::String("not-observed".to_string())),
                    },
                )]),
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .update_agent_provision_config("RpcCounter", |config| {
            config.initial_permissions.lower_bound.negative.push(
                golem_common::model::card::parse_polymorphic_permission(
                    "secret(?env) @ * : reveal : walletSecret",
                )
                .unwrap(),
            );
        })
        .store()
        .await?;
    let agent_id = agent_id!("RpcCounter", "reveal-tail-authorization");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "get_value", data_value!())
        .await?;

    let owner_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let active_agent = executor.active_agent(&owner_id).await.unwrap();
    let owner_metadata =
        owner_component_metadata(&active_agent, component.id, component.revision).await?;
    let middleware_name = ToolMiddlewareName::try_from("reveal-tail-authorization").unwrap();
    let entity = AgentEntity::ToolMiddleware(middleware_name.clone());
    let principal = Principal::Agent(AgentPrincipal {
        agent_id: owner_id.agent_id.clone(),
    });
    let activation = Arc::new(middleware_activation(
        ExecutableTarget::new(component.id, component.revision),
        middleware_name,
    ));
    let expected = encode_graph(&SchemaGraph::anonymous(SchemaType::string()))?;
    let snapshot = SecretValuePayload {
        secret_id: secret_id.0,
        config_key: Some(vec!["walletSecret".to_string()]),
        version: AgentSecretRevision::INITIAL.get(),
        resolved_at: chrono::Utc::now(),
        category: None,
    };
    let lane = active_agent.execution().lane();
    let parent_start = active_agent.execution().oplog().current_oplog_index().await;
    let entity_start = parent_start.next();
    let parent_id = OwnerInvocationId::Agent(parent_start);

    let primary = lane.enter_primary(parent_start)?.acquire().await?;
    stage_entity_boundary(
        &active_agent,
        &invocation_scope(
            &owner_id,
            &entity,
            entity_start,
            parent_start,
            activation.clone(),
            principal.clone(),
        ),
    )
    .await?;
    let live = active_agent.start_entity_invocation(
        parent_id.clone(),
        invocation_scope(
            &owner_id,
            &entity,
            entity_start,
            parent_start,
            activation.clone(),
            principal.clone(),
        ),
        owner_metadata.clone(),
        EntityCallMode::Synchronous,
        {
            let expected = expected.clone();
            let snapshot = snapshot.clone();
            move |_instance, store| {
                Box::pin(async move {
                    let ctx = store.data_mut().durable_ctx_mut();
                    let handle = ctx.secret_handle_from_snapshot(&snapshot)?;
                    let denied = SecretRevealHost::reveal(ctx, handle, expected).await?;
                    assert!(denied.is_err(), "wallet-negative reveal must be denied");
                    GolemApiHost::get_oplog_index(ctx).await?;
                    Ok(())
                })
            }
        },
        std::future::ready,
    )?;
    live.await_result(&parent_id).await?;
    finish_entity_boundary(&active_agent, entity_start).await?;
    drop(primary);
    active_agent
        .execution()
        .commit(CommitLevel::Always)
        .await
        .unwrap();
    let live_tip = active_agent.execution().oplog().current_oplog_index().await;
    let entries = active_agent
        .execution()
        .oplog()
        .read_exact(
            parent_start.next(),
            u64::from(live_tip) - u64::from(parent_start),
        )
        .await;
    let reveal_start = entries
        .iter()
        .find_map(|(index, entry)| match entry {
            OplogEntry::Start { function_name, .. }
                if *function_name == HostFunctionName::GolemSecretsReveal =>
            {
                Some(*index)
            }
            _ => None,
        })
        .expect("live reveal denial must have a Start");
    let reveal_end = entries
        .iter()
        .find_map(|(index, entry)| match entry {
            OplogEntry::End { start_index, .. } if *start_index == reveal_start => Some(*index),
            _ => None,
        })
        .expect("live reveal denial must have an End");

    active_agent
        .execution()
        .install_replay_generation(
            DeletedRegions::from_regions([
                OplogRegion::from_index_range(OplogIndex::INITIAL.next()..=parent_start),
                OplogRegion::from_index_range(reveal_end..=live_tip),
            ]),
            None,
        )
        .await?;
    if fail_authority_check {
        executor
            .queue_card_install(&worker_id, registry_test_card())
            .await?;
    }
    let replay_primary = lane.enter_primary(parent_start)?.acquire().await?;
    let claim = active_agent
        .execution()
        .test_claim_entity_boundary(parent_start)
        .await?;
    assert_eq!(claim.start_idx(), entity_start);
    let replay_parent_id = parent_id.clone();
    let replay = active_agent.start_entity_invocation(
        parent_id.clone(),
        EntityInvocationScope::new(
            EntityInvocationId::new(
                OwnedAgentEntityId {
                    owner: owner_id,
                    entity,
                },
                entity_start,
            )
            .unwrap(),
            parent_start,
            activation,
            principal,
            InvocationExecutionMode::ReplayingIncomplete,
            IdempotencyKey::new("instance-layer-incomplete-wallet-reveal".to_string()),
            true,
            false,
            IdempotencyKey::new("instance-layer-incomplete-wallet-reveal-streams".to_string()),
        )
        .unwrap(),
        owner_metadata,
        EntityCallMode::Synchronous,
        {
            let fail_checks = fail_checks.clone();
            move |_instance, store| {
                Box::pin(async move {
                    let ctx = store.data_mut().durable_ctx_mut();
                    ctx.test_install_entity_tool_operation(
                        replay_parent_id,
                        EntityCallMode::Synchronous,
                    )?;
                    let handle = ctx.secret_handle_from_snapshot(&snapshot)?;
                    fail_checks.store(fail_authority_check, Ordering::SeqCst);
                    let repaired = SecretRevealHost::reveal(ctx, handle, expected).await?;
                    GolemApiHost::get_oplog_index(ctx).await?;
                    Ok(repaired)
                })
            }
        },
        std::future::ready,
    )?;
    let replay_result = replay.await_result(&parent_id).await;
    active_agent
        .execution()
        .test_settle_entity_boundary(claim)
        .await?;
    drop(replay_primary);
    if fail_authority_check {
        let error = replay_result.expect_err("authority synchronization failure must escape");
        let error = format!("{error:?}");
        assert!(
            error.contains("synthetic card authority failure"),
            "the authority failure must remain the recoverable error: {error}"
        );
        assert!(
            !error.contains("unfinished NotCancellable durable call"),
            "the live durable-call handle must be abandoned through its trap boundary: {error}"
        );
    } else {
        let repaired = replay_result?;
        assert!(
            repaired.is_err(),
            "a Start-only reveal repair must rerun current wallet authorization"
        );
    }
    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn gol838_retained_entity_roots_survive_body_cleanup_and_detached_settlement(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::card::ScopeCard;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent = agent_id!("Clock", "retained-scope-interest");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &agent, "healthcheck", data_value!())
        .await?;
    let owner = OwnedAgentId::new(context.default_environment_id, &worker);
    let active = executor.active_agent(&owner).await.unwrap();
    let metadata = owner_component_metadata(&active, component.id, component.revision).await?;
    let tool = ToolName::try_from("retained-interest").unwrap();
    let entity = AgentEntity::Tool(tool.clone());
    let activation = Arc::new(activation(
        ExecutableTarget::new(component.id, component.revision),
        "test:retained-interest",
        agent.agent_type.clone(),
        tool,
        context.account_id,
        FilesystemCapability::Incapable,
    ));
    let start = active
        .execution()
        .oplog()
        .current_oplog_index()
        .await
        .next();
    let parent_id = OwnerInvocationId::Agent(start);
    let parent = active
        .execution()
        .lane()
        .enter_primary(start)?
        .acquire()
        .await?;
    let shared = CardId::new();
    let first_root = CardId::new();
    let second_root = CardId::new();
    let mut bodies = Vec::new();
    for (offset, root) in [first_root, second_root].into_iter().enumerate() {
        let scope = invocation_scope(
            &owner,
            &entity,
            OplogIndex::from_u64(u64::from(start) + offset as u64 + 1),
            start,
            activation.clone(),
            Principal::Agent(AgentPrincipal {
                agent_id: worker.clone(),
            }),
        )
        .with_invocation_scope_card(Some(ScopeCard {
            scope_card_id: CardId::new(),
            root_card_ids: vec![shared, root],
            lower_positive: Vec::new(),
            lower_negative: Vec::new(),
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
        }));
        let (cleaned, cleanup) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let handle = active.start_entity_invocation(
            parent_id.clone(),
            scope,
            metadata.clone(),
            EntityCallMode::Synchronous,
            move |_, store| {
                Box::pin(async move {
                    assert!(store.data().entity_invocation_scope().is_some());
                    Ok(())
                })
            },
            move |result| async move {
                // The runner has cleared execution scope before it invokes this finalizer,
                // but still owns the same retained Store.
                cleaned.send(()).unwrap();
                released.await.unwrap();
                result
            },
        )?;
        cleanup.await?;
        bodies.push((handle, release));
    }
    let tracked = executor.active_agents().tracked_card_ids().await;
    for root in [shared, first_root, second_root] {
        assert!(tracked.contains(&root));
    }
    let (second, release_second) = bodies.pop().unwrap();
    let (first, release_first) = bodies.pop().unwrap();
    drop(first); // Detach the caller; the retained Store must still keep its roots.
    assert!(
        executor
            .active_agents()
            .tracked_card_ids()
            .await
            .contains(&first_root)
    );
    release_first.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while executor
            .active_agents()
            .tracked_card_ids()
            .await
            .contains(&first_root)
        {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let tracked = executor.active_agents().tracked_card_ids().await;
    assert!(
        tracked.contains(&shared),
        "first settlement must not erase the sibling's shared root"
    );
    assert!(tracked.contains(&second_root));
    release_second.send(()).unwrap();
    second.await_result(&parent_id).await?;
    drop(parent);
    let tracked = executor.active_agents().tracked_card_ids().await;
    for root in [shared, first_root, second_root] {
        assert!(!tracked.contains(&root));
    }
    Ok(())
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
        IdempotencyKey::new("owner-entity-scope".to_string()),
        true,
        false,
        IdempotencyKey::new("owner-entity-streams".to_string()),
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
        IdempotencyKey::new("second-entity-scope".to_string()),
        true,
        false,
        IdempotencyKey::new("second-entity-streams".to_string()),
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
        IdempotencyKey::new("cancelled-entity-scope".to_string()),
        true,
        false,
        IdempotencyKey::new("cancelled-entity-streams".to_string()),
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
        IdempotencyKey::new("panicking-entity-scope".to_string()),
        true,
        false,
        IdempotencyKey::new("panicking-entity-streams".to_string()),
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
            && slot.invocations[0].executable == tool_activation.executable_opt().cloned()
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

    let restart = tokio::spawn({
        let worker = active_agent.primary();
        async move {
            worker
                .set_interrupting(
                    golem_service_base::error::worker_executor::InterruptKind::Restart,
                )
                .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while active_agent.entity_metadata().accepting_entities {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("owner restart did not fence the nested execution unit");
    restart.await??;
    assert!(
        release_nested.send(()).is_err(),
        "nested tool was still waiting after owner restart establishment"
    );
    assert!(
        active_agent
            .entity_metadata()
            .slots
            .iter()
            .all(|slot| slot.invocations.is_empty()),
        "owner restart did not retire every nested entity body"
    );
    assert!(
        middleware.await_result(&root_id).await.is_err(),
        "nested middleware execution survived its owner generation"
    );
    drop(root);
    executor
        .invoke_and_await_agent(&component, &agent_id, "healthcheck", data_value!())
        .await?;
    assert!(
        active_agent
            .entity_metadata()
            .slots
            .iter()
            .all(|slot| slot.invocations.is_empty()),
        "completed Stores leave only vacant in-memory slots, never durable child status"
    );
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
    )
    .with_authority_wallet(active_agent.primary().get_wallet_cards().await?);
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
    let authority_wallet = active_agent.primary().get_wallet_cards().await?;
    let live_scope = invocation_scope(
        &owner_id,
        &entity,
        entity_start,
        parent_start,
        activation.clone(),
        principal.clone(),
    )
    .with_authority_wallet(authority_wallet.clone());
    stage_entity_boundary(&active_agent, &live_scope).await?;
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
    finish_entity_boundary(&active_agent, entity_start).await?;
    drop(primary);
    active_agent
        .execution()
        .commit(CommitLevel::Always)
        .await
        .unwrap();

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
    let claim = active_agent
        .execution()
        .test_claim_entity_boundary(parent_start)
        .await?;
    assert_eq!(claim.start_idx(), entity_start);
    let replay_scope = replay_invocation_scope(
        &owner_id,
        &entity,
        entity_start,
        parent_start,
        activation,
        principal.clone(),
    )
    .with_authority_wallet(authority_wallet);
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
    active_agent
        .execution()
        .test_settle_entity_boundary(claim)
        .await?;
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
    let authority_wallet = active_agent.primary().get_wallet_cards().await?;

    let first_scope = invocation_scope(
        &owner_id,
        &entity,
        root_start.next(),
        root_start,
        activation.clone(),
        principal.clone(),
    )
    .with_authority_wallet(authority_wallet.clone());
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
    )
    .with_authority_wallet(authority_wallet.clone());
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
    )
    .with_authority_wallet(authority_wallet);
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
    let expected_secret = encode_graph(&SchemaGraph::anonymous(SchemaType::secret(
        Default::default(),
    )))?;
    let replay_expected_secret = expected_secret.clone();
    let before_denial = active_agent.execution().oplog().current_oplog_index().await;
    run_synchronous_entity_invocation(
        &active_agent,
        owner_metadata.clone(),
        &owner_id,
        &entity,
        activation.clone(),
        move |_instance, store, _principal| {
            Box::pin(async move {
                let ctx = store.data_mut().durable_ctx_mut();
                let denied = AgentHost::get_config_value(
                    ctx,
                    vec!["secret".to_string()],
                    expected_secret,
                )
                .await?;
                assert!(matches!(
                    denied,
                    Err(golem_worker_executor::preview2::golem::agent::host::ConfigValueError::PermissionDenied)
                ));
                GolemApiHost::get_oplog_index(ctx).await?;
                Ok(())
            })
        },
    )
    .await?;
    let after_denial = active_agent.execution().oplog().current_oplog_index().await;
    let denial_entries = active_agent
        .execution()
        .oplog()
        .read_exact(
            before_denial.next(),
            u64::from(after_denial) - u64::from(before_denial),
        )
        .await;
    let config_start = denial_entries
        .iter()
        .find_map(|(index, entry)| match entry {
            OplogEntry::Start { function_name, .. }
                if *function_name == HostFunctionName::GolemAgentGetConfigValue =>
            {
                Some(*index)
            }
            _ => None,
        })
        .expect("an unreadable secret config denial must have a durable Start");
    assert!(
        denial_entries.values().any(
            |entry| matches!(entry, OplogEntry::End { start_index, .. } if *start_index == config_start)
        ),
        "an unreadable secret config denial must be a completed durable call"
    );
    let config_entity_start = denial_entries
        .values()
        .find_map(|entry| match entry {
            OplogEntry::Start {
                function_name,
                parent_start_index,
                ..
            } if *function_name == HostFunctionName::GolemAgentGetConfigValue => {
                *parent_start_index
            }
            _ => None,
        })
        .expect("the config denial must belong to the entity invocation");

    // A completed denial remains authoritative even when the occurrence now permits the key.
    // Completed replay must not consult current occurrence or wallet authority.
    let permissive_activation = Arc::new(activation_with_secret_policy(
        ExecutableTarget::new(component.id, component.revision),
        "test:restricted-secret-tool",
        agent_id.agent_type.clone(),
        tool_name.clone(),
        context.account_id,
        SecretKeyScope::All,
        SecretKeyScope::All,
    ));
    active_agent
        .execution()
        .install_replay_generation(
            DeletedRegions::from_regions([OplogRegion::from_index_range(
                OplogIndex::INITIAL.next()..=before_denial,
            )]),
            None,
        )
        .await?;
    let config_parent_id = OwnerInvocationId::Agent(before_denial);
    let lane = active_agent.execution().lane();
    let replay_primary = lane.enter_primary(before_denial)?.acquire().await?;
    let claim = active_agent
        .execution()
        .test_claim_entity_boundary(before_denial)
        .await?;
    assert_eq!(claim.start_idx(), config_entity_start);
    let replay = active_agent.start_entity_invocation(
        config_parent_id.clone(),
        replay_invocation_scope(
            &owner_id,
            &entity,
            config_entity_start,
            before_denial,
            permissive_activation,
            Principal::Agent(AgentPrincipal {
                agent_id: owner_id.agent_id.clone(),
            }),
        ),
        owner_metadata.clone(),
        EntityCallMode::Synchronous,
        move |_instance, store| {
            Box::pin(async move {
                let ctx = store.data_mut().durable_ctx_mut();
                let recorded = AgentHost::get_config_value(
                    ctx,
                    vec!["secret".to_string()],
                    replay_expected_secret,
                )
                .await?;
                GolemApiHost::get_oplog_index(ctx).await?;
                Ok(recorded)
            })
        },
        std::future::ready,
    )?;
    let recorded = replay.await_result(&config_parent_id).await?;
    active_agent
        .execution()
        .test_settle_entity_boundary(claim)
        .await?;
    drop(replay_primary);
    assert!(
        matches!(
            recorded,
            Err(golem_worker_executor::preview2::golem::agent::host::ConfigValueError::PermissionDenied)
        ),
        "completed config denial must replay without consulting a newly permissive policy: {recorded:?}"
    );

    // A replay generation retires the primary Store's live wallet. Record the reveal cases on
    // a fresh owner instead of asking that stale primary Store to admit another live body.
    let agent_id = agent_id!("ConfigAgent", "entity-secret-reveal-owner");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "echo_local_config", data_value!())
        .await?;
    let owner_id = OwnedAgentId::new(environment_id, &worker_id);
    let active_agent = executor
        .active_agent(&owner_id)
        .await
        .expect("reveal owner must be active");
    let owner_metadata =
        owner_component_metadata(&active_agent, component.id, component.revision).await?;

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
    let expected_secret = encode_graph(&SchemaGraph::anonymous(SchemaType::secret(
        Default::default(),
    )))?;
    let expected_plaintext = encode_graph(&SchemaGraph::anonymous(SchemaType::string()))?;
    let replay_expected_secret = expected_secret.clone();
    let replay_expected_plaintext = expected_plaintext.clone();
    let completed_replay_expected_secret = expected_secret.clone();
    let completed_replay_expected_plaintext = expected_plaintext.clone();
    let before_reveal_denial = active_agent.execution().oplog().current_oplog_index().await;
    let reveal_result = run_synchronous_entity_invocation(
        &active_agent,
        owner_metadata.clone(),
        &owner_id,
        &entity,
        reveal_activation.clone(),
        move |_instance, store, _principal| {
            Box::pin(async move {
                let ctx = store.data_mut().durable_ctx_mut();
                let encoded =
                    AgentHost::get_config_value(ctx, vec!["secret".to_string()], expected_secret)
                        .await?
                        .map_err(|error| WorkerExecutorError::runtime(format!("{error:?}")))?;
                let decoded = decode_value_with(encoded, ctx)
                    .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
                let SchemaValue::Secret(snapshot) = decoded else {
                    return Err(WorkerExecutorError::runtime(
                        "secret config did not produce an opaque handle",
                    ));
                };
                let handle = ctx.secret_handle_from_snapshot(&snapshot)?;
                config_loaded_in_call.store(true, Ordering::SeqCst);
                let denied = SecretRevealHost::reveal(ctx, handle, expected_plaintext).await?;
                assert!(denied.is_err(), "empty reveal scope must deny plaintext");
                GolemApiHost::get_oplog_index(ctx).await?;
                Ok(())
            })
        },
    )
    .await;
    reveal_result?;
    assert!(
        config_loaded.load(Ordering::SeqCst),
        "readable secret handles must load successfully before reveal policy rejects them"
    );
    let after_reveal_denial = active_agent.execution().oplog().current_oplog_index().await;
    let reveal_entries = active_agent
        .execution()
        .oplog()
        .read_exact(
            before_reveal_denial.next(),
            u64::from(after_reveal_denial) - u64::from(before_reveal_denial),
        )
        .await;
    let reveal_start = reveal_entries
        .iter()
        .find_map(|(index, entry)| match entry {
            OplogEntry::Start { function_name, .. }
                if *function_name == HostFunctionName::GolemSecretsReveal =>
            {
                Some(*index)
            }
            _ => None,
        })
        .expect("a secret reveal policy denial must have a durable Start");
    let entity_start = reveal_entries
        .iter()
        .find_map(|(_index, entry)| match entry {
            OplogEntry::Start {
                function_name,
                parent_start_index,
                ..
            } if *function_name == HostFunctionName::GolemAgentGetConfigValue => {
                *parent_start_index
            }
            _ => None,
        })
        .expect("the secret config call must belong to the entity invocation");
    let reveal_end = reveal_entries
        .iter()
        .find_map(|(index, entry)| match entry {
            OplogEntry::End { start_index, .. } if *start_index == reveal_start => Some(*index),
            _ => None,
        })
        .expect("a secret reveal policy denial must have a durable End");

    // The recorded denial also remains authoritative when this occurrence is changed to permit
    // reveal. Replaying the completed result must not ask current policy or wallet authority.
    let permissive_activation = Arc::new(activation_with_secret_policy(
        ExecutableTarget::new(component.id, component.revision),
        "test:restricted-secret-tool",
        agent_id.agent_type.clone(),
        ToolName::try_from("restricted-secret-tool").unwrap(),
        context.account_id,
        SecretKeyScope::All,
        SecretKeyScope::All,
    ));
    active_agent
        .execution()
        .install_replay_generation(
            DeletedRegions::from_regions([OplogRegion::from_index_range(
                OplogIndex::INITIAL.next()..=before_reveal_denial,
            )]),
            None,
        )
        .await?;
    let parent_id = OwnerInvocationId::Agent(before_reveal_denial);
    let lane = active_agent.execution().lane();
    let replay_primary = lane.enter_primary(before_reveal_denial)?.acquire().await?;
    let claim = active_agent
        .execution()
        .test_claim_entity_boundary(before_reveal_denial)
        .await?;
    assert_eq!(claim.start_idx(), entity_start);
    let replay = active_agent.start_entity_invocation(
        parent_id.clone(),
        replay_invocation_scope(
            &owner_id,
            &entity,
            entity_start,
            before_reveal_denial,
            permissive_activation.clone(),
            Principal::Agent(AgentPrincipal {
                agent_id: owner_id.agent_id.clone(),
            }),
        ),
        owner_metadata.clone(),
        EntityCallMode::Synchronous,
        move |_instance, store| {
            Box::pin(async move {
                let ctx = store.data_mut().durable_ctx_mut();
                let encoded = AgentHost::get_config_value(
                    ctx,
                    vec!["secret".to_string()],
                    completed_replay_expected_secret,
                )
                .await?
                .map_err(|error| WorkerExecutorError::runtime(format!("{error:?}")))?;
                let decoded = decode_value_with(encoded, ctx)
                    .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
                let SchemaValue::Secret(snapshot) = decoded else {
                    return Err(WorkerExecutorError::runtime(
                        "replayed secret config did not produce an opaque handle",
                    ));
                };
                let handle = ctx.secret_handle_from_snapshot(&snapshot)?;
                let recorded =
                    SecretRevealHost::reveal(ctx, handle, completed_replay_expected_plaintext)
                        .await?;
                GolemApiHost::get_oplog_index(ctx).await?;
                Ok(recorded)
            })
        },
        std::future::ready,
    )?;
    let recorded = replay.await_result(&parent_id).await?;
    active_agent
        .execution()
        .test_settle_entity_boundary(claim)
        .await?;
    drop(replay_primary);
    assert!(
        recorded.is_err(),
        "completed reveal denial must replay without consulting a newly permissive policy"
    );
    active_agent
        .execution()
        .install_replay_generation(
            DeletedRegions::from_regions([
                OplogRegion::from_index_range(OplogIndex::INITIAL.next()..=before_reveal_denial),
                OplogRegion::from_index_range(reveal_end..=after_reveal_denial),
            ]),
            None,
        )
        .await?;
    let replay_primary = lane.enter_primary(before_reveal_denial)?.acquire().await?;
    let claim = active_agent
        .execution()
        .test_claim_entity_boundary(before_reveal_denial)
        .await?;
    assert_eq!(claim.start_idx(), entity_start);
    let replay_scope = EntityInvocationScope::new(
        EntityInvocationId::new(
            OwnedAgentEntityId {
                owner: owner_id.clone(),
                entity: entity.clone(),
            },
            entity_start,
        )
        .unwrap(),
        before_reveal_denial,
        reveal_activation,
        Principal::Agent(AgentPrincipal {
            agent_id: owner_id.agent_id.clone(),
        }),
        InvocationExecutionMode::ReplayingIncomplete,
        IdempotencyKey::new("instance-layer-secret-reveal-repair".to_string()),
        true,
        false,
        IdempotencyKey::new("instance-layer-secret-reveal-repair-streams".to_string()),
    )
    .unwrap();
    let replay_parent_id = parent_id.clone();
    let replay = active_agent.start_entity_invocation(
        parent_id.clone(),
        replay_scope,
        owner_metadata,
        EntityCallMode::Synchronous,
        move |_instance, store| {
            Box::pin(async move {
                let ctx = store.data_mut().durable_ctx_mut();
                ctx.test_install_entity_tool_operation(
                    replay_parent_id,
                    EntityCallMode::Synchronous,
                )?;
                let encoded = AgentHost::get_config_value(
                    ctx,
                    vec!["secret".to_string()],
                    replay_expected_secret,
                )
                .await?
                .map_err(|error| WorkerExecutorError::runtime(format!("{error:?}")))?;
                let decoded = decode_value_with(encoded, ctx)
                    .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
                let SchemaValue::Secret(snapshot) = decoded else {
                    return Err(WorkerExecutorError::runtime(
                        "replayed secret config did not produce an opaque handle",
                    ));
                };
                let handle = ctx.secret_handle_from_snapshot(&snapshot)?;
                let repaired =
                    SecretRevealHost::reveal(ctx, handle, replay_expected_plaintext).await?;
                GolemApiHost::get_oplog_index(ctx).await?;
                Ok(repaired.is_err())
            })
        },
        std::future::ready,
    )?;
    assert!(
        replay.await_result(&parent_id).await?,
        "a Start-only reveal repair must rerun occurrence and wallet authorization"
    );
    active_agent
        .execution()
        .test_settle_entity_boundary(claim)
        .await?;
    drop(replay_primary);
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
    let outage = Arc::new(crate::hot_update::Outage::default());
    let wrapped_outage = outage.clone();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            wrap_component_service: Some(Arc::new(move |inner| {
                Arc::new(crate::hot_update::FlakyComponentService {
                    inner,
                    outage: wrapped_outage.clone(),
                })
            })),
            ..Default::default()
        },
    )
    .await?;
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
    let lower_entity_component = entity_component.clone();
    let entity_component = executor
        .update_component(&entity_component.id, &host_api_tests.wasm_name)
        .await?;
    let entity_component = executor
        .update_component(&entity_component.id, &host_api_tests.wasm_name)
        .await?;
    assert_ne!(owner_component.id, entity_component.id);
    assert_ne!(owner_component.revision, entity_component.revision);
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
    let owner_update = executor
        .update_component(&owner_component.id, &agent_sdk_rust.wasm_name)
        .await?;
    assert!(lower_entity_component.revision < owner_update.revision);
    assert!(entity_component.revision > owner_update.revision);
    let target_revision = owner_update.revision;
    let tool_name = ToolName::try_from("owner-config-reader").unwrap();
    let entity = AgentEntity::Tool(tool_name.clone());
    let expected_owner_component_id = owner_component.id;
    let expected_owner_component_revision = owner_component.revision;
    let expected_entity_component_id = entity_component.id;
    let foo_path = CanonicalAgentConfigPath(vec!["foo".to_string()]);
    let cases = [
        (ConfigKeyScope::All, "foo", SchemaType::s32(), true),
        (
            ConfigKeyScope::Keys(BTreeSet::from([foo_path.clone()])),
            "foo",
            SchemaType::s32(),
            true,
        ),
        (
            ConfigKeyScope::Keys(BTreeSet::from([foo_path])),
            "bar",
            SchemaType::string(),
            false,
        ),
        (
            ConfigKeyScope::Keys(BTreeSet::new()),
            "foo",
            SchemaType::s32(),
            false,
        ),
    ];
    for entity_component in [lower_entity_component, entity_component] {
        let expected_entity_component_revision = entity_component.revision;
        for (scope, key, expected_type, allowed) in cases.clone() {
            let outage = outage.clone();
            let activation = Arc::new(activation_with_config_policy(
                ExecutableTarget::new(entity_component.id, entity_component.revision),
                "test:owner-config-reader",
                agent_id.agent_type.clone(),
                tool_name.clone(),
                context.account_id,
                scope,
            ));
            let expected = encode_graph(&SchemaGraph::anonymous(expected_type))?;
            let key = key.to_string();
            // The executable deliberately has no LocalConfigAgent declaration. These lookups prove
            // both declaration ownership and binding-policy narrowing against creation overrides.
            run_synchronous_entity_invocation(
            &active_agent,
            owner_metadata.clone(),
            &owner_id,
            &entity,
            activation,
            move |_instance, store, _principal| {
                Box::pin(async move {
                    let ctx = store.data_mut().durable_ctx_mut();
                    let scope_before = ctx.entity_invocation_scope().unwrap().clone();
                    let (_, historical_before, canonical_before, _) = ctx.test_replay_application_state();
                    let fetches_before = outage.metadata_fetches();
                    ctx.test_stage_replay_applications(vec![
                        golem_worker_executor::durable_host::TestReplayEvent::UpdateReplayed {
                            new_revision: target_revision,
                        },
                    ])?;
                    ctx.process_pending_replay_events().await?;
                    assert!(ctx.test_replay_application_state().0.is_empty(), "Entity must not prepare an owner revision update");
                    assert_eq!(
                        store.data().component_metadata().id,
                        expected_entity_component_id
                    );
                    assert_eq!(
                        store.data().component_metadata().revision,
                        expected_entity_component_revision
                    );
                    assert_eq!(
                        store.data().durable_ctx().owner_component_metadata().id,
                        expected_owner_component_id
                    );
                    assert_eq!(
                        store
                            .data()
                            .durable_ctx()
                            .owner_component_metadata()
                            .revision,
                        expected_owner_component_revision
                    );

                    let config = AgentHost::get_config_value(
                        store.data_mut().durable_ctx_mut(),
                        vec![key.clone()],
                        expected,
                    )
                    .await?;
                    assert_eq!(
                        config.is_ok(),
                        allowed,
                        "unexpected policy result for {key}"
                    );
                    if let Ok(config) = config {
                        let config = decode_value(&config)
                            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
                        assert_eq!(config, SchemaValue::S32(7));
                    }

                    let agent_type = AgentHost::get_agent_type(
                        store.data_mut().durable_ctx_mut(),
                        "LocalConfigAgent".to_string(),
                    )
                    .await?;
                    assert!(agent_type.is_some());
                    let ctx = store.data().durable_ctx();
                    assert_eq!(ctx.entity_invocation_scope().unwrap(), &scope_before,
                        "activation and recorded authority wallet must remain pinned");
                    let (_, historical, canonical, _) = ctx.test_replay_application_state();
                    assert_eq!((historical, canonical), (historical_before, canonical_before));
                    assert_eq!(outage.metadata_fetches(), fetches_before,
                        "Entity must not fetch update metadata or start initial-file preparation");
                    Ok(())
                })
            },
        )
        .await?;
        }
    }
    Ok(())
}
