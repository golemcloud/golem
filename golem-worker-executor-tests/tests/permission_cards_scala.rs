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

//! Runs the shared permission-card scenario through the typed Scala SDK API.

use crate::Tracing;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use golem_api_grpc::proto::golem::worker::UpdateMode;
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentConfigEntryDto, ResolvedRevert, RevertWorkerTarget};
use golem_common::model::agent::{AgentInvocationMode, InvocationFreshnessDisposition, Principal};
use golem_common::model::card::{
    CardId, CardManagedByRuntimeDerived, PolymorphicPermissionPattern, ScopeCard, StoredCard,
    parse_polymorphic_permission,
};
use golem_common::model::component::ComponentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{
    AgentFingerprint, AgentId, AgentInvocationOutput, IdempotencyKey, OwnedAgentId, PromiseId,
};
use golem_common::schema::{SchemaType, SchemaValue};
use golem_common::{agent_id, data_value};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::card::{
    CardRevokeResult, CardService, CardState, RuntimeCardCreation,
};
use golem_worker_executor::services::worker_proxy::{WorkerProxy, WorkerProxyError};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("tool_streaming_scala")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

/// A card store that knows the agent's initial card and every derived card, and
/// revokes a card together with its descendants that are not revoked yet.
#[derive(Default)]
struct CardStore {
    cards: RwLock<HashMap<CardId, StoredCard>>,
    revoked: RwLock<HashSet<CardId>>,
}

impl CardStore {
    fn add(&self, card: StoredCard) {
        self.cards
            .write()
            .expect("card store lock poisoned")
            .insert(card.card_id(), card);
    }

    fn descendants_including_self(&self, card_id: CardId) -> Vec<CardId> {
        let cards = self.cards.read().expect("card store lock poisoned");
        let mut result = vec![card_id];
        let mut index = 0;
        while index < result.len() {
            let parent = result[index];
            for card in cards.values() {
                if card.parent_ids().contains(&parent) && !result.contains(&card.card_id()) {
                    result.push(card.card_id());
                }
            }
            index += 1;
        }
        result
    }
}

struct CardStoreService {
    store: Arc<CardStore>,
}

#[async_trait]
impl CardService for CardStoreService {
    async fn record_revoked_cards(&self, card_ids: &[CardId]) {
        self.store
            .revoked
            .write()
            .expect("card store lock poisoned")
            .extend(card_ids.iter().copied());
    }

    async fn create_runtime_card(
        &self,
        card: StoredCard,
        _provenance: CardManagedByRuntimeDerived,
    ) -> Result<RuntimeCardCreation, WorkerExecutorError> {
        self.store.add(card.clone());
        Ok(RuntimeCardCreation::Created(Box::new(card)))
    }

    async fn revoke_card(&self, card_id: CardId) -> Result<CardRevokeResult, WorkerExecutorError> {
        // Read the card graph before taking the revoked-set lock; `check_cards`
        // takes the two locks in the opposite order.
        let descendants = self.store.descendants_including_self(card_id);
        let mut already_revoked = self
            .store
            .revoked
            .write()
            .expect("card store lock poisoned");
        let revoked: Vec<CardId> = descendants
            .into_iter()
            .filter(|card_id| !already_revoked.contains(card_id))
            .collect();
        already_revoked.extend(revoked.iter().copied());
        Ok(CardRevokeResult::Revoked(revoked))
    }

    async fn live_ancestor_ids_including_self(
        &self,
        card: &StoredCard,
    ) -> Result<Vec<CardId>, WorkerExecutorError> {
        Ok(std::iter::once(card.card_id())
            .chain(card.parent_ids().iter().copied())
            .collect())
    }

