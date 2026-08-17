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
use golem_common::model::card::{
    CardId, CardManagedByRuntimeDerived, PolymorphicPermissionPattern, StoredCard,
    parse_polymorphic_permission,
};
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry, PublicQueuedCardEvent};
use golem_common::model::{AgentStatus, IdempotencyKey, PromiseId};
use golem_common::{agent_id, data_value};
use golem_schema::model::CardId as SchemaCardId;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::card::{CardService, CardState};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

fn scope_card_initial_permissions() -> Vec<PolymorphicPermissionPattern> {
    vec![
        parse_polymorphic_permission("card(*) @ * : derive : *").unwrap(),
        parse_polymorphic_permission("card(*) @ * : inspect : *").unwrap(),
    ]
}

#[derive(Default)]
struct ScopeCardAuthority {
    revoked: AtomicBool,
    check_cards_count: AtomicUsize,
    root_card: RwLock<Option<StoredCard>>,
}

impl ScopeCardAuthority {
    fn set_root_card(&self, card: StoredCard) {
        *self.root_card.write().expect("root-card lock poisoned") = Some(card);
    }

    fn root_card(&self) -> Option<StoredCard> {
        self.root_card
            .read()
            .expect("root-card lock poisoned")
            .clone()
    }

    fn root_card_id(&self) -> Option<CardId> {
        self.root_card().map(|card| card.card_id())
    }

    fn revoke(&self) {
        self.revoked.store(true, Ordering::SeqCst);
    }

    fn reset_check_cards_count(&self) {
        self.check_cards_count.store(0, Ordering::SeqCst);
    }

    fn check_cards_count(&self) -> usize {
        self.check_cards_count.load(Ordering::SeqCst)
    }
}

struct ScopeCardService {
    authority: Arc<ScopeCardAuthority>,
}

#[async_trait]
impl CardService for ScopeCardService {
    async fn record_revoked_cards(&self, card_ids: &[CardId]) {
        if self
            .authority
            .root_card_id()
            .is_some_and(|root_card_id| card_ids.contains(&root_card_id))
        {
            self.authority.revoke();
        }
    }

    async fn create_runtime_card(
        &self,
        card: StoredCard,
        _provenance: CardManagedByRuntimeDerived,
    ) -> Result<StoredCard, WorkerExecutorError> {
        Ok(card)
    }

    async fn check_cards(
        &self,
        card_ids: Vec<CardId>,
    ) -> Result<HashMap<CardId, CardState>, WorkerExecutorError> {
        self.authority
            .check_cards_count
            .fetch_add(1, Ordering::SeqCst);
        let root_card = self.authority.root_card().ok_or_else(|| {
            WorkerExecutorError::runtime("scope-card test root was not configured")
        })?;
        let root_card_id = root_card.card_id();
        let root_is_revoked = self.authority.revoked.load(Ordering::SeqCst);
        Ok(card_ids
            .into_iter()
            .map(|card_id| {
                let state = if card_id == root_card_id {
                    if root_is_revoked {
                        CardState::Revoked
                    } else {
                        CardState::Live(Box::new(root_card.clone()))
                    }
                } else {
                    CardState::Unknown
                };
                (card_id, state)
            })
            .collect())
    }
}

async fn start_scope_card_executor(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    authority: Arc<ScopeCardAuthority>,
) -> anyhow::Result<TestWorkerExecutor> {
    start_with_overrides(
        deps,
        context,
        TestExecutorOverrides {
            create_card_service: Some(Arc::new(move || {
                Arc::new(ScopeCardService {
                    authority: authority.clone(),
                })
            })),
            ..Default::default()
        },
    )
    .await
}

fn configure_scope_card_root(
    authority: &ScopeCardAuthority,
    component: &golem_common::model::component::ComponentDto,
    agent_id: &golem_common::model::agent::ParsedAgentId,
) -> anyhow::Result<CardId> {
    let card = component
        .metadata
        .agent_type_initial_permission_card(&agent_id.agent_type)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("missing scope-card test initial card"))?;
    let card = StoredCard::Polymorphic(card);
    let card_id = card.card_id();
    authority.set_root_card(card);
    Ok(card_id)
}

