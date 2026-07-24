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
use chrono::DateTime;
use golem_common::model::card::{
    Card, CardId, CardManagedByRuntimeDerived, StoredCard, parse_permission_fields,
};
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::model::{AgentStatus, IdempotencyKey, PromiseId};
use golem_common::{agent_id, data_value};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::card::{CardService, CardState};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

const SCOPE_CARD_ROOT_ID: CardId = CardId(uuid::uuid!("62502f9f-2a66-45f7-9f6f-710d10387c30"));

fn scope_card_root() -> StoredCard {
    StoredCard::Concrete(Card {
        card_id: SCOPE_CARD_ROOT_ID,
        parent_ids: Vec::new(),
        lower_positive: vec![
            parse_permission_fields("card", "*", "*", "derive", "*").unwrap(),
            parse_permission_fields("card", "*", "*", "inspect", "*").unwrap(),
        ],
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: DateTime::from_timestamp_nanos(0),
        expires_at: None,
        system_card: false,
        managed_by: None,
    })
}

#[derive(Default)]
struct ScopeCardAuthority {
    revoked: AtomicBool,
}

impl ScopeCardAuthority {
    fn revoke(&self) {
        self.revoked.store(true, Ordering::SeqCst);
    }
}

struct ScopeCardService {
    authority: Arc<ScopeCardAuthority>,
}

#[async_trait]
impl CardService for ScopeCardService {
    async fn record_revoked_cards(&self, card_ids: &[CardId]) {
        if card_ids.contains(&SCOPE_CARD_ROOT_ID) {
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
        Ok(card_ids
            .into_iter()
            .map(|card_id| {
                let state = if card_id == SCOPE_CARD_ROOT_ID {
                    if self.authority.revoked.load(Ordering::SeqCst) {
                        CardState::Revoked
                    } else {
                        CardState::Live(Box::new(scope_card_root()))
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

fn root_bits() -> (u64, u64) {
    SCOPE_CARD_ROOT_ID.0.as_u64_pair()
}

async fn install_scope_parent(
    executor: &TestWorkerExecutor,
    component: &golem_common::model::component::ComponentDto,
    caller: &golem_common::model::agent::ParsedAgentId,
) -> anyhow::Result<()> {
    let (high_bits, low_bits) = root_bits();
    let installed = executor
        .invoke_and_await_agent(
            component,
            caller,
            "install_parent",
            data_value!(high_bits, low_bits),
        )
        .await?
        .into_typed::<bool>()?;
    assert!(installed, "scope-card parent should install");
    Ok(())
}

async fn assert_scope_absent(
    executor: &TestWorkerExecutor,
    component: &golem_common::model::component::ComponentDto,
    target: &golem_common::model::agent::ParsedAgentId,
) -> anyhow::Result<()> {
    let (high_bits, low_bits) = root_bits();
    let present = executor
        .invoke_and_await_agent(
            component,
            target,
            "has_scope",
            data_value!(high_bits, low_bits),
        )
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
    let executor =
        start_scope_card_executor(deps, &context, Arc::new(ScopeCardAuthority::default())).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let caller = agent_id!("ScopeCardAgent", "await-caller");
    let target_name = "await-target";
    let target = agent_id!("ScopeCardAgent", target_name);
    let caller_worker = executor.start_agent(&component.id, caller.clone()).await?;
    let target_worker = executor.start_agent(&component.id, target.clone()).await?;
    install_scope_parent(&executor, &component, &caller).await?;
    let (high_bits, low_bits) = root_bits();

    let observation = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "invoke_and_await_scope",
            data_value!(target_name, high_bits, low_bits),
        )
        .await?
        .into_typed::<(bool, bool, bool)>()?;
    assert_eq!(observation, (true, true, true));
    assert_scope_absent(&executor, &component, &target).await?;

    let observation = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "async_invoke_and_await_scope",
            data_value!(target_name, high_bits, low_bits),
        )
        .await?
        .into_typed::<(bool, bool, bool)>()?;
    assert_eq!(observation, (true, true, true));
    assert_scope_absent(&executor, &component, &target).await?;

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
    let executor =
        start_scope_card_executor(deps, &context, Arc::new(ScopeCardAuthority::default())).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let caller = agent_id!("ScopeCardAgent", "rejection-caller");
    executor.start_agent(&component.id, caller.clone()).await?;
    install_scope_parent(&executor, &component, &caller).await?;
    let (high_bits, low_bits) = root_bits();

    let invoke_denied = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "invoke_scope_is_denied",
            data_value!("rejection-target", high_bits, low_bits),
        )
        .await?
        .into_typed::<bool>()?;
    assert!(invoke_denied, "fire-and-forget scope card should be denied");

    let persistent_denied = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "persistent_scope_is_denied",
            data_value!("rejection-target", high_bits, low_bits),
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
        install_scope_parent(&executor, &component, &schedule_caller).await?;
        let error = executor
            .invoke_and_await_agent(
                &component,
                &schedule_caller,
                method,
                data_value!("rejection-target", high_bits, low_bits),
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
        .store()
        .await?;
    let caller = agent_id!("ScopeCardAgent", "revocation-caller");
    let target_name = "revocation-target";
    let target = agent_id!("ScopeCardAgent", target_name);
    let target_worker = executor.start_agent(&component.id, target.clone()).await?;
    executor.start_agent(&component.id, caller.clone()).await?;
    install_scope_parent(&executor, &component, &caller).await?;
    let release = executor
        .invoke_and_await_agent(&component, &target, "create_release_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let (high_bits, low_bits) = root_bits();
    let key = IdempotencyKey::fresh();
    let params = data_value!(target_name, high_bits, low_bits, release.clone());

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

    let observation = executor
        .invoke_and_await_agent_with_key(
            &component,
            &caller,
            &key,
            "invoke_scope_after_promise",
            params,
        )
        .await?
        .into_typed::<(bool, bool)>()?;
    assert_eq!(observation, (true, false));
    assert_scope_absent(&executor, &component, &target).await?;

    let target_oplog = executor
        .get_oplog(&target_worker, OplogIndex::INITIAL)
        .await?;
    assert!(target_oplog.iter().any(|entry| matches!(
        &entry.entry,
        PublicOplogEntry::CardRevokedCascade(params)
            if params.revoked_card_ids.contains(&SCOPE_CARD_ROOT_ID)
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
        .store()
        .await?;
    let caller = agent_id!("ScopeCardAgent", "replay-caller");
    let target_name = "replay-target";
    let target = agent_id!("ScopeCardAgent", target_name);
    let caller_worker = executor.start_agent(&component.id, caller.clone()).await?;
    let target_worker = executor.start_agent(&component.id, target.clone()).await?;
    install_scope_parent(&executor, &component, &caller).await?;
    let release = executor
        .invoke_and_await_agent(&component, &target, "create_release_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let (high_bits, low_bits) = root_bits();
    let key = IdempotencyKey::fresh();
    let params = data_value!(target_name, high_bits, low_bits, release.clone());

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
    let observation = executor
        .invoke_and_await_agent_with_key(
            &component,
            &caller,
            &key,
            "invoke_scope_after_promise",
            params,
        )
        .await?
        .into_typed::<(bool, bool)>()?;
    assert_eq!(observation, (true, true));
    assert_scope_absent(&executor, &component, &target).await?;
    executor.check_oplog_is_queryable(&caller_worker).await?;
    executor.check_oplog_is_queryable(&target_worker).await?;
    Ok(())
}