    async fn check_cards(
        &self,
        card_ids: Vec<CardId>,
    ) -> Result<HashMap<CardId, CardState>, WorkerExecutorError> {
        let cards = self.store.cards.read().expect("card store lock poisoned");
        let revoked = self.store.revoked.read().expect("card store lock poisoned");
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
}

type CardDelivery = dyn Fn(
        AgentId,
        EnvironmentId,
        uuid::Uuid,
        CardId,
        StoredCard,
    ) -> BoxFuture<'static, Result<(), WorkerProxyError>>
    + Send
    + Sync;

/// Delivers permission-card transfers to the executor under test, which owns
/// every agent of the scenario. Every other call goes to the wrapped proxy.
struct LocalCardDeliveryProxy {
    inner: Arc<dyn WorkerProxy>,
    delivery: Arc<Mutex<Option<Arc<CardDelivery>>>>,
}

fn connect_local_card_delivery(
    delivery: &Arc<Mutex<Option<Arc<CardDelivery>>>>,
    executor: &TestWorkerExecutor,
) {
    let client = executor.client.clone();
    *delivery.lock().unwrap() = Some(Arc::new(
        move |target_agent_id, environment_id, transfer_id, source_card_id, card| {
            let mut client = client.clone();
            Box::pin(async move {
                use golem_api_grpc::proto::golem::workerexecutor::v1::{
                    DeliverCardTransferRequest, deliver_card_transfer_response,
                };

                let response = client
                    .deliver_card_transfer(DeliverCardTransferRequest {
                        target_agent_id: Some(target_agent_id.into()),
                        environment_id: Some(environment_id.into()),
                        transfer_id: Some(transfer_id.into()),
                        card: desert_rust::serialize_to_byte_vec(&card).map_err(|error| {
                            WorkerProxyError::InternalError(WorkerExecutorError::unknown(format!(
                                "failed to serialize local card transfer: {error}"
                            )))
                        })?,
                        auth_ctx: Some(AuthCtx::System.into()),
                        source_card_id: Some(source_card_id.0.into()),
                    })
                    .await?
                    .into_inner();
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
                        WorkerExecutorError::unknown(
                            "local permission-card delivery returned no result",
                        ),
                    )),
                }
            })
        },
    ));
}

#[async_trait]
impl WorkerProxy for LocalCardDeliveryProxy {
    async fn prepare(
        &self,
        owned_agent_id: &OwnedAgentId,
        method_name: &str,
        caller_agent_id: &AgentId,
        caller_env: HashMap<String, String>,
        caller_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        principal: Principal,
        auth_ctx: &AuthCtx,
        idempotency_key: Option<IdempotencyKey>,
        freshness_disposition: golem_common::model::agent::InvocationFreshnessDisposition,
    ) -> Result<AgentFingerprint, WorkerProxyError> {
        self.inner
            .prepare(
                owned_agent_id,
                method_name,
                caller_agent_id,
                caller_env,
                caller_stack,
                config,
                principal,
                auth_ctx,
                idempotency_key,
                freshness_disposition,
            )
            .await
    }

    async fn start(
        &self,
        owned_agent_id: &OwnedAgentId,
        method_name: &str,
        caller_agent_id: &AgentId,
        caller_env: HashMap<String, String>,
        caller_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        principal: Principal,
        auth_ctx: &AuthCtx,
    ) -> Result<AgentFingerprint, WorkerProxyError> {
        self.inner
            .start(
                owned_agent_id,
                method_name,
                caller_agent_id,
                caller_env,
                caller_stack,
                config,
                principal,
                auth_ctx,
            )
            .await
    }

