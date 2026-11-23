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
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use golem_api_grpc::proto::golem::worker::UpdateMode;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    DeliverCardTransferRequest, deliver_card_transfer_response,
};
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::agent::{AgentConfigEntryDto, ResolvedRevert, RevertWorkerTarget};
use golem_common::model::agent::{AgentInvocationMode, InvocationFreshnessDisposition};
use golem_common::model::card::{
    CardId, CardManagedByRuntimeDerived, PolymorphicPermissionPattern, ScopeCard, StoredCard,
    parse_polymorphic_permission,
};
use golem_common::model::component::{AgentTypeProvisionConfigUpdate, ComponentId, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{
    AgentFingerprint, AgentId, AgentInvocationOutput, IdempotencyKey, OwnedAgentId, PromiseId,
};
use golem_common::schema::SchemaValue;
use golem_common::{agent_key, data_value};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::card::{
    CardRevokeResult, CardService, CardState, RuntimeCardCreation,
};
use golem_worker_executor::services::worker_proxy::{WorkerProxy, WorkerProxyError};
use golem_worker_executor::worker::target::ResolvedTarget;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("permissions_effect")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

#[test]
#[timeout("120s")]
async fn effect_static_config_captures_survive_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("permissions_effect")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::oplog::PublicOplogEntry;

    let corpus: Value =
        serde_json::from_str(include_str!("../../test-data/config-conformance/v1.json"))?;
    let reference = &corpus["reference"];
    let agent_type = "EffectStaticDefaultsAgent";
    let entries = |layer: &str| {
        reference[layer]
            .as_object()
            .unwrap()
            .iter()
            .map(|(path, value)| AgentConfigEntryDto {
                path: vec![path.clone()],
                value: value.clone().into(),
            })
            .collect()
    };
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    let mut component = executor
        .component_dep(&context.default_environment_id, component)
        .with_agent_config(agent_type, entries("deployment"))
        .store()
        .await?;
    let agent_id = agent_key!(agent_type, "creation");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::new(),
            entries("creation"),
        )
        .await?;
    let deployed_agent_id = agent_key!(agent_type, "deployment");
    let deployed_worker_id = executor
        .start_agent(&component.id, deployed_agent_id.clone())
        .await?;
    let mut expected_deployment = reference["expectedDeploymentRead"].clone();
    let mut expected_creation = reference["expectedRead"].clone();
    for expected in [&mut expected_deployment, &mut expected_creation] {
        expected["choice"] = corpus["staticCases"][0]["expected"]["choice"].clone();
    }
    let reads = [
        (&deployed_agent_id, &expected_deployment),
        (&agent_id, &expected_creation),
    ];
    for (id, expected) in reads {
        for method in ["constructorRead", "read"] {
            let actual = executor
                .invoke_and_await_agent(&component, id, method, data_value!())
                .await?
                .into_typed::<String>()?;
            assert_eq!(&serde_json::from_str::<Value>(&actual)?, expected);
        }
    }
    crate::hot_update::wait_for_snapshot_after(&executor, &worker_id, OplogIndex::INITIAL).await?;
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    for (id, expected) in reads {
        for method in ["constructorRead", "read"] {
            let actual = executor
                .invoke_and_await_agent(&component, id, method, data_value!())
                .await?
                .into_typed::<String>()?;
            assert_eq!(&serde_json::from_str::<Value>(&actual)?, expected);
        }
    }
    let mut current_expected = corpus["staticCases"][0]["expected"].clone();
    for (label, count, config) in [
        (
            "new-revision",
            0,
            Some(vec![AgentConfigEntryDto {
                path: vec!["settings".into()],
                value: json!({"label":"new-revision", "count":0}).into(),
            }]),
        ),
        ("new-revision", 0, None),
        ("code-default", 31, Some(vec![])),
    ] {
        component = executor
            .update_component_with(
                &component.id,
                component.revision,
                None,
                Some(BTreeMap::from([(
                    AgentTypeName(agent_type.into()),
                    AgentTypeProvisionConfigUpdate {
                        config,
                        ..Default::default()
                    },
                )])),
                vec![],
            )
            .await?;
        current_expected["settings"] = json!({"label":label, "count":count});
        for ((id, expected), worker) in [
            (&deployed_agent_id, &current_expected),
            (&agent_id, &expected_creation),
        ]
        .into_iter()
        .zip([&deployed_worker_id, &worker_id])
        {
            executor
                .auto_update_worker(worker, component.revision, false)
                .await?;
            let actual = executor
                .invoke_and_await_agent(&component, id, "read", data_value!())
                .await?
                .into_typed::<String>()?;
            assert_eq!(&serde_json::from_str::<Value>(&actual)?, expected);
            assert_eq!(
                executor
                    .get_worker_metadata(worker)
                    .await?
                    .component_revision,
                component.revision
            );
        }
    }
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    for (id, expected) in [
        (&deployed_agent_id, &current_expected),
        (&agent_id, &expected_creation),
    ] {
        let actual = executor
            .invoke_and_await_agent(&component, id, "read", data_value!())
            .await?
            .into_typed::<String>()?;
        assert_eq!(&serde_json::from_str::<Value>(&actual)?, expected);
    }
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    for entry in &oplog {
        if let PublicOplogEntry::Start(_) = &entry.entry {
            assert!(
                oplog.iter().any(|terminal| matches!(
                    &terminal.entry,
                    PublicOplogEntry::End(end) if end.start_index == entry.oplog_index
                )),
                "config call at {} must be settled",
                entry.oplog_index
            );
        }
    }
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
            .count(),
        9
    );
    Ok(())
}