async fn assert_scope_absent(
    executor: &TestWorkerExecutor,
    component: &golem_common::model::component::ComponentDto,
    target: &golem_common::model::agent::ParsedAgentId,
    scope_card_id: SchemaCardId,
) -> anyhow::Result<()> {
    let present = executor
        .invoke_and_await_agent(component, target, "has_card", data_value!(scope_card_id))
        .await?
        .into_typed::<bool>()?;
    assert!(!present, "scope card must not survive invocation end");
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn scope_cards_are_delivered_by_both_await_variants_and_removed_at_end(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .update_agent_provision_config("ScopeCardAgent", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .extend(scope_card_initial_permissions());
        })
        .store()
        .await?;
    let caller = agent_id!("ScopeCardAgent", "await-caller");
    let target_name = "await-target";
    let target = agent_id!("ScopeCardAgent", target_name);
    configure_scope_card_root(&authority, &component, &caller)?;
    let caller_worker = executor.start_agent(&component.id, caller.clone()).await?;
    let target_worker = executor.start_agent(&component.id, target.clone()).await?;

    let (present, parent_matches, inspect_matches, scope_card_id) = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "invoke_and_await_scope",
            data_value!(target_name),
        )
        .await?
        .into_typed::<(bool, bool, bool, SchemaCardId)>()?;
    assert_eq!(
        (present, parent_matches, inspect_matches),
        (true, true, true)
    );
    assert_scope_absent(&executor, &component, &target, scope_card_id).await?;

    let (present, parent_matches, inspect_matches, scope_card_id) = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "async_invoke_and_await_scope",
            data_value!(target_name),
        )
        .await?
        .into_typed::<(bool, bool, bool, SchemaCardId)>()?;
    assert_eq!(
        (present, parent_matches, inspect_matches),
        (true, true, true)
    );
    assert_scope_absent(&executor, &component, &target, scope_card_id).await?;

    executor.check_oplog_is_queryable(&caller_worker).await?;
    executor.check_oplog_is_queryable(&target_worker).await?;
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn scope_cards_reject_non_await_and_persistent_arguments(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .update_agent_provision_config("ScopeCardAgent", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .extend(scope_card_initial_permissions());
        })
        .store()
        .await?;
    let caller = agent_id!("ScopeCardAgent", "rejection-caller");
    configure_scope_card_root(&authority, &component, &caller)?;
    executor.start_agent(&component.id, caller.clone()).await?;

    let invoke_denied = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "invoke_scope_is_denied",
            data_value!("rejection-target"),
        )
        .await?
        .into_typed::<bool>()?;
    assert!(invoke_denied, "fire-and-forget scope card should be denied");

    let persistent_denied = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "persistent_scope_is_denied",
            data_value!("rejection-target"),
        )
        .await?
        .into_typed::<bool>()?;
    assert!(
        persistent_denied,
        "a persistent card must not be accepted as the scope-card argument"
    );

    for (method, caller_name) in [
        ("schedule_scope", "schedule-caller"),
        ("schedule_cancelable_scope", "cancelable-schedule-caller"),
    ] {
        let schedule_caller = agent_id!("ScopeCardAgent", caller_name);
        executor
            .start_agent(&component.id, schedule_caller.clone())
            .await?;
        let error = executor
            .invoke_and_await_agent(
                &component,
                &schedule_caller,
                method,
                data_value!("rejection-target"),
            )
            .await
            .expect_err("scheduled scope-card invocation should fail");
        assert!(
            error.to_string().contains("does not accept scope cards"),
            "unexpected {method} error: {error:#}"
        );
    }

    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn scope_card_revocation_removes_authority_at_the_next_boundary(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .update_agent_provision_config("ScopeCardAgent", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .extend(scope_card_initial_permissions());
        })
        .store()
        .await?;
    let caller = agent_id!("ScopeCardAgent", "revocation-caller");
    let target_name = "revocation-target";
    let target = agent_id!("ScopeCardAgent", target_name);
    let root_card_id = configure_scope_card_root(&authority, &component, &caller)?;
    let target_worker = executor.start_agent(&component.id, target.clone()).await?;
    executor.start_agent(&component.id, caller.clone()).await?;
    let release = executor
        .invoke_and_await_agent(&component, &target, "create_release_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let key = IdempotencyKey::fresh();
    let params = data_value!(target_name, release.clone());

    executor
        .invoke_agent_with_key(
            &component,
            &caller,
            &key,
            "invoke_scope_after_promise",
            params.clone(),
        )
        .await?;
    executor
        .wait_for_status(
            &target_worker,
            AgentStatus::Suspended,
            Duration::from_secs(10),
        )
        .await?;
    authority.revoke();
    executor.complete_promise(&release, vec![1]).await?;

    let (before, after, scope_card_id) = executor
        .invoke_and_await_agent_with_key(
            &component,
            &caller,
            &key,
            "invoke_scope_after_promise",
            params,
        )
        .await?
        .into_typed::<(bool, bool, SchemaCardId)>()?;
    assert_eq!((before, after), (true, false));
    assert_scope_absent(&executor, &component, &target, scope_card_id).await?;

    let target_oplog = executor
        .get_oplog(&target_worker, OplogIndex::INITIAL)
        .await?;
    assert!(target_oplog.iter().any(|entry| matches!(
        &entry.entry,
        PublicOplogEntry::CardRevokedCascade(params)
            if params.revoked_card_ids.contains(&root_card_id)
    )));
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn scope_card_delivery_and_cleanup_survive_crash_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .update_agent_provision_config("ScopeCardAgent", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .extend(scope_card_initial_permissions());
        })
        .store()
        .await?;
    let caller = agent_id!("ScopeCardAgent", "replay-caller");
    let target_name = "replay-target";
    let target = agent_id!("ScopeCardAgent", target_name);
    configure_scope_card_root(&authority, &component, &caller)?;
    let caller_worker = executor.start_agent(&component.id, caller.clone()).await?;
    let target_worker = executor.start_agent(&component.id, target.clone()).await?;
    let release = executor
        .invoke_and_await_agent(&component, &target, "create_release_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let key = IdempotencyKey::fresh();
    let params = data_value!(target_name, release.clone());

    executor
        .invoke_agent_with_key(
            &component,
            &caller,
            &key,
            "invoke_scope_after_promise",
            params.clone(),
        )
        .await?;
    executor
        .wait_for_status(
            &target_worker,
            AgentStatus::Suspended,
            Duration::from_secs(10),
        )
        .await?;
    executor.check_oplog_is_queryable(&caller_worker).await?;
    executor.check_oplog_is_queryable(&target_worker).await?;
    drop(executor);

    let executor = start_scope_card_executor(deps, &context, authority).await?;
    executor.complete_promise(&release, vec![1]).await?;
    let (before, after, scope_card_id) = executor
        .invoke_and_await_agent_with_key(
            &component,
            &caller,
            &key,
            "invoke_scope_after_promise",
            params,
        )
        .await?
        .into_typed::<(bool, bool, SchemaCardId)>()?;
    assert_eq!((before, after), (true, true));
    assert_scope_absent(&executor, &component, &target, scope_card_id).await?;
    executor.check_oplog_is_queryable(&caller_worker).await?;
    executor.check_oplog_is_queryable(&target_worker).await?;
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn queued_wallet_revocation_updates_cached_authorization_at_boundary(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .update_agent_provision_config("ScopeCardAgent", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .extend(scope_card_initial_permissions());
        })
        .store()
        .await?;
    let caller = agent_id!("ScopeCardAgent", "wallet-revocation-caller");
    let root_card_id = configure_scope_card_root(&authority, &component, &caller)?;
    let caller_worker = executor.start_agent(&component.id, caller.clone()).await?;

    assert!(
        !executor
            .invoke_and_await_agent(
                &component,
                &caller,
                "derive_from_wallet_is_denied",
                data_value!(),
            )
            .await?
            .into_typed::<bool>()?,
        "the cached surface must initially authorize wallet derivation"
    );

    authority.revoke();
    executor
        .queue_uncommitted_card_revocation(&caller_worker, root_card_id)
        .await?;

    let error = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "derive_from_wallet_is_denied",
            data_value!(),
        )
        .await
        .expect_err("the queued revocation must deny wallet derivation");
    assert!(
        error.to_string().contains("card:derive is not permitted"),
        "unexpected authorization error: {error:#}"
    );

    let oplog = executor
        .get_oplog(&caller_worker, OplogIndex::INITIAL)
        .await?;
    assert!(oplog.iter().any(|entry| matches!(
        &entry.entry,
        PublicOplogEntry::CardRevokedCascade(params)
            if params.revoked_card_ids.contains(&root_card_id)
    )));
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn queued_wallet_revocation_precedes_the_next_accessor_start(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .update_agent_provision_config("ScopeCardAgent", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .extend(scope_card_initial_permissions());
        })
        .store()
        .await?;
    let caller = agent_id!("ScopeCardAgent", "accessor-revocation-caller");
    let root_card_id = configure_scope_card_root(&authority, &component, &caller)?;
    let caller_worker = executor.start_agent(&component.id, caller.clone()).await?;
    let release = executor
        .invoke_and_await_agent(&component, &caller, "create_release_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    executor.complete_promise(&release, vec![1]).await?;

    authority.revoke();
    executor
        .queue_uncommitted_card_revocation(&caller_worker, root_card_id)
        .await?;
    assert!(
        executor
            .invoke_and_await_agent(&component, &caller, "await_release", data_value!(release))
            .await?
            .into_typed::<bool>()?
    );

    let oplog = executor
        .get_oplog(&caller_worker, OplogIndex::INITIAL)
        .await?;
    let queued_index = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::CardEventQueued(params)
                if matches!(
                    &params.event,
                    PublicQueuedCardEvent::Revoke(event) if event.card_id == root_card_id
                ) =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("the revocation must have a durable arrival record");
    let cascade_index = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::CardRevokedCascade(params)
                if params.revoked_card_ids.contains(&root_card_id) =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("the accessor boundary must apply the queued revocation");
    let next_start_index = oplog
        .iter()
        .find_map(|entry| {
            (entry.oplog_index > queued_index && matches!(&entry.entry, PublicOplogEntry::Start(_)))
                .then_some(entry.oplog_index)
        })
        .expect("the promise await must append a durable start");
    assert!(
        queued_index < cascade_index && cascade_index < next_start_index,
        "expected queued revocation {queued_index} < cascade {cascade_index} < accessor start {next_start_index}"
    );
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn accessor_boundary_preserves_install_before_revoke_order(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .update_agent_provision_config("ScopeCardAgent", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .extend(scope_card_initial_permissions());
        })
        .store()
        .await?;
    let caller = agent_id!("ScopeCardAgent", "accessor-install-revoke-order");
    configure_scope_card_root(&authority, &component, &caller)?;
    let caller_worker = executor.start_agent(&component.id, caller.clone()).await?;
    let release = executor
        .invoke_and_await_agent(&component, &caller, "create_release_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    executor.complete_promise(&release, vec![1]).await?;

    let root_card = authority.root_card().expect("root card must be configured");
    let root_card_id = root_card.card_id();
    executor
        .queue_card_install(&caller_worker, root_card)
        .await?;
    executor
        .queue_card_revocation(&caller_worker, root_card_id)
        .await?;

    assert!(
        executor
            .invoke_and_await_agent(&component, &caller, "await_release", data_value!(release))
            .await?
            .into_typed::<bool>()?
    );

    let wallet_card_count = executor
        .invoke_and_await_agent(&component, &caller, "wallet_card_count", data_value!())
        .await?
        .into_typed::<u32>()?;
    assert_eq!(
        wallet_card_count, 0,
        "an install followed by a revoke must leave the card revoked"
    );
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn replayed_wallet_authorization_uses_pinned_cards_until_live_transition(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .update_agent_provision_config("ScopeCardAgent", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .extend(scope_card_initial_permissions());
        })
        .store()
        .await?;
    let caller = agent_id!("ScopeCardAgent", "wallet-replay-caller");
    configure_scope_card_root(&authority, &component, &caller)?;
    let caller_worker = executor.start_agent(&component.id, caller.clone()).await?;
    let release = executor
        .invoke_and_await_agent(&component, &caller, "create_release_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let key = IdempotencyKey::fresh();
    let params = data_value!(release.clone());

    executor
        .invoke_agent_with_key(
            &component,
            &caller,
            &key,
            "derive_before_promise",
            params.clone(),
        )
        .await?;
    executor
        .wait_for_status(
            &caller_worker,
            AgentStatus::Suspended,
            Duration::from_secs(10),
        )
        .await?;
    drop(executor);

    authority.revoke();
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    authority.reset_check_cards_count();
    executor.complete_promise(&release, vec![1]).await?;
    let _derived_card_id = executor
        .invoke_and_await_agent_with_key(&component, &caller, &key, "derive_before_promise", params)
        .await?
        .into_typed::<SchemaCardId>()?;

    assert_eq!(
        authority.check_cards_count(),
        1,
        "replay must not query registry liveness; only the replay-to-live transition may do so"
    );
    let error = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "derive_from_wallet_is_denied",
            data_value!(),
        )
        .await
        .expect_err("the revoked card must stop authorizing new live invocations");
    assert!(
        error.to_string().contains("card:derive is not permitted"),
        "unexpected authorization error: {error:#}"
    );
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn repeated_authorization_does_not_revalidate_wallet_or_scope_roots(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .update_agent_provision_config("ScopeCardAgent", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .extend(scope_card_initial_permissions());
        })
        .store()
        .await?;
    let caller = agent_id!("ScopeCardAgent", "authorization-cost-caller");
    let target_name = "authorization-cost-target";
    let target = agent_id!("ScopeCardAgent", target_name);
    configure_scope_card_root(&authority, &component, &caller)?;
    executor.start_agent(&component.id, caller.clone()).await?;
    executor.start_agent(&component.id, target).await?;

    authority.reset_check_cards_count();
    let wallet_card_count = executor
        .invoke_and_await_agent(&component, &caller, "wallet_card_count", data_value!())
        .await?
        .into_typed::<u32>()?;
    assert_eq!(wallet_card_count, 1);
    let introspection_checks = authority.check_cards_count();
    assert!(introspection_checks > 0);

    authority.reset_check_cards_count();
    assert!(
        executor
            .invoke_and_await_agent(
                &component,
                &caller,
                "authorize_repeatedly",
                data_value!(5u32),
            )
            .await?
            .into_typed::<bool>()?
    );
    assert_eq!(
        authority.check_cards_count(),
        introspection_checks,
        "repeated inspect and derive authorization must add no full-wallet liveness checks"
    );

    authority.reset_check_cards_count();
    assert!(
        executor
            .invoke_and_await_agent(
                &component,
                &caller,
                "invoke_and_await_repeated_scope_inspection",
                data_value!(target_name, 0u32),
            )
            .await?
            .into_typed::<bool>()?
    );
    let scope_admission_checks = authority.check_cards_count();
    assert!(scope_admission_checks > 0);

    authority.reset_check_cards_count();
    assert!(
        executor
            .invoke_and_await_agent(
                &component,
                &caller,
                "invoke_and_await_repeated_scope_inspection",
                data_value!(target_name, 5u32),
            )
            .await?
            .into_typed::<bool>()?
    );
    assert_eq!(
        authority.check_cards_count(),
        scope_admission_checks,
        "repeated scoped authorization must add no root-card liveness checks after admission"
    );
    Ok(())
}