    async fn invoke_agent(
        &self,
        agent_id: &AgentId,
        method_name: String,
        method_parameters: SchemaValue,
        mode: AgentInvocationMode,
        schedule_at: Option<DateTime<Utc>>,
        idempotency_key: Option<IdempotencyKey>,
        freshness_disposition: InvocationFreshnessDisposition,
        caller_agent_id: AgentId,
        caller_env: HashMap<String, String>,
        caller_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        principal: Principal,
        environment_id: EnvironmentId,
        auth_ctx: &AuthCtx,
        scope_card: Option<ScopeCard>,
    ) -> Result<AgentInvocationOutput, WorkerProxyError> {
        self.inner
            .invoke_agent(
                agent_id,
                method_name,
                method_parameters,
                mode,
                schedule_at,
                idempotency_key,
                freshness_disposition,
                caller_agent_id,
                caller_env,
                caller_stack,
                config,
                principal,
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
                "local card-transfer delivery is not connected",
            ))
        })?;
        delivery(
            target_agent_id.clone(),
            environment_id,
            transfer_id,
            source_card_id,
            card.clone(),
        )
        .await
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
        owned_agent_id: &AgentId,
        force: bool,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        self.inner.resume(owned_agent_id, force, auth_ctx).await
    }

    async fn fork_worker(
        &self,
        source_agent_id: &AgentId,
        target_agent_id: &AgentId,
        oplog_index_cutoff: &OplogIndex,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        self.inner
            .fork_worker(
                source_agent_id,
                target_agent_id,
                oplog_index_cutoff,
                auth_ctx,
            )
            .await
    }

    async fn revert(
        &self,
        agent_id: &AgentId,
        target: RevertWorkerTarget,
        resolved_revert: Option<ResolvedRevert>,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        self.inner
            .revert(agent_id, target, resolved_revert, auth_ctx)
            .await
    }

    async fn complete_promise(
        &self,
        promise_id: PromiseId,
        data: Vec<u8>,
        auth_ctx: &AuthCtx,
    ) -> Result<bool, WorkerProxyError> {
        self.inner
            .complete_promise(promise_id, data, auth_ctx)
            .await
    }

    async fn cancel_invocation(
        &self,
        agent_id: &AgentId,
        idempotency_key: IdempotencyKey,
        auth_ctx: &AuthCtx,
    ) -> Result<bool, WorkerProxyError> {
        self.inner
            .cancel_invocation(agent_id, idempotency_key, auth_ctx)
            .await
    }

    async fn lookup_invocation_status(
        &self,
        agent_id: &AgentId,
        idempotency_key: IdempotencyKey,
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
        component_revision: ComponentRevision,
        idempotency_key: IdempotencyKey,
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
fn card_management_permissions() -> Vec<PolymorphicPermissionPattern> {
    ["derive", "inspect", "install", "revoke"]
        .into_iter()
        .map(|verb| {
            parse_polymorphic_permission(&format!("card(*) @ * : {verb} : *"))
                .expect("valid card permission")
        })
        .collect()
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn scala_sdk_runs_the_shared_permission_card_scenario(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_scala")] tool_streaming_scala: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let store = Arc::new(CardStore::default());
    let delivery = Arc::new(Mutex::new(None));
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            wrap_worker_proxy: Some(Arc::new({
                let delivery = delivery.clone();
                move |inner| {
                    Arc::new(LocalCardDeliveryProxy {
                        inner,
                        delivery: delivery.clone(),
                    })
                }
            })),
            create_card_service: Some(Arc::new({
                let store = store.clone();
                move || {
                    Arc::new(CardStoreService {
                        store: store.clone(),
                    })
                }
            })),
            ..Default::default()
        },
    )
    .await?;
    connect_local_card_delivery(&delivery, &executor);
    let component = executor
        .component_dep(&context.default_environment_id, tool_streaming_scala)
        .unique()
        .update_agent_provision_config("ScalaPermissionCardAgent", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .extend(card_management_permissions());
        })
        .update_agent_provision_config("ScalaPermissionCardTarget", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .retain(|permission| !matches!(permission, PolymorphicPermissionPattern::Card(_)));
        })
        .store()
        .await?;

    // The SDK declares a template card parameter as
    // `permission-card { polymorphic: true }` and a concrete card parameter as
    // `polymorphic: false`.
    let target_type = component
        .metadata
        .agent_types()
        .iter()
        .find(|agent_type| agent_type.type_name.0 == "ScalaPermissionCardTarget")
        .ok_or_else(|| anyhow::anyhow!("missing ScalaPermissionCardTarget agent type"))?;
    let card_parameter_polymorphism = |method_name: &str| {
        target_type
            .methods
            .iter()
            .find(|method| method.name == method_name)
            .and_then(|method| method.input_schema.fields().first())
            .and_then(|field| match &field.schema {
                SchemaType::PermissionCard { spec, .. } => Some(spec.polymorphic),
                _ => None,
            })
    };
    assert_eq!(card_parameter_polymorphism("receiveTemplate"), Some(true));
    assert_eq!(card_parameter_polymorphism("receiveConcrete"), Some(false));

    let caller = agent_id!("ScalaPermissionCardAgent", "scenario");
    let root = component
        .metadata
        .agent_type_initial_permission_card(&caller.agent_type)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("missing initial card for ScalaPermissionCardAgent"))?;
    store.add(StoredCard::Polymorphic(root));
    executor.start_agent(&component.id, caller.clone()).await?;

    let observations = executor
        .invoke_and_await_agent(&component, &caller, "scenario", data_value!("target"))
        .await?
        .into_typed::<Vec<String>>()?;

    assert_eq!(
        observations,
        vec![
            // The initial card of the agent type is a template card.
            "root-kind=template",
            "derived-inspect=true",
            "child-parent=true",
            "child-expiry=true",
            // Deriving from the template card returns concrete children of it,
            // which the wallet lists as concrete cards once installed.
            "template-derive=true",
            "template-children=concrete,concrete,concrete",
            // A scope card expires with its earliest root card; a scope card
            // of a root card without expiry does not expire.
            "scope-expiry=true",
            "root-scope-expiry=None",
            // The target agent type has no card permissions of its own; a scope
            // card grants card:inspect only while the awaited call runs.
            "before=false",
            "per-call=true",
            "default=true",
            "cancelable=true",
            "after=false",
            "scope-usable=true",
            "denied-derive=NotPermitted",
            // The target's wallet is empty; during a call with a scope card it
            // lists that card as a scope entry, with the expiry of the
            // caller's scope card.
            "kinds-plain=",
            "kinds-lent=scope",
            "lent-expiry=true",
            "lent-no-expiry=None",
            "installed=true",
            "target-expiring=1",
            "target-inspect=true",
            "self-installed=true",
            "held-before=true",
            "self-held-kind=Some(concrete)",
            // The revoked card and the grandchild derived from it; the holder
            // loses the revoked card.
            "revoked=Some(Right(2))",
            "held-after=false",
            // Revoking the parent of the card installed on the target removes
            // the target's authority.
            // The parent and the card installed on the target; the self-held
            // branch was revoked before.
            "parent-revoked=Right(2)",
            "target-inspect-after-revoke=false",
            "target-expiring-after-revoke=0",
            "moved-child=true",
            "mismatch=Left(InstallRecipientMismatch)",
            "children-revoked=List(Right(1), Right(1), Right(1))",
        ]
    );

    // The scope card of the previous invocation is retired: holding and
    // releasing it is safe, but it carries no authority any more.
    let stale = executor
        .invoke_and_await_agent(&component, &caller, "staleScope", data_value!("target"))
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(
        stale,
        vec![
            "attach-failed=true",
            "stale-parent=Left(NotPermitted)",
            "released=true"
        ]
    );

    // Sending the template card moves the caller's handle, and the callee
    // decodes it as the declared template card type. An argument carries the
    // card but does not install it: the callee's wallet stays empty, and
    // without card:inspect of its own the callee cannot inspect the card.
    let sent = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "sendTemplate",
            data_value!("template-target"),
        )
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(
        sent,
        vec![
            "received-parents=0",
            "received-inspect=false",
            "sent-moved=true",
            "receiver-kinds=",
        ]
    );
    Ok(())
}