/// Card registry for the test: initial cards and runtime-derived cards are live until revoked,
/// and revoking a card revokes every stored descendant.
#[derive(Default)]
struct CardRegistry {
    cards: RwLock<HashMap<CardId, StoredCard>>,
    revoked: RwLock<HashSet<CardId>>,
}

impl CardRegistry {
    fn add(&self, card: StoredCard) {
        self.cards.write().unwrap().insert(card.card_id(), card);
    }

    fn subtree(&self, root: CardId) -> Vec<CardId> {
        let cards = self.cards.read().unwrap();
        let mut subtree = vec![root];
        let mut index = 0;
        while index < subtree.len() {
            let parent = subtree[index];
            for card in cards.values() {
                if card.parent_ids().contains(&parent) && !subtree.contains(&card.card_id()) {
                    subtree.push(card.card_id());
                }
            }
            index += 1;
        }
        subtree
    }
}

struct RegistryCardService {
    registry: Arc<CardRegistry>,
}

#[async_trait]
impl CardService for RegistryCardService {
    async fn record_revoked_cards(&self, card_ids: &[CardId]) {
        self.registry
            .revoked
            .write()
            .unwrap()
            .extend(card_ids.iter().copied());
    }

    async fn check_cards(
        &self,
        card_ids: Vec<CardId>,
    ) -> Result<HashMap<CardId, CardState>, WorkerExecutorError> {
        let cards = self.registry.cards.read().unwrap();
        let revoked = self.registry.revoked.read().unwrap();
        Ok(card_ids
            .into_iter()
            .map(|card_id| {
                let state = if revoked.contains(&card_id) {
                    CardState::Revoked
                } else if let Some(card) = cards.get(&card_id) {
                    CardState::Live(Box::new(card.clone()))
                } else {
                    CardState::Unknown
                };
                (card_id, state)
            })
            .collect())
    }

    async fn create_runtime_card(
        &self,
        card: StoredCard,
        _provenance: CardManagedByRuntimeDerived,
    ) -> Result<RuntimeCardCreation, WorkerExecutorError> {
        self.registry.add(card.clone());
        Ok(RuntimeCardCreation::Created(Box::new(card)))
    }

    async fn revoke_card(&self, card_id: CardId) -> Result<CardRevokeResult, WorkerExecutorError> {
        let subtree = self.registry.subtree(card_id);
        self.registry
            .revoked
            .write()
            .unwrap()
            .extend(subtree.iter().copied());
        Ok(CardRevokeResult::Revoked(subtree))
    }

