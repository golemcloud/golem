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

//! Permission-card management through the Go SDK: a `CardIssuer` derives cards
//! from its wallet, inspects them, lends a scope card to awaited calls,
//! installs a card on a `CardReceiver` and revokes it, and the receiver reports
//! the authority it holds, and the kinds of its wallet cards, after each step.

use crate::Tracing;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_api_grpc::proto::golem::worker::UpdateMode;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    DeliverCardTransferRequest, deliver_card_transfer_response,
};
use golem_common::model::account::AccountId;
use golem_common::model::agent::{
    AgentInvocationMode, AgentTypeName, InvocationFreshnessDisposition, Principal,
};
use golem_common::model::card::{
    CardId, CardManagedByRuntimeDerived, PolymorphicPermissionPattern, ScopeCard, StoredCard,
    parse_polymorphic_permission,
};
use golem_common::model::component::ComponentRevision;
use golem_common::model::durable_stream::{
    DurableStreamReadRequest, StreamAttachmentControlRequest,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::worker::{AgentConfigEntryDto, ResolvedRevert, RevertWorkerTarget};
use golem_common::model::{
    AgentFingerprint, AgentId, AgentInvocationOutput, IdempotencyKey, InvocationStatus,
    OwnedAgentId, PromiseId,
};
use golem_common::schema::SchemaValue;
use golem_common::{agent_id, data_value};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::card::{
    CardRevokeResult, CardService, CardState, RuntimeCardCreation,
};
use golem_worker_executor::services::rpc::DurableStreamRemoteError;
use golem_worker_executor::services::worker_proxy::{
    InvocationRequestStream, InvocationResponseStream, WorkerProxy, WorkerProxyError,
};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock, RwLock};
use test_r::{inherit_test_dep, test, timeout};
use tonic::transport::Channel;
use tonic_tracing_opentelemetry::middleware::client::OtelGrpcService;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_go")]
    PrecompiledComponent
);

/// The cards the registry knows, and the ones it revoked.
#[derive(Default)]
struct CardRegistry {
    cards: RwLock<HashMap<CardId, StoredCard>>,
    revoked: RwLock<HashSet<CardId>>,
}

impl CardRegistry {
    fn add(&self, card: StoredCard) {
        self.cards.write().unwrap().insert(card.card_id(), card);
    }

    fn ancestors_including_self(&self, card_id: CardId) -> Vec<CardId> {
        let cards = self.cards.read().unwrap();
        let mut out = Vec::new();
        let mut pending = vec![card_id];
        while let Some(id) = pending.pop() {
            if out.contains(&id) {
                continue;
            }
            out.push(id);
            if let Some(card) = cards.get(&id) {
                pending.extend(card.parent_ids().iter().copied());
            }
        }
        out
    }
}

/// A card service over [`CardRegistry`] whose revocation cascades to every
/// card derived from the revoked one, as the registry service's does.
struct RegistryCardService(Arc<CardRegistry>);

#[async_trait]
impl CardService for RegistryCardService {
    async fn record_revoked_cards(&self, card_ids: &[CardId]) {
        self.0
            .revoked
            .write()
            .unwrap()
            .extend(card_ids.iter().copied());
    }

    async fn check_cards(
        &self,
        card_ids: Vec<CardId>,
    ) -> Result<HashMap<CardId, CardState>, WorkerExecutorError> {
        let cards = self.0.cards.read().unwrap();
        let revoked = self.0.revoked.read().unwrap();
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
        self.0.add(card.clone());
        Ok(RuntimeCardCreation::Created(Box::new(card)))
    }

    async fn revoke_card(&self, card_id: CardId) -> Result<CardRevokeResult, WorkerExecutorError> {
        if self.0.revoked.read().unwrap().contains(&card_id) {
            return Ok(CardRevokeResult::AlreadyRevoked(format!(
                "card {card_id} is already revoked"
            )));
        }
        let known = self
            .0
            .cards
            .read()
            .unwrap()
            .keys()
            .copied()
            .collect::<Vec<_>>();
        let cascade = known
            .into_iter()
            .filter(|id| self.0.ancestors_including_self(*id).contains(&card_id))
            .collect::<Vec<_>>();
        self.0
            .revoked
            .write()
            .unwrap()
            .extend(cascade.iter().copied());
        Ok(CardRevokeResult::Revoked(cascade))
    }

    async fn live_ancestor_ids_including_self(
        &self,
        card: &StoredCard,
    ) -> Result<Vec<CardId>, WorkerExecutorError> {
        let revoked = self.0.revoked.read().unwrap().clone();
        Ok(self
            .0
            .ancestors_including_self(card.card_id())
            .into_iter()
            .filter(|id| !revoked.contains(id))
            .collect())
    }
}

type ExecutorClient = WorkerExecutorClient<OtelGrpcService<Channel>>;

/// The executor's worker proxy, delivering installed cards to this executor
/// directly: the test has no worker service to route them through.
struct LocalCardDeliveryProxy {
    inner: Arc<dyn WorkerProxy>,
    executor: Arc<OnceLock<ExecutorClient>>,
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

    async fn invoke_agent_session(
        &self,
        request: InvocationRequestStream,
    ) -> Result<InvocationResponseStream, WorkerProxyError> {
        self.inner.invoke_agent_session(request).await
    }

    async fn control_durable_stream_attachment(
        &self,
        request: StreamAttachmentControlRequest,
        auth_ctx: &AuthCtx,
    ) -> Result<bool, DurableStreamRemoteError<WorkerProxyError>> {
        self.inner
            .control_durable_stream_attachment(request, auth_ctx)
            .await
    }

