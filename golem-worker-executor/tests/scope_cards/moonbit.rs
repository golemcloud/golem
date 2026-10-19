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

//! The shared permission-card scenario for the MoonBit SDK, run against the
//! `MoonBitPermissionAgent` of the MoonBit tool-streaming test component.

use super::*;
use chrono::DateTime;
use futures::future::BoxFuture;
use golem_api_grpc::proto::golem::worker::UpdateMode;
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentInvocationMode, InvocationFreshnessDisposition, Principal};
use golem_common::model::card::ScopeCard;
use golem_common::model::component::ComponentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::worker::{ResolvedRevert, RevertWorkerTarget};
use golem_common::model::{
    AgentFingerprint, AgentInvocationOutput, InvocationStatus, OwnedAgentId,
};
use golem_service_base::model::auth::AuthCtx;
use golem_worker_executor::services::worker_proxy::{WorkerProxy, WorkerProxyError};
use std::sync::Mutex;
use test_r::{test, timeout};

const AGENT_TYPE: &str = "MoonBitPermissionAgent";

type CardTransferDelivery = dyn Fn(
        AgentId,
        EnvironmentId,
        Uuid,
        CardId,
        StoredCard,
    ) -> BoxFuture<'static, Result<(), WorkerProxyError>>
    + Send
    + Sync;

/// Delivers permission-card transfers to the executor under test and forwards
/// every other call to the wrapped proxy.
struct LocalCardTransferProxy {
    inner: Arc<dyn WorkerProxy>,
    delivery: Arc<Mutex<Option<Arc<CardTransferDelivery>>>>,
}

#[async_trait]
impl WorkerProxy for LocalCardTransferProxy {
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