    async fn live_ancestor_ids_including_self(
        &self,
        card: &StoredCard,
    ) -> Result<Vec<CardId>, WorkerExecutorError> {
        Ok(std::iter::once(card.card_id())
            .chain(card.parent_ids().iter().copied())
            .collect())
    }
}

type CardTransferDelivery = dyn Fn(DeliverCardTransferRequest) -> BoxFuture<'static, Result<(), WorkerProxyError>>
    + Send
    + Sync;

/// Delivers permission-card transfers to the executor under test instead of the worker service,
/// and delegates every other operation.
struct LoopbackCardTransferProxy {
    inner: Arc<dyn WorkerProxy>,
    delivery: Arc<Mutex<Option<Arc<CardTransferDelivery>>>>,
    delivered: Arc<Mutex<Vec<(AgentId, CardId)>>>,
}

fn connect_card_transfer_delivery(
    delivery: &Mutex<Option<Arc<CardTransferDelivery>>>,
    executor: &TestWorkerExecutor,
) {
    let client = executor.client.clone();
    *delivery.lock().unwrap() = Some(Arc::new(move |request| {
        let mut client = client.clone();
        Box::pin(async move {
            let response = client.deliver_card_transfer(request).await?.into_inner();
            match response.result {
                Some(deliver_card_transfer_response::Result::Success(_)) => Ok(()),
                Some(deliver_card_transfer_response::Result::Failure(error)) => {
                    Err(WorkerProxyError::InternalError(
                        WorkerExecutorError::try_from(error).map_err(|error| {
                            WorkerProxyError::InternalError(WorkerExecutorError::unknown(error))
                        })?,
                    ))
                }
                None => Err(WorkerProxyError::InternalError(
                    WorkerExecutorError::unknown("card transfer delivery returned no result"),
                )),
            }
        })
    }));
}

#[async_trait]
impl WorkerProxy for LoopbackCardTransferProxy {
    async fn prepare(
        &self,
        target: &ResolvedTarget,
        method_name: &str,
        caller_agent_id: &AgentId,
        caller_env: HashMap<String, String>,
        caller_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        caller: golem_common::model::caller::Caller,
        auth_ctx: &AuthCtx,
        idempotency_key: Option<IdempotencyKey>,
        freshness_disposition: golem_common::model::agent::InvocationFreshnessDisposition,
    ) -> Result<AgentFingerprint, WorkerProxyError> {
        self.inner
            .prepare(
                target,
                method_name,
                caller_agent_id,
                caller_env,
                caller_stack,
                config,
                caller,
                auth_ctx,
                idempotency_key,
                freshness_disposition,
            )
            .await
    }

    async fn create(
        &self,
        target: &ResolvedTarget,
        exclusive: bool,
        caller_agent_id: &AgentId,
        caller_env: HashMap<String, String>,
        caller_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        caller: golem_common::model::caller::Caller,
        auth_ctx: &AuthCtx,
        idempotency_key: Option<IdempotencyKey>,
    ) -> Result<AgentFingerprint, WorkerProxyError> {
        self.inner
            .create(
                target,
                exclusive,
                caller_agent_id,
                caller_env,
                caller_stack,
                config,
                caller,
                auth_ctx,
                idempotency_key,
            )
            .await
    }

    async fn invoke_agent(
        &self,
        target: &ResolvedTarget,
        method_name: String,
        method_parameters: SchemaValue,
        mode: AgentInvocationMode,
        schedule_at: Option<DateTime<Utc>>,
        idempotency_key: Option<IdempotencyKey>,
        freshness_disposition: InvocationFreshnessDisposition,
        expected_callee_fingerprint: Option<AgentFingerprint>,
        caller_agent_id: AgentId,
        caller_env: HashMap<String, String>,
        caller_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        caller: golem_common::model::caller::Caller,
        environment_id: EnvironmentId,
        auth_ctx: &AuthCtx,
        scope_card: Option<ScopeCard>,
    ) -> Result<AgentInvocationOutput, WorkerProxyError> {
        self.inner
            .invoke_agent(
                target,
                method_name,
                method_parameters,
                mode,
                schedule_at,
                idempotency_key,
                freshness_disposition,
                expected_callee_fingerprint,
                caller_agent_id,
                caller_env,
                caller_stack,
                config,
                caller,
                environment_id,
                auth_ctx,
                scope_card,
            )
            .await
    }