    async fn read_durable_stream_segment(
        &self,
        request: DurableStreamReadRequest,
        auth_ctx: &AuthCtx,
    ) -> Result<Vec<u8>, DurableStreamRemoteError<WorkerProxyError>> {
        self.inner
            .read_durable_stream_segment(request, auth_ctx)
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
        let internal = |message: String| {
            WorkerProxyError::InternalError(WorkerExecutorError::unknown(message))
        };
        let mut client = self
            .executor
            .get()
            .cloned()
            .ok_or_else(|| internal("the executor client is not connected".to_string()))?;
        let response = client
            .deliver_card_transfer(DeliverCardTransferRequest {
                target_agent_id: Some(target_agent_id.clone().into()),
                environment_id: Some(environment_id.into()),
                transfer_id: Some(transfer_id.into()),
                card: desert_rust::serialize_to_byte_vec(card)
                    .map_err(|error| internal(error.to_string()))?,
                auth_ctx: Some(AuthCtx::System.into()),
                source_card_id: Some(source_card_id.0.into()),
            })
            .await?
            .into_inner();
        match response.result {
            Some(deliver_card_transfer_response::Result::Success(_)) => Ok(()),
            Some(deliver_card_transfer_response::Result::Failure(error)) => {
                Err(WorkerProxyError::InternalError(
                    WorkerExecutorError::try_from(error).map_err(internal)?,
                ))
            }
            None => Err(internal("card delivery returned no result".to_string())),
        }
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
    ) -> Result<InvocationStatus, WorkerProxyError> {
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

fn card_grant(verb: &str) -> PolymorphicPermissionPattern {
    parse_polymorphic_permission(&format!("card(*) @ * : {verb} : *"))
        .expect("valid card permission")
}

/// The shared permission-card scenario. The issuer holds the four card verbs;
/// the receiver holds none, so every card authority it reports comes from a
/// card the issuer lent or installed.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn go_permission_cards_derive_lend_install_and_revoke(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let registry = Arc::new(CardRegistry::default());
    let executor_client = Arc::new(OnceLock::new());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            create_card_service: Some(Arc::new({
                let registry = registry.clone();
                move || Arc::new(RegistryCardService(registry.clone()))
            })),
            wrap_worker_proxy: Some(Arc::new({
                let executor_client = executor_client.clone();
                move |inner| {
                    Arc::new(LocalCardDeliveryProxy {
                        inner,
                        executor: executor_client.clone(),
                    })
                }
            })),
            ..Default::default()
        },
    )
    .await?;
    let _ = executor_client.set(executor.client.clone());

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .unique()
        .update_agent_provision_config("CardIssuer", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .extend(["derive", "inspect", "install", "revoke"].map(card_grant));
        })
        .update_agent_provision_config("CardReceiver", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .retain(|grant| !matches!(grant, PolymorphicPermissionPattern::Card(_)));
        })
        .store()
        .await?;
    for agent_type in ["CardIssuer", "CardReceiver"] {
        let card = component
            .metadata
            .agent_type_initial_permission_card(&AgentTypeName(agent_type.to_string()))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("{agent_type} has no initial card"))?;
        registry.add(StoredCard::Polymorphic(card));
    }

    let issuer = agent_id!("CardIssuer", "go-card-issuer");
    executor.start_agent(&component.id, issuer.clone()).await?;
    let report = executor
        .invoke_and_await_agent(
            &component,
            &issuer,
            "scenario",
            data_value!("go-card-receiver"),
        )
        .await?
        .into_typed::<Vec<String>>()?;

    assert_eq!(
        report,
        [
            "wallet-grants-inspect=true",
            "wallet-kinds=template",
            "from-wallet-parents=1",
            "from-wallet-grants=1/0/0/0",
            "from-wallet-grant-is-requested=true",
            "from-wallet-expires=false",
            "child-parent-is-from-wallet=true",
            "child-expires-within-the-hour=true",
            "receiver-before=not-permitted",
            "scope-parent-is-source=true",
            "receiver-with-scope=ok",
            "receiver-wallet-with-scope=scope@none,template",
            "receiver-with-default-scope=ok",
            "receiver-call-scope-over-default=not-permitted",
            "receiver-call-scope-over-empty-default=ok",
            "reflected-default-scope=ok",
            "reflected-call-scope-over-default=not-permitted",
            "dynamic-default-scope=ok",
            "dynamic-call-scope-over-default=ok",
            "receiver-trigger-and-schedule=not-permitted,not-permitted",
            "receiver-after-scope=not-permitted",
            "scope-card-kept=true",
            "install=ok",
            "child-moved-by-install=true",
            "receiver-after-install=ok",
            "receiver-wallet-after-install=concrete,template",
            "revoke=ok",
            "revoked-count=3",
            "from-wallet-moved-by-revoke=true",
            "receiver-after-revoke=not-permitted",
            "receiver-derive-from-wallet=not-permitted",
            "install-elsewhere=install-recipient-mismatch",
            "unknown-class=unknown-resource-class",
        ]
    );

    // A scope card is valid only in the invocation that derived it.
    let stale = executor
        .invoke_and_await_agent(&component, &issuer, "use-stale-scope", data_value!())
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(
        stale,
        [
            "attach=remote-denied",
            "derive-from=not-permitted",
            "released=true"
        ]
    );
    Ok(())
}