    async fn deliver_card_transfer(
        &self,
        target_agent_id: &AgentId,
        environment_id: EnvironmentId,
        transfer_id: Uuid,
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

fn connect_local_card_transfer_delivery(
    delivery: &Arc<Mutex<Option<Arc<CardTransferDelivery>>>>,
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

pub(super) async fn start_local_card_transfer_executor(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    authority: Arc<ScopeCardAuthority>,
) -> anyhow::Result<TestWorkerExecutor> {
    let delivery = Arc::new(Mutex::new(None));
    let executor = start_with_overrides(
        deps,
        context,
        TestExecutorOverrides {
            create_card_service: Some(Arc::new(move || {
                Arc::new(ScopeCardService {
                    authority: authority.clone(),
                })
            })),
            wrap_worker_proxy: Some(Arc::new({
                let delivery = delivery.clone();
                move |inner| {
                    Arc::new(LocalCardTransferProxy {
                        inner,
                        delivery: delivery.clone(),
                    })
                }
            })),
            ..Default::default()
        },
    )
    .await?;
    connect_local_card_transfer_delivery(&delivery, &executor);
    Ok(executor)
}

/// Starts an executor with the scope-card authority and local card-transfer
/// delivery, and stores the MoonBit component with card authority for the
/// permission agent.
async fn start_moonbit_permission_executor(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    authority: Arc<ScopeCardAuthority>,
    component: &PrecompiledComponent,
) -> anyhow::Result<(
    TestWorkerExecutor,
    golem_common::model::component::ComponentDto,
)> {
    let executor = start_local_card_transfer_executor(deps, context, authority).await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .update_agent_provision_config(AGENT_TYPE, |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .extend(scope_card_initial_permissions());
            config.initial_permissions.lower_bound.positive.extend(
                [
                    "card(*) @ * : install : *",
                    "card(*) @ * : revoke : *",
                    "agent(?agent) @ * : view : *",
                ]
                .map(|pattern| parse_polymorphic_permission(pattern).unwrap()),
            );
        })
        .store()
        .await?;
    Ok((executor, component))
}

async fn invoke(
    executor: &TestWorkerExecutor,
    component: &golem_common::model::component::ComponentDto,
    agent: &golem_common::model::agent::ParsedAgentId,
    method: &str,
    params: golem_common::schema::TypedSchemaValue,
) -> anyhow::Result<golem_test_framework::dsl::AgentResult> {
    executor
        .invoke_and_await_agent(component, agent, method, params)
        .await
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn moonbit_sdk_derives_inspects_and_rejects_permission_cards(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_moonbit")] tool_streaming_moonbit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let (executor, component) = start_moonbit_permission_executor(
        deps,
        &context,
        authority.clone(),
        tool_streaming_moonbit,
    )
    .await?;
    let caller = agent_id!(AGENT_TYPE, "derive-caller");
    let target = agent_id!(AGENT_TYPE, "derive-target");
    configure_scope_card_root(&authority, &component, &caller)?;
    executor.start_agent(&component.id, caller.clone()).await?;
    executor.start_agent(&component.id, target.clone()).await?;

    let observations = invoke(
        &executor,
        &component,
        &caller,
        "derive_chain",
        data_value!(),
    )
    .await?
    .into_typed::<Result<Vec<String>, String>>()?
    .map_err(|error| anyhow::anyhow!("derive_chain failed: {error}"))?;
    assert_eq!(
        observations,
        vec![
            "wallet-card-inspect-grants=1",
            "wallet-card-other-grants=0",
            "wallet-card-expires=false",
            "child-parent-is-card=true",
            "child-id-differs=true",
            "child-expires=true",
            "child-inspect-grants=1",
            "open-scope-expires=false",
            "derived-scope-parent=not-permitted",
            "root-child-parent-is-root=true",
            "root-child-expires=true",
            "calendar-lifespan=invalid-pattern",
            "zero-lifespan=invalid-pattern",
        ]
    );

    let negatives = invoke(
        &executor,
        &component,
        &caller,
        "negative_cases",
        data_value!(target.to_string()),
    )
    .await?
    .into_typed::<Result<Vec<String>, String>>()?
    .map_err(|error| anyhow::anyhow!("negative_cases failed: {error}"))?;
    assert_eq!(
        negatives,
        vec![
            "recipient-mismatch=install-recipient-mismatch",
            "mismatched-card-moved=true",
            "account-target=not-permitted",
            "invalid-verb=invalid-verb",
        ]
    );

    // The same read succeeds while the card is held, so the abort below comes
    // from the move and not from a missing root.
    let held = invoke(
        &executor,
        &component,
        &caller,
        "use_after_move",
        data_value!(false),
    )
    .await?
    .into_typed::<String>()?;
    assert_eq!(held, "root-present");
    let error = invoke(
        &executor,
        &component,
        &caller,
        "use_after_move",
        data_value!(true),
    )
    .await
    .expect_err("using a moved card must abort the invocation");
    // MoonBit's `abort` traps with `unreachable`; its message is not part of
    // the invocation error.
    assert!(
        format!("{error:#}").contains("wasm `unreachable` instruction executed"),
        "unexpected moved-card failure: {error:#}"
    );
    Ok(())
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn moonbit_sdk_lends_scope_cards_installs_and_revokes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_moonbit")] tool_streaming_moonbit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let (executor, component) = start_moonbit_permission_executor(
        deps,
        &context,
        authority.clone(),
        tool_streaming_moonbit,
    )
    .await?;
    let caller = agent_id!(AGENT_TYPE, "scope-caller");
    let scope_target_name = "scope-target";
    let scope_target = agent_id!(AGENT_TYPE, scope_target_name);
    let install_target = agent_id!(AGENT_TYPE, "install-target");
    configure_scope_card_root(&authority, &component, &caller)?;
    executor.start_agent(&component.id, caller.clone()).await?;
    executor
        .start_agent(&component.id, scope_target.clone())
        .await?;
    executor
        .start_agent(&component.id, install_target.clone())
        .await?;

    // A requested grant must be concrete, so the caller gets its own owner
    // path. The test executor stores every component under the application
    // `test-app` and the environment `test-env`; an agent name escapes `%`,
    // `/` and `?`.
    let caller_owner = format!(
        "{}/test-app/test-env/{}/{}",
        executor.account_email().as_str(),
        component.component_name.0,
        caller
            .to_string()
            .replace('%', "%25")
            .replace('/', "%2F")
            .replace('?', "%3F")
    );

    // Step 4: the callee sees the scope card only during awaited calls that
    // attach it, and loses it when the invocation ends. Only the scope card
    // lets the callee read the caller's oplog. The inspected `?agent`
    // template grant is not a valid request.
    let scope = invoke(
        &executor,
        &component,
        &caller,
        "scope_rpc",
        data_value!(scope_target_name, caller_owner.clone()),
    )
    .await?
    .into_typed::<Result<Vec<String>, String>>()?
    .map_err(|error| anyhow::anyhow!("scope_rpc failed: {error}"))?;
    assert_eq!(
        scope,
        vec![
            "template-grant=invalid-pattern",
            "per-call=present/true/2/readable",
            "default=present/true/2/readable",
            "without=absent/denied",
            "scope-kept=true",
        ]
    );
    let scope_target_cards = invoke(
        &executor,
        &component,
        &scope_target,
        "wallet_card_count",
        data_value!(),
    )
    .await?
    .into_typed::<u32>()?;
    assert_eq!(
        scope_target_cards, 1,
        "the scope card must not survive the call"
    );

    // Step 5: installing a card adds it to the target's wallet, and with it
    // the authority to read the caller's oplog.
    let caller_name = caller.to_string();
    let can_read_caller_oplog = async || -> anyhow::Result<bool> {
        invoke(
            &executor,
            &component,
            &install_target,
            "can_read_oplog",
            data_value!(caller_name.clone()),
        )
        .await?
        .into_typed::<bool>()
    };
    let wallet_count = async || -> anyhow::Result<u32> {
        invoke(
            &executor,
            &component,
            &install_target,
            "wallet_card_count",
            data_value!(),
        )
        .await?
        .into_typed::<u32>()
    };
    let before = wallet_count().await?;
    assert!(
        !can_read_caller_oplog().await?,
        "the target must not read the caller's oplog yet"
    );
    let moved = invoke(
        &executor,
        &component,
        &caller,
        "install_on",
        data_value!(install_target.to_string(), caller_owner.clone()),
    )
    .await?
    .into_typed::<Result<bool, String>>()?
    .map_err(|error| anyhow::anyhow!("install_on failed: {error}"))?;
    assert!(moved, "install must move the card");
    assert_eq!(
        wallet_count().await?,
        before + 1,
        "the installed card must reach the target"
    );
    assert!(
        can_read_caller_oplog().await?,
        "the installed card must let the target read the caller's oplog"
    );

    // Step 6: revoking the installed card removes it and its authority from
    // the target.
    let revoked_installed = invoke(
        &executor,
        &component,
        &install_target,
        "revoke_installed_card",
        data_value!(),
    )
    .await?
    .into_typed::<Result<Vec<String>, String>>()?
    .map_err(|error| anyhow::anyhow!("revoke_installed_card failed: {error}"))?;
    assert_eq!(revoked_installed, vec!["revoked=1", "card-moved=true"]);
    assert_eq!(
        wallet_count().await?,
        before,
        "the revoked card must leave the wallet"
    );
    assert!(
        !can_read_caller_oplog().await?,
        "the target must lose the authority of the revoked card"
    );

    // Revoking the wallet root returns the revoked-card count.
    let revoke_calls = authority.revoke_card_count();
    let revoked = invoke(&executor, &component, &caller, "revoke_root", data_value!())
        .await?
        .into_typed::<Result<u32, String>>()?
        .map_err(|error| anyhow::anyhow!("revoke_root failed: {error}"))?;
    assert_eq!(revoked, 1);
    assert_eq!(authority.revoke_card_count(), revoke_calls + 1);
    Ok(())
}

/// `derive-from-wallet` reports a request the wallet does not cover, and a
/// wallet that lost its derive authority, as permission errors.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn moonbit_sdk_derive_from_wallet_reports_permission_errors(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_moonbit")] tool_streaming_moonbit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let (executor, component) = start_moonbit_permission_executor(
        deps,
        &context,
        authority.clone(),
        tool_streaming_moonbit,
    )
    .await?;
    let caller = agent_id!(AGENT_TYPE, "wallet-errors-caller");
    configure_scope_card_root(&authority, &component, &caller)?;
    executor.start_agent(&component.id, caller.clone()).await?;

    let beyond_wallet = invoke(
        &executor,
        &component,
        &caller,
        "derive_beyond_wallet",
        data_value!(),
    )
    .await?
    .into_typed::<String>()?;
    assert_eq!(beyond_wallet, "lower-bound-too-broad");

    assert_eq!(
        invoke(
            &executor,
            &component,
            &caller,
            "derive_from_wallet_kind",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?,
        "derived"
    );
    let revoked = invoke(&executor, &component, &caller, "revoke_root", data_value!())
        .await?
        .into_typed::<Result<u32, String>>()?
        .map_err(|error| anyhow::anyhow!("revoke_root failed: {error}"))?;
    assert_eq!(
        revoked, 2,
        "revoking the root also revokes the card derived from the wallet"
    );
    let after_revoke = invoke(
        &executor,
        &component,
        &caller,
        "derive_from_wallet_kind",
        data_value!(),
    )
    .await?
    .into_typed::<String>()?;
    assert_eq!(after_revoke, "not-permitted");
    Ok(())
}