    async fn deliver_card_transfer(
        &self,
        target_agent_id: &AgentId,
        environment_id: EnvironmentId,
        transfer_id: uuid::Uuid,
        source_card_id: CardId,
        card: &StoredCard,
    ) -> Result<(), WorkerProxyError> {
        let delivery = self.delivery.lock().unwrap().clone().ok_or_else(|| {
            WorkerProxyError::InternalError(WorkerExecutorError::runtime(
                "card transfer delivery is not connected",
            ))
        })?;
        let request = DeliverCardTransferRequest {
            target_agent_id: Some((*target_agent_id).into()),
            environment_id: Some(environment_id.into()),
            transfer_id: Some(transfer_id.into()),
            card: desert_rust::serialize_to_byte_vec(card).map_err(|error| {
                WorkerProxyError::InternalError(WorkerExecutorError::unknown(format!(
                    "failed to serialize card transfer: {error}"
                )))
            })?,
            auth_ctx: Some(AuthCtx::System.into()),
            source_card_id: Some(source_card_id.0.into()),
        };
        delivery(request).await?;
        self.delivered
            .lock()
            .unwrap()
            .push((*target_agent_id, card.card_id()));
        Ok(())
    }

    async fn update(
        &self,
        owned_agent_id: &OwnedAgentId,
        target_revision: ComponentRevision,
        mode: UpdateMode,
        disable_wakeup: bool,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        self.inner
            .update(
                owned_agent_id,
                target_revision,
                mode,
                disable_wakeup,
                auth_ctx,
            )
            .await
    }

    async fn resume(
        &self,
        owned_agent_id: &OwnedAgentId,
        force: bool,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        self.inner.resume(owned_agent_id, force, auth_ctx).await
    }

    async fn fork_worker(
        &self,
        source_agent_id: &OwnedAgentId,
        target_agent_id: &AgentId,
        oplog_index_cutoff: &OplogIndex,
        caller: golem_common::model::caller::Caller,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        self.inner
            .fork_worker(
                source_agent_id,
                target_agent_id,
                oplog_index_cutoff,
                caller,
                auth_ctx,
            )
            .await
    }

    async fn revert(
        &self,
        owned_agent_id: &OwnedAgentId,
        target: RevertWorkerTarget,
        resolved_revert: Option<ResolvedRevert>,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        self.inner
            .revert(owned_agent_id, target, resolved_revert, auth_ctx)
            .await
    }

    async fn complete_promise(
        &self,
        environment_id: EnvironmentId,
        promise_id: PromiseId,
        data: Vec<u8>,
        auth_ctx: &AuthCtx,
    ) -> Result<bool, WorkerProxyError> {
        self.inner
            .complete_promise(environment_id, promise_id, data, auth_ctx)
            .await
    }

    async fn cancel_invocation(
        &self,
        owned_agent_id: &OwnedAgentId,
        idempotency_key: golem_common::model::InvocationId,
        auth_ctx: &AuthCtx,
    ) -> Result<bool, WorkerProxyError> {
        self.inner
            .cancel_invocation(owned_agent_id, idempotency_key, auth_ctx)
            .await
    }

    async fn cancel_scheduled_invocation(
        &self,
        target: &golem_common::model::OwnedAgentId,
        key: golem_common::model::InvocationId,
        receipt: golem_common::model::agent::ScheduleId,
        canceller: &golem_common::model::caller::Principal,
        auth: &AuthCtx,
    ) -> Result<bool, WorkerProxyError> {
        self.inner
            .cancel_scheduled_invocation(target, key, receipt, canceller, auth)
            .await
    }

    async fn lookup_invocation_status(
        &self,
        agent_id: &AgentId,
        idempotency_key: golem_common::model::InvocationId,
        environment_id: Option<EnvironmentId>,
        auth_ctx: &AuthCtx,
    ) -> Result<golem_common::model::InvocationStatus, WorkerProxyError> {
        self.inner
            .lookup_invocation_status(agent_id, idempotency_key, environment_id, auth_ctx)
            .await
    }

    async fn process_oplog_entries(
        &self,
        target_agent_id: &AgentId,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        idempotency_key: golem_common::model::InvocationId,
        account_id: AccountId,
        config: Vec<(String, String)>,
        metadata: golem_api_grpc::proto::golem::worker::AgentMetadata,
        first_entry_index: OplogIndex,
        entries: Vec<golem_api_grpc::proto::golem::worker::RawOplogEntry>,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        self.inner
            .process_oplog_entries(
                target_agent_id,
                environment_id,
                component_id,
                component_revision,
                idempotency_key,
                account_id,
                config,
                metadata,
                first_entry_index,
                entries,
                auth_ctx,
            )
            .await
    }
}

fn permission(pattern: &str) -> PolymorphicPermissionPattern {
    parse_polymorphic_permission(pattern).expect("valid test permission")
}

fn probe(wallet_kinds: &[&str], inspect_allowed: bool) -> Value {
    json!({ "walletKinds": wallet_kinds, "inspectAllowed": inspect_allowed, "scopeExpiresAt": null })
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn effect_sdk_manages_permission_cards_end_to_end(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("permissions_effect")] permissions_effect: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let registry = Arc::new(CardRegistry::default());
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let delivery = Arc::new(Mutex::new(None));
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            wrap_worker_proxy: Some(Arc::new({
                let delivered = delivered.clone();
                let delivery = delivery.clone();
                move |inner| {
                    Arc::new(LoopbackCardTransferProxy {
                        inner,
                        delivery: delivery.clone(),
                        delivered: delivered.clone(),
                    })
                }
            })),
            create_card_service: Some(Arc::new({
                let registry = registry.clone();
                move || {
                    Arc::new(RegistryCardService {
                        registry: registry.clone(),
                    })
                }
            })),
            ..Default::default()
        },
    )
    .await?;
    connect_card_transfer_delivery(&delivery, &executor);
    let component = executor
        .component_dep(&context.default_environment_id, permissions_effect)
        .unique()
        .update_agent_provision_config("EffectPermissionCaller", |config| {
            config.initial_permissions.lower_bound.positive.extend([
                permission("card(*) @ * : derive : *"),
                permission("card(*) @ * : inspect : *"),
                permission("card(*) @ * : revoke : *"),
                permission("card(*) @ * : install : *"),
            ]);
        })
        .without_default_host_permissions("EffectPermissionCallee")
        .store()
        .await?;
    for agent_type in ["EffectPermissionCaller", "EffectPermissionCallee"] {
        let card = component
            .metadata
            .agent_type_provision_configs
            .get(&AgentTypeName(agent_type.to_string()))
            .map(|config| &config.initial_permissions)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing initial card for {agent_type}"))?;
        registry.add(StoredCard::Polymorphic(card));
    }

    let caller = agent_key!("EffectPermissionCaller", "caller");
    let callee_name = "callee";
    let callee = agent_key!("EffectPermissionCallee", callee_name);
    let caller_worker = executor.start_agent(&component.id, caller.clone()).await?;
    let callee_worker = executor.start_agent(&component.id, callee.clone()).await?;

    let result = executor
        .invoke_and_await_agent(&component, &caller, "run", data_value!(callee_name))
        .await?
        .into_typed::<String>()?;
    let result: Value = serde_json::from_str(&result)?;

    let inspect_grant = json!({
        "class": "card", "owner": "*", "recipient": "*", "verb": "inspect", "resourceId": "*"
    });
    assert_eq!(
        result["derivedView"],
        json!({
            "lowerPositive": [inspect_grant],
            "lowerNegative": [],
            "upperPositive": [inspect_grant],
            "upperNegative": [],
        }),
        "{result:#}"
    );
    assert_eq!(result["childParentMatches"], json!(true), "{result:#}");
    assert_eq!(result["childExpires"], json!(true), "{result:#}");
    assert_eq!(result["isoChildExpires"], json!(true), "{result:#}");
    assert_eq!(result["cardExpires"], json!(false), "{result:#}");
    assert_eq!(result["templateDerive"], json!("ok"), "{result:#}");
    assert_eq!(result["walletKinds"], json!(["Template"]), "{result:#}");

    assert_eq!(
        result["calleeBefore"],
        probe(&["Template"], false),
        "{result:#}"
    );
    assert_eq!(
        result["calleePerCallScope"],
        probe(&["Scope", "Template"], true),
        "{result:#}"
    );
    assert_eq!(
        result["calleeDefaultScope"],
        probe(&["Scope", "Template"], true),
        "{result:#}"
    );
    assert_eq!(
        result["calleeAfterScope"],
        probe(&["Template"], false),
        "{result:#}"
    );
    assert_eq!(result["scopeCardStillUsable"], json!(true), "{result:#}");

    assert_eq!(result["scopeExpiresAt"], Value::Null, "{result:#}");

    let expiring = result["expiringExpiresAt"].clone();
    assert!(expiring.is_string(), "{result:#}");
    let expires_in = result["expiringExpiresInSeconds"]
        .as_f64()
        .expect("expiry in seconds");
    assert!((3500.0..=3700.0).contains(&expires_in), "{result:#}");
    assert_eq!(
        result["walletKindsAfterSelfInstall"],
        json!(["Concrete", "Template"]),
        "{result:#}"
    );
    assert_eq!(result["installedOnSelfExpiresAt"], expiring, "{result:#}");
    assert_eq!(result["expiringScopeExpiresAt"], expiring, "{result:#}");
    assert_eq!(
        result["calleeExpiringScope"],
        json!({
            "walletKinds": ["Scope", "Template"],
            "inspectAllowed": true,
            "scopeExpiresAt": expiring,
        }),
        "{result:#}"
    );

    assert_eq!(
        result["calleeAfterInstall"],
        probe(&["Concrete", "Template"], true),
        "{result:#}"
    );
    assert_eq!(
        result["installedCardMoved"],
        json!("PermissionsHostError"),
        "{result:#}"
    );

    assert_eq!(result["revokedCount"], json!(4), "{result:#}");
    assert_eq!(
        result["revokedCardReuse"],
        json!("PermissionsHostError"),
        "{result:#}"
    );
    assert_eq!(
        result["calleeAfterRevoke"],
        probe(&["Template"], false),
        "{result:#}"
    );

    assert_eq!(
        result["releasedScopeCall"],
        json!("ScopeCardUnavailableError"),
        "{result:#}"
    );
    assert_eq!(
        result["recipientMismatch"],
        json!("install-recipient-mismatch"),
        "{result:#}"
    );
    assert_eq!(
        result["accountHolder"],
        json!("not-permitted"),
        "{result:#}"
    );

    let stale = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "staleScopeCall",
            data_value!(callee_name),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(stale, "RpcCallError:denied");

    assert_eq!(
        result["calleeReceivedTemplate"],
        json!({ "template": true, "concrete": false }),
        "{result:#}"
    );
    assert_eq!(
        result["templateSent"],
        json!("PermissionsHostError"),
        "{result:#}"
    );

    let delivered = delivered.lock().unwrap().clone();
    let targets: Vec<_> = delivered.iter().map(|(target, _)| *target).collect();
    assert_eq!(
        targets,
        vec![callee_worker.agent_id, caller_worker.agent_id],
        "the installed card reaches the callee and the self-installed card reaches the caller"
    );

    let denied = executor
        .invoke_and_await_agent(&component, &callee, "deriveWithoutAuthority", data_value!())
        .await?
        .into_typed::<String>()?;
    assert_eq!(denied, "not-permitted");

    executor.check_oplog_is_queryable(&caller_worker).await?;
    executor.check_oplog_is_queryable(&callee_worker).await?;
    Ok(())
}
