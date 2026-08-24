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
use crate::durability::assert_snapshot_recovery_loaded;
use crate::tool_discovery::deployment_state;
use async_trait::async_trait;
use chrono::Utc;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::agent_secret::{
    AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
};
use golem_common::model::card::{
    CardId, CardManagedByRuntimeDerived, PolymorphicCard, PolymorphicPermissionPattern, StoredCard,
    parse_polymorphic_permission,
};
use golem_common::model::component::ComponentId;
use golem_common::model::oplog::types::{SerializableP3SocketErrorCode, SerializableRpcError};
use golem_common::model::oplog::{
    HostRequestGolemRpcActivate, HostResponse, HostResponseGolemRpcActivate,
    HostResponseP3SocketsConnect, OplogIndex, PublicOplogEntry, PublicQueuedCardEvent,
};
use golem_common::model::worker::AgentConfigEntryDto;
use golem_common::model::{AgentStatus, IdempotencyKey, PromiseId};
use golem_common::schema::{FromSchema, SchemaGraph, SchemaType, SchemaValue};
use golem_common::{agent_id, data_value};
use golem_schema::model::CardId as SchemaCardId;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::agent_secret::AgentSecret;
use golem_test_framework::components::rdb::docker_ignite::DockerIgniteRdb;
use golem_test_framework::components::rdb::docker_mysql::DockerMysqlRdb;
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::card::{CardRevokeResult, CardService, CardState};
use golem_worker_executor::services::golem_config::SnapshotPolicy;
use golem_worker_executor_test_utils::agent_deployments_service::TestEnvironmentStateService;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, registry_test_card, start_with_overrides,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use test_r::{inherit_test_dep, test, test_dep, timeout};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use uuid::Uuid;

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

#[test_dep(scope = Shared)]
async fn permission_postgres() -> DockerPostgresRdb {
    DockerPostgresRdb::new(&Uuid::new_v4().to_string(), false).await
}

#[test_dep(scope = Shared)]
async fn permission_mysql() -> DockerMysqlRdb {
    DockerMysqlRdb::new(&Uuid::new_v4().to_string()).await
}

#[test_dep(scope = Shared)]
async fn permission_ignite() -> DockerIgniteRdb {
    DockerIgniteRdb::new().await
}

fn scope_card_initial_permissions() -> Vec<PolymorphicPermissionPattern> {
    vec![
        parse_polymorphic_permission("card(*) @ * : derive : *").unwrap(),
        parse_polymorphic_permission("card(*) @ * : inspect : *").unwrap(),
    ]
}

fn p3_tcp_permission(port: u16) -> PolymorphicPermissionPattern {
    parse_polymorphic_permission(&format!("network() @ * : connect : 127.0.0.1:{port}"))
        .expect("valid test network permission")
}

fn filesystem_permission(verb: &str, path: &str) -> PolymorphicPermissionPattern {
    parse_polymorphic_permission(&format!("filesystem(?agent) @ * : {verb} : {path}"))
        .expect("valid test filesystem permission")
}

fn secret_permission(verb: &str, path: &str) -> PolymorphicPermissionPattern {
    parse_polymorphic_permission(&format!("secret(?env) @ * : {verb} : {path}"))
        .expect("valid test secret permission")
}

fn config_permission(path: &str) -> PolymorphicPermissionPattern {
    parse_polymorphic_permission(&format!("config(?agent) @ * : read : {path}"))
        .expect("valid test config permission")
}

fn env_permission(name: &str) -> PolymorphicPermissionPattern {
    parse_polymorphic_permission(&format!("env(?agent) @ * : read : {name}"))
        .expect("valid test environment permission")
}

fn kv_permission(verb: &str, store: &str, key: &str) -> PolymorphicPermissionPattern {
    parse_polymorphic_permission(&format!("kv(?env) @ * : {verb} : {store}.{key}"))
        .expect("valid test key-value permission")
}

fn blob_permission(verb: &str, resource: &str) -> PolymorphicPermissionPattern {
    parse_polymorphic_permission(&format!("blob(?env) @ * : {verb} : {resource}"))
        .expect("valid test blob permission")
}

fn normalized_error_contains(error: &str, expected: &str) -> bool {
    error
        .chars()
        .filter(|char| char.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
        .contains(expected)
}

fn is_not_permitted(error: &str) -> bool {
    normalized_error_contains(error, "notpermitted")
}

#[derive(Default)]
struct ScopeCardAuthority {
    revoked: AtomicBool,
    check_cards_count: AtomicUsize,
    revoke_card_count: AtomicUsize,
    root_card: RwLock<Option<StoredCard>>,
    additional_cards: RwLock<HashMap<CardId, StoredCard>>,
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

    fn add_card(&self, card: StoredCard) {
        self.additional_cards
            .write()
            .expect("additional-card lock poisoned")
            .insert(card.card_id(), card);
    }

    fn card(&self, card_id: CardId) -> Option<StoredCard> {
        self.additional_cards
            .read()
            .expect("additional-card lock poisoned")
            .get(&card_id)
            .cloned()
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

    fn revoke_card_count(&self) -> usize {
        self.revoke_card_count.load(Ordering::SeqCst)
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

    async fn revoke_card(&self, card_id: CardId) -> Result<CardRevokeResult, WorkerExecutorError> {
        self.authority
            .revoke_card_count
            .fetch_add(1, Ordering::SeqCst);
        Ok(CardRevokeResult::Revoked(vec![card_id]))
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
                } else if let Some(card) = self.authority.card(card_id) {
                    CardState::Live(Box::new(card))
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
    let caller_oplog = executor
        .get_oplog(&caller_worker, OplogIndex::INITIAL)
        .await?;
    let activation_start = caller_oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name == "golem::rpc::wasm-rpc::activate" =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("the first admitted RPC call must durably record target activation");
    let activation_response = caller_oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::End(params) if params.start_index == activation_start => {
                params.response.clone()
            }
            _ => None,
        })
        .expect("the activation decision must complete before RPC dispatch");
    let activation_response =
        HostResponseGolemRpcActivate::from_value(activation_response.value())?;
    let persisted_fingerprint = activation_response
        .result
        .expect("the admitted activation must persist its target fingerprint");
    assert!(!persisted_fingerprint.0.is_nil());
    executor.check_oplog_is_queryable(&caller_worker).await?;
    executor.check_oplog_is_queryable(&target_worker).await?;
    drop(executor);

    let executor = start_scope_card_executor(deps, &context, authority).await?;
    executor
        .wait_for_status(
            &target_worker,
            AgentStatus::Suspended,
            Duration::from_secs(10),
        )
        .await?;
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
    let replayed_caller_oplog = executor
        .get_oplog(&caller_worker, OplogIndex::INITIAL)
        .await?;
    let invocation_start = replayed_caller_oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name == "golem::rpc::wasm-rpc::invoke_and_await" =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("the admitted RPC call must have a durable invocation Start");
    assert!(
        activation_start < invocation_start,
        "target activation must be durable before the invocation Start"
    );
    assert_eq!(
        replayed_caller_oplog
            .iter()
            .filter(|entry| matches!(
                &entry.entry,
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem::rpc::wasm-rpc::activate"
            ))
            .count(),
        1,
        "restart must replay the recorded activation and validate its fingerprint without appending a duplicate"
    );
    executor.check_oplog_is_queryable(&caller_worker).await?;
    executor.check_oplog_is_queryable(&target_worker).await?;
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn outbound_rpc_denial_replays_without_activating_the_target(
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
        .without_default_host_permissions("GolemHostApi")
        .update_agent_provision_config("GolemHostApi", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .retain(|permission| !matches!(permission, PolymorphicPermissionPattern::Agent(_)));
            config.initial_permissions.lower_bound.positive.push(
                parse_polymorphic_permission("agent(?agent) @ * : view : *")
                    .expect("valid self-view permission"),
            );
        })
        .store()
        .await?;
    let caller = agent_id!("GolemHostApi", "durable-rpc-denial-caller");
    let target = agent_id!("GolemHostApi", "durable-rpc-denial-target");
    let target_id = golem_common::base_model::AgentId {
        component_id: component.id,
        agent_id: target.to_string(),
    };
    configure_scope_card_root(&authority, &component, &caller)?;
    let caller_worker = executor.start_agent(&component.id, caller.clone()).await?;
    let invocation_key = IdempotencyKey::fresh();
    let params = data_value!("GolemHostApi", "durable-rpc-denial-target", "get_self_uri");

    let denied = executor
        .invoke_and_await_agent_with_key(
            &component,
            &caller,
            &invocation_key,
            "outbound_agent_rpc_invoke_and_await_result",
            params.clone(),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(
        denied.as_ref().is_err_and(|error| error.contains("Denied")),
        "outbound RPC must return a typed denial, got {denied:?}"
    );
    assert!(
        executor
            .get_worker_metadata_opt(&target_id)
            .await?
            .is_none(),
        "authorization denial must occur before target activation"
    );

    let caller_oplog = executor
        .get_oplog(&caller_worker, OplogIndex::INITIAL)
        .await?;
    let activation_starts: Vec<_> = caller_oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name == "golem::rpc::wasm-rpc::activate" =>
            {
                Some((entry.oplog_index, params.request.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(activation_starts.len(), 1);
    let (activation_start, activation_request) = &activation_starts[0];
    let activation_request = activation_request
        .as_ref()
        .expect("activation Start must persist its authorization decision");
    let activation_request = HostRequestGolemRpcActivate::from_value(activation_request.value())?;
    assert_eq!(activation_request.remote_agent_id, target_id);
    assert_eq!(activation_request.method_name, "get_self_uri");
    assert!(matches!(
        activation_request.decision,
        Err(SerializableRpcError::Denied { .. })
    ));
    let activation_response = caller_oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::End(params) if params.start_index == *activation_start => {
                params.response.clone()
            }
            _ => None,
        })
        .expect("activation denial must have a durable End response");
    let activation_response =
        HostResponseGolemRpcActivate::from_value(activation_response.value())?;
    assert!(matches!(
        activation_response.result,
        Err(SerializableRpcError::Denied { .. })
    ));
    assert!(
        caller_oplog.iter().all(|entry| !matches!(
            &entry.entry,
            PublicOplogEntry::Start(params)
                if params.function_name == "golem::rpc::wasm-rpc::invoke_and_await"
        )),
        "a denied activation must not open the invocation durable call"
    );
    drop(executor);

    authority.revoke();
    let executor = start_scope_card_executor(deps, &context, authority).await?;
    let replayed = executor
        .invoke_and_await_agent_with_key(
            &component,
            &caller,
            &invocation_key,
            "outbound_agent_rpc_invoke_and_await_result",
            params,
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert_eq!(replayed, denied);
    assert!(
        executor
            .get_worker_metadata_opt(&target_id)
            .await?
            .is_none(),
        "replaying a recorded denial must not activate the target"
    );
    let replayed_caller_oplog = executor
        .get_oplog(&caller_worker, OplogIndex::INITIAL)
        .await?;
    assert_eq!(
        replayed_caller_oplog
            .iter()
            .filter(|entry| matches!(
                &entry.entry,
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem::rpc::wasm-rpc::activate"
            ))
            .count(),
        1,
        "replay must consume the recorded denial without appending another activation decision"
    );
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
async fn revoke_does_not_use_authority_revoked_before_its_boundary(
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
            config
                .initial_permissions
                .lower_bound
                .positive
                .push(parse_polymorphic_permission("card(*) @ * : revoke : *").unwrap());
        })
        .store()
        .await?;
    let caller = agent_id!("ScopeCardAgent", "stale-revoke-authority-caller");
    let authority_card_id = configure_scope_card_root(&authority, &component, &caller)?;
    let caller_worker = executor.start_agent(&component.id, caller.clone()).await?;
    let target_card = registry_test_card();
    let target_card_id = target_card.card_id();
    authority.add_card(target_card.clone());
    executor
        .queue_card_install(&caller_worker, target_card)
        .await?;
    let release = executor
        .invoke_and_await_agent(&component, &caller, "create_release_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let key = IdempotencyKey::fresh();
    let params = data_value!(SchemaCardId::from(target_card_id.0), release.clone());

    executor
        .invoke_agent_with_key(
            &component,
            &caller,
            &key,
            "revoke_card_after_promise_is_denied",
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

    authority.reset_check_cards_count();
    authority.revoke();
    executor
        .queue_uncommitted_card_revocation(&caller_worker, authority_card_id)
        .await?;
    executor.complete_promise(&release, vec![1]).await?;

    assert!(
        executor
            .invoke_and_await_agent_with_key(
                &component,
                &caller,
                &key,
                "revoke_card_after_promise_is_denied",
                params,
            )
            .await?
            .into_typed::<bool>()?,
        "revoked authority must not authorize revoking a separately possessed card"
    );
    assert_eq!(
        authority.revoke_card_count(),
        0,
        "denied revoke must not reach the registry card service"
    );
    let check_cards_count = authority.check_cards_count();
    assert!(
        check_cards_count <= 1,
        "only replay-to-live recovery may check card liveness; revoke authorization added a full-wallet check ({check_cards_count} checks observed)"
    );

    let target_is_still_installed = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "has_card",
            data_value!(SchemaCardId::from(target_card_id.0)),
        )
        .await?
        .into_typed::<bool>()?;
    assert!(
        target_is_still_installed,
        "the denied target must remain installed"
    );

    let oplog = executor
        .get_oplog(&caller_worker, OplogIndex::INITIAL)
        .await?;
    assert!(oplog.iter().any(|entry| matches!(
        &entry.entry,
        PublicOplogEntry::CardRevokedCascade(params)
            if params.revoked_card_ids.contains(&authority_card_id)
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

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn p3_tcp_default_denial_is_durable_and_does_not_reach_the_backend(
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
        .without_default_host_permissions("Networking")
        .store()
        .await?;
    let caller = agent_id!("Networking", "permission-denial");
    configure_scope_card_root(&authority, &component, &caller)?;
    let caller_worker = executor.start_agent(&component.id, caller.clone()).await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let invocation_key = IdempotencyKey::fresh();

    let denied = executor
        .invoke_and_await_agent_with_key(
            &component,
            &caller,
            &invocation_key,
            "tcp_collect_p3",
            data_value!(port),
        )
        .await?
        .into_typed::<Result<String, String>>()?;
    assert!(
        denied
            .as_ref()
            .expect_err("network access must default to deny")
            .contains("AccessDenied")
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(200), listener.accept())
            .await
            .is_err(),
        "a denied connect must not reach the TCP backend"
    );

    let oplog = executor
        .get_oplog(&caller_worker, OplogIndex::INITIAL)
        .await?;
    let connect_start = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name == "sockets::types::tcp-socket::connect" =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("denial must have a durable connect Start");
    let recorded_response = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::End(params) if params.start_index == connect_start => {
                params.response.clone()
            }
            _ => None,
        })
        .expect("denial must have a durable connect End response");
    let expected_response: HostResponse = HostResponseP3SocketsConnect {
        result: Err(SerializableP3SocketErrorCode::AccessDenied),
    }
    .into();
    assert_eq!(
        recorded_response,
        expected_response.into_typed_schema_value()?,
        "the durable terminal must contain the guest-visible typed denial"
    );

    drop(executor);
    authority.revoke();
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    authority.reset_check_cards_count();
    let replayed = executor
        .invoke_and_await_agent_with_key(
            &component,
            &caller,
            &invocation_key,
            "tcp_collect_p3",
            data_value!(port),
        )
        .await?
        .into_typed::<Result<String, String>>()?;
    assert_eq!(replayed, denied);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), listener.accept())
            .await
            .is_err(),
        "replaying a denied connect must not reach the TCP backend"
    );
    assert_eq!(
        authority.check_cards_count(),
        0,
        "replaying a recorded denial must not consult current card authority"
    );
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn p3_tcp_replay_keeps_an_admitted_result_after_revocation_and_denies_new_work(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let connection_count = Arc::new(AtomicUsize::new(0));
    let server_count = connection_count.clone();
    let server = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            server_count.fetch_add(1, Ordering::SeqCst);
            if stream.write_all(b"authorized").await.is_err() {
                break;
            }
            let _ = stream.shutdown().await;
        }
    });

    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .without_default_host_permissions("Networking")
        .update_agent_provision_config("Networking", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .push(p3_tcp_permission(port));
        })
        .store()
        .await?;
    let caller = agent_id!("Networking", "permission-replay");
    let root_card_id = configure_scope_card_root(&authority, &component, &caller)?;
    let caller_worker = executor.start_agent(&component.id, caller.clone()).await?;
    let invocation_key = IdempotencyKey::fresh();

    let first = executor
        .invoke_and_await_agent_with_key(
            &component,
            &caller,
            &invocation_key,
            "tcp_collect_p3",
            data_value!(port),
        )
        .await?
        .into_typed::<Result<String, String>>()?;
    assert_eq!(first, Ok("authorized".to_string()));
    assert_eq!(connection_count.load(Ordering::SeqCst), 1);

    drop(executor);
    authority.revoke();
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    authority.reset_check_cards_count();
    let replayed = executor
        .invoke_and_await_agent_with_key(
            &component,
            &caller,
            &invocation_key,
            "tcp_collect_p3",
            data_value!(port),
        )
        .await?
        .into_typed::<Result<String, String>>()?;
    assert_eq!(replayed, first);
    assert_eq!(
        connection_count.load(Ordering::SeqCst),
        1,
        "replay must not reconnect after authority was revoked"
    );

    let denied = executor
        .invoke_and_await_agent(&component, &caller, "tcp_collect_p3", data_value!(port))
        .await?
        .into_typed::<Result<String, String>>()?;
    assert!(
        denied
            .expect_err("new work must observe the revocation")
            .contains("AccessDenied")
    );
    assert_eq!(connection_count.load(Ordering::SeqCst), 1);
    assert_eq!(authority.check_cards_count(), 1);

    let oplog = executor
        .get_oplog(&caller_worker, OplogIndex::INITIAL)
        .await?;
    assert!(oplog.iter().any(|entry| matches!(
        &entry.entry,
        PublicOplogEntry::CardRevokedCascade(params)
            if params.revoked_card_ids.contains(&root_card_id)
    )));
    server.abort();
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn p3_tcp_expiration_is_enforced_at_the_first_due_live_boundary(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let connection_count = Arc::new(AtomicUsize::new(0));
    let server_count = connection_count.clone();
    let server = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            server_count.fetch_add(1, Ordering::SeqCst);
            if stream.write_all(b"authorized").await.is_err() {
                break;
            }
            let _ = stream.shutdown().await;
        }
    });

    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .without_default_host_permissions("Networking")
        .store()
        .await?;
    let caller = agent_id!("Networking", "permission-expiration");
    configure_scope_card_root(&authority, &component, &caller)?;
    let expiration = Utc::now() + chrono::Duration::seconds(30);
    let caller_worker = executor.start_agent(&component.id, caller.clone()).await?;
    let expiring_card = StoredCard::Polymorphic(PolymorphicCard {
        card_id: CardId::new(),
        parent_ids: Vec::new(),
        lower_positive: vec![p3_tcp_permission(port)],
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: Utc::now(),
        expires_at: Some(expiration),
        system_card: false,
    });
    authority.add_card(expiring_card.clone());
    executor
        .queue_card_install(&caller_worker, expiring_card)
        .await?;

    let first = executor
        .invoke_and_await_agent(&component, &caller, "tcp_collect_p3", data_value!(port))
        .await?
        .into_typed::<Result<String, String>>()?;
    assert_eq!(first, Ok("authorized".to_string()));
    assert_eq!(connection_count.load(Ordering::SeqCst), 1);

    tokio::time::sleep((expiration - Utc::now()).to_std().unwrap_or_default()).await;
    let denied = executor
        .invoke_and_await_agent(&component, &caller, "tcp_collect_p3", data_value!(port))
        .await?
        .into_typed::<Result<String, String>>()?;
    assert!(
        denied
            .expect_err("new work at the expiration boundary must be denied")
            .contains("AccessDenied")
    );
    assert_eq!(
        connection_count.load(Ordering::SeqCst),
        1,
        "the expired operation must not reach the TCP backend"
    );
    let oplog = executor
        .get_oplog(&caller_worker, OplogIndex::INITIAL)
        .await?;
    assert!(
        oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::CardExpired(_)))
    );
    server.abort();
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn filesystem_admitted_write_stream_survives_revocation_and_crash_replay(
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
        .without_default_host_permissions("FileSystem")
        .update_agent_provision_config("FileSystem", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .push(filesystem_permission("write", "/admitted.txt"));
        })
        .store()
        .await?;
    let agent_id = agent_id!("FileSystem", "admitted-stream-replay");
    configure_scope_card_root(&authority, &component, &agent_id)?;
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let release = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "create_release_promise",
            data_value!(),
        )
        .await?
        .into_typed::<PromiseId>()?;
    let key = IdempotencyKey::fresh();
    let params = data_value!("/admitted.txt", "admitted", release.clone());

    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &key,
            "write_after_promise",
            params.clone(),
        )
        .await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;
    drop(executor);

    authority.revoke();
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    authority.reset_check_cards_count();
    executor.complete_promise(&release, vec![1]).await?;
    let replayed = executor
        .invoke_and_await_agent_with_key(&component, &agent_id, &key, "write_after_promise", params)
        .await?
        .into_typed::<Result<(), String>>()?;
    assert_eq!(replayed, Ok(()));
    let admitted_contents = executor
        .get_file_contents(&worker_id, "/admitted.txt")
        .await?;
    assert_eq!(admitted_contents.as_ref(), b"admitted");

    let denied = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "stream_to_file",
            data_value!("/denied.txt", 1u64),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    let denial = denied.expect_err("a new stream must observe revocation");
    assert!(
        denial
            .chars()
            .filter(|char| char.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase()
            .contains("notpermitted"),
        "expected typed NotPermitted denial, got {denial}"
    );
    assert!(
        executor
            .get_file_contents(&worker_id, "/denied.txt")
            .await
            .is_err(),
        "denied stream creation must not create a file"
    );
    assert_eq!(
        authority.check_cards_count(),
        1,
        "replay must retain stream admission without reauthorizing it"
    );
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn cache_vacancy_admission_survives_revocation_and_denies_new_work(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    let cache_bucket = "__golem_wasi_keyvalue_cache";
    let key = "admitted-vacancy";
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .without_default_host_permissions("KeyValue")
        .update_agent_provision_config("KeyValue", |config| {
            config.initial_permissions.lower_bound.positive.extend([
                kv_permission("read", cache_bucket, key),
                kv_permission("write", cache_bucket, key),
            ]);
        })
        .store()
        .await?;
    let agent = agent_id!("KeyValue", "admitted-cache-vacancy");
    let root_card_id = configure_scope_card_root(&authority, &component, &agent)?;
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;
    let release = executor
        .invoke_and_await_agent(&component, &agent, "create_release_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let invocation_key = IdempotencyKey::fresh();
    let params = data_value!(key, vec![1u8, 2, 3], release.clone());

    executor
        .invoke_agent_with_key(
            &component,
            &agent,
            &invocation_key,
            "cache_fill_after_promise",
            params.clone(),
        )
        .await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;
    authority.revoke();
    executor
        .queue_card_revocation(&worker_id, root_card_id)
        .await?;
    executor.complete_promise(&release, vec![1]).await?;

    let admitted = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent,
            &invocation_key,
            "cache_fill_after_promise",
            params,
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert_eq!(
        admitted,
        Ok(()),
        "an admitted cache vacancy must retain its permit through fill"
    );

    let denied = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "cache_probe",
            data_value!("get", key, Vec::<u8>::new()),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(
        denied.is_err(),
        "a new cache operation must observe revocation"
    );
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn blob_read_stream_admission_survives_revocation_and_denies_new_work(
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
        .without_default_host_permissions("BlobStore")
        .update_agent_provision_config("BlobStore", |config| {
            config.initial_permissions.lower_bound.positive.extend([
                blob_permission("read", "admitted-container.**"),
                blob_permission("write", "admitted-container.**"),
            ]);
        })
        .store()
        .await?;
    let agent = agent_id!("BlobStore", "admitted-blob-read-stream");
    let root_card_id = configure_scope_card_root(&authority, &component, &agent)?;
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(
                &component,
                &agent,
                "blobstore_probe",
                data_value!("create-container", "admitted-container", "", "", ""),
            )
            .await?
            .into_typed::<Result<(), String>>()?,
        Ok(())
    );
    let contents = vec![4u8, 5, 6];
    assert_eq!(
        executor
            .invoke_and_await_agent(
                &component,
                &agent,
                "write_data_result",
                data_value!("admitted-container", "admitted-object", contents.clone()),
            )
            .await?
            .into_typed::<Result<(), String>>()?,
        Ok(())
    );
    let release = executor
        .invoke_and_await_agent(&component, &agent, "create_release_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let invocation_key = IdempotencyKey::fresh();
    let params = data_value!("admitted-container", "admitted-object", release.clone());

    executor
        .invoke_agent_with_key(
            &component,
            &agent,
            &invocation_key,
            "consume_data_after_promise",
            params.clone(),
        )
        .await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;
    authority.revoke();
    executor
        .queue_card_revocation(&worker_id, root_card_id)
        .await?;
    executor.complete_promise(&release, vec![1]).await?;

    let admitted = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent,
            &invocation_key,
            "consume_data_after_promise",
            params,
        )
        .await?
        .into_typed::<Result<Vec<u8>, String>>()?;
    assert_eq!(
        admitted,
        Ok(contents),
        "an admitted blob read stream must retain its permit while consumed"
    );

    let denied = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "get_data_result",
            data_value!("admitted-container", "admitted-object"),
        )
        .await?
        .into_typed::<Result<Vec<u8>, String>>()?;
    assert!(denied.is_err(), "a new blob read must observe revocation");
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn filesystem_rename_preflights_both_paths_before_mutating_source(
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
        .without_default_host_permissions("FileSystem")
        .update_agent_provision_config("FileSystem", |config| {
            config.initial_permissions.lower_bound.positive.extend([
                filesystem_permission("write", "/source.txt"),
                filesystem_permission("delete", "/source.txt"),
            ]);
        })
        .store()
        .await?;
    let agent_id = agent_id!("FileSystem", "rename-preflight");
    configure_scope_card_root(&authority, &component, &agent_id)?;
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let created = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/source.txt", "source"),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert_eq!(created, Ok(()));

    let denied = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "rename_file",
            data_value!("/source.txt", "/destination.txt"),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(
        denied
            .expect_err("destination without write authority must be denied")
            .contains("not permitted")
    );
    let source_contents = executor
        .get_file_contents(&worker_id, "/source.txt")
        .await?;
    assert_eq!(source_contents.as_ref(), b"source");
    assert!(
        executor
            .get_file_contents(&worker_id, "/destination.txt")
            .await
            .is_err()
    );
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn every_protected_p2_and_p3_filesystem_import_enforces_permissions(
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
        .without_default_host_permissions("FileSystem")
        .store()
        .await?;
    let agent = agent_id!("FileSystem", "every-protected-filesystem-import");
    configure_scope_card_root(&authority, &component, &agent)?;
    executor.start_agent(&component.id, agent.clone()).await?;

    let descriptor_operations = [
        "read",
        "read-via-stream",
        "write",
        "write-via-stream",
        "append-via-stream",
        "set-size",
        "set-times",
        "sync-data",
        "sync",
        "read-directory",
        "stat",
        "stat-at",
        "metadata-hash",
        "metadata-hash-at",
        "readlink-at",
        "create-directory-at",
        "set-times-at",
        "symlink-at",
        "remove-directory-at",
        "unlink-file-at",
        "rename-at",
        "link-at",
    ];
    for preview in ["probe_p2", "probe_p3"] {
        for operation in descriptor_operations {
            if preview == "probe_p3" && matches!(operation, "read" | "write") {
                continue;
            }
            let denied = executor
                .invoke_and_await_agent(
                    &component,
                    &agent,
                    preview,
                    data_value!(operation, "", format!("{preview}-{operation}")),
                )
                .await?
                .into_typed::<Result<(), String>>()?;
            assert!(
                denied.as_ref().is_err_and(|error| is_not_permitted(error)),
                "{preview} {operation} must return a typed NotPermitted denial, got {denied:?}"
            );
        }
        for operation in ["open-read", "open-create", "open-write", "open-truncate"] {
            let denied = executor
                .invoke_and_await_agent(
                    &component,
                    &agent,
                    preview,
                    data_value!(operation, format!("{preview}-{operation}"), ""),
                )
                .await?
                .into_typed::<Result<(), String>>()?;
            assert!(
                denied.as_ref().is_err_and(|error| is_not_permitted(error)),
                "{preview} {operation} must return a typed NotPermitted denial, got {denied:?}"
            );
        }
    }

    let list_component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .unique()
        .name("filesystem-open-list-denied")
        .without_default_host_permissions("FileSystem")
        .update_agent_provision_config("FileSystem", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .push(filesystem_permission("read", "/open-list"));
        })
        .store()
        .await?;
    let list_agent = agent_id!("FileSystem", "open-list-denied");
    configure_scope_card_root(&authority, &list_component, &list_agent)?;
    executor
        .start_agent(&list_component.id, list_agent.clone())
        .await?;
    for preview in ["probe_p2", "probe_p3"] {
        let denied = executor
            .invoke_and_await_agent(
                &list_component,
                &list_agent,
                preview,
                data_value!("open-list", "open-list", ""),
            )
            .await?
            .into_typed::<Result<(), String>>()?;
        assert!(
            denied.as_ref().is_err_and(|error| is_not_permitted(error)),
            "{preview} open-list must preflight List in addition to Read, got {denied:?}"
        );
    }
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn filesystem_permissions_isolate_resource_owners(
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
        .without_default_host_permissions("FileSystem")
        .try_update_agent_provision_config("FileSystem", |config| {
            for grant in [
                "filesystem(?agent) @ * : write : /owned.txt",
                "filesystem(other/shop/prod/other/FileSystem(*)) @ * : write : /foreign.txt",
            ] {
                config
                    .initial_permissions
                    .lower_bound
                    .positive
                    .push(parse_polymorphic_permission(grant)?);
            }
            Ok::<_, golem_common::model::card::CardParseError>(())
        })?
        .store()
        .await?;
    let agent = agent_id!("FileSystem", "owner-isolation");
    configure_scope_card_root(&authority, &component, &agent)?;
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    let allowed = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/owned.txt", "owned"),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert_eq!(allowed, Ok(()));

    let denied = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/foreign.txt", "foreign"),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(
        denied
            .expect_err("a grant for another owner must not authorize this filesystem")
            .contains("not permitted")
    );
    let owned_contents = executor.get_file_contents(&worker, "/owned.txt").await?;
    assert_eq!(owned_contents.as_ref(), b"owned");
    assert!(
        executor
            .get_file_contents(&worker, "/foreign.txt")
            .await
            .is_err(),
        "an owner-mismatched grant must produce no filesystem effect"
    );
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn concurrent_p3_operations_authorize_independently_before_backend_access(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let allowed_listener = TcpListener::bind("127.0.0.1:0").await?;
    let allowed_port = allowed_listener.local_addr()?.port();
    let denied_listener = TcpListener::bind("127.0.0.1:0").await?;
    let denied_port = denied_listener.local_addr()?.port();
    let allowed_connections = Arc::new(AtomicUsize::new(0));
    let server_connections = allowed_connections.clone();
    let server = tokio::spawn(async move {
        while let Ok((mut stream, _)) = allowed_listener.accept().await {
            server_connections.fetch_add(1, Ordering::SeqCst);
            if stream.write_all(b"allowed").await.is_err() {
                break;
            }
            let _ = stream.shutdown().await;
        }
    });

    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .without_default_host_permissions("Networking")
        .update_agent_provision_config("Networking", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .push(p3_tcp_permission(allowed_port));
        })
        .store()
        .await?;
    let agent = agent_id!("Networking", "concurrent-permissions");
    configure_scope_card_root(&authority, &component, &agent)?;
    executor.start_agent(&component.id, agent.clone()).await?;

    let (allowed, denied) = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "tcp_collect_two_p3",
            data_value!(allowed_port, denied_port),
        )
        .await?
        .into_typed::<(Result<String, String>, Result<String, String>)>()?;
    assert_eq!(allowed, Ok("allowed".to_string()));
    assert!(
        denied
            .expect_err("the concurrent ungranted connection must be denied")
            .contains("AccessDenied")
    );
    assert_eq!(allowed_connections.load(Ordering::SeqCst), 1);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), denied_listener.accept())
            .await
            .is_err(),
        "the denied concurrent connection must not reach its backend"
    );
    server.abort();
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn every_protected_network_http_and_websocket_import_enforces_permissions(
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
        .without_default_host_permissions("Networking")
        .without_default_host_permissions("RawWasiHttp")
        .without_default_host_permissions("WebsocketTest")
        .store()
        .await?;

    let networking = agent_id!("Networking", "every-protected-network-import");
    configure_scope_card_root(&authority, &component, &networking)?;
    executor
        .start_agent(&component.id, networking.clone())
        .await?;
    for operation in [
        "resolve-addresses",
        "tcp-start-connect",
        "udp-stream-send",
        "udp-unconnected-send",
    ] {
        let denied = executor
            .invoke_and_await_agent(
                &component,
                &networking,
                "probe_p2",
                data_value!(operation, "localhost", 9u16, vec![1u8]),
            )
            .await?
            .into_typed::<Result<String, String>>()?;
        assert!(
            denied
                .as_ref()
                .is_err_and(|error| normalized_error_contains(error, "accessdenied")),
            "P2 network {operation} must return AccessDenied, got {denied:?}"
        );
    }
    let dns_denied = executor
        .invoke_and_await_agent(
            &component,
            &networking,
            "resolve_p3",
            data_value!("localhost"),
        )
        .await?
        .into_typed::<Result<Vec<String>, String>>()?;
    assert!(
        dns_denied
            .as_ref()
            .is_err_and(|error| normalized_error_contains(error, "accessdenied")),
        "P3 DNS must return AccessDenied, got {dns_denied:?}"
    );
    let tcp_denied = executor
        .invoke_and_await_agent(&component, &networking, "tcp_collect_p3", data_value!(9u16))
        .await?
        .into_typed::<Result<String, String>>()?;
    assert!(
        tcp_denied
            .as_ref()
            .is_err_and(|error| normalized_error_contains(error, "accessdenied")),
        "P3 TCP connect must return AccessDenied, got {tcp_denied:?}"
    );
    for operation in ["udp-connect", "udp-send-unconnected"] {
        let denied = executor
            .invoke_and_await_agent(
                &component,
                &networking,
                "probe_udp_p3",
                data_value!(operation, 9u16, vec![1u8]),
            )
            .await?
            .into_typed::<Result<(), String>>()?;
        assert!(
            denied
                .as_ref()
                .is_err_and(|error| normalized_error_contains(error, "accessdenied")),
            "P3 {operation} must return AccessDenied, got {denied:?}"
        );
    }

    let http = agent_id!("RawWasiHttp", "every-protected-http-import");
    configure_scope_card_root(&authority, &component, &http)?;
    executor.start_agent(&component.id, http.clone()).await?;
    let p2_http_denied = executor
        .invoke_and_await_agent(
            &component,
            &http,
            "probe_p2",
            data_value!("outgoing-handler-handle", "127.0.0.1:9"),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(
        p2_http_denied
            .as_ref()
            .is_err_and(|error| normalized_error_contains(error, "httprequestdenied")),
        "P2 HTTP handle must return HttpRequestDenied, got {p2_http_denied:?}"
    );
    let p3_http_denied = executor
        .invoke_and_await_agent(
            &component,
            &http,
            "dispatch_result",
            data_value!("127.0.0.1:9"),
        )
        .await?
        .into_typed::<Result<u16, String>>()?;
    assert!(
        p3_http_denied
            .as_ref()
            .is_err_and(|error| normalized_error_contains(error, "httprequestdenied")),
        "P3 HTTP send must return HttpRequestDenied, got {p3_http_denied:?}"
    );

    let websocket = agent_id!("WebsocketTest", "every-protected-websocket-import");
    configure_scope_card_root(&authority, &component, &websocket)?;
    executor
        .start_agent(&component.id, websocket.clone())
        .await?;
    let websocket_denied = executor
        .invoke_and_await_agent(
            &component,
            &websocket,
            "connect_result",
            data_value!("ws://127.0.0.1:9"),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(
        websocket_denied
            .as_ref()
            .is_err_and(|error| normalized_error_contains(error, "denied")),
        "WebSocket connect must return a typed denial, got {websocket_denied:?}"
    );
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn every_protected_storage_config_and_secret_import_enforces_permissions(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let environment_state_service = Arc::new(TestEnvironmentStateService::default());
    let secret_path = CanonicalAgentSecretPath(vec!["secretPath".to_string()]);
    environment_state_service.set_agent_secret(AgentSecret {
        environment_id: context.default_environment_id,
        revision: AgentSecretRevision::INITIAL,
        path: secret_path,
        id: AgentSecretId::new(),
        secret_type: SchemaGraph::anonymous(SchemaType::string()),
        secret_value: Some(SchemaValue::String("must-not-leak".to_string())),
    });
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            create_card_service: Some(Arc::new({
                let authority = authority.clone();
                move || {
                    Arc::new(ScopeCardService {
                        authority: authority.clone(),
                    })
                }
            })),
            environment_state_service: Some(environment_state_service),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .without_default_host_permissions("KeyValue")
        .without_default_host_permissions("BlobStore")
        .without_default_host_permissions("WasiConfig")
        .with_agent_config(
            "WasiConfig",
            vec![
                AgentConfigEntryDto {
                    path: vec!["k1".to_string()],
                    value: serde_json::Value::String("private-1".to_string()).into(),
                },
                AgentConfigEntryDto {
                    path: vec!["k2".to_string()],
                    value: serde_json::Value::String("private-2".to_string()).into(),
                },
            ],
        )
        .store()
        .await?;

    let key_value = agent_id!("KeyValue", "every-protected-keyvalue-import");
    configure_scope_card_root(&authority, &component, &key_value)?;
    executor
        .start_agent(&component.id, key_value.clone())
        .await?;
    for operation in ["get", "exists", "set", "delete"] {
        let denied = executor
            .invoke_and_await_agent(
                &component,
                &key_value,
                "eventual_probe",
                data_value!(operation, "probe-store", "probe-key", vec![1u8]),
            )
            .await?
            .into_typed::<Result<(), String>>()?;
        assert!(denied.is_err(), "eventual {operation} must be denied");
    }
    for operation in ["get-many", "keys", "set-many", "delete-many"] {
        let denied = executor
            .invoke_and_await_agent(
                &component,
                &key_value,
                "eventual_batch_probe",
                data_value!(
                    operation,
                    "probe-store",
                    vec!["probe-key".to_string()],
                    vec![1u8]
                ),
            )
            .await?
            .into_typed::<Result<(), String>>()?;
        assert!(denied.is_err(), "eventual-batch {operation} must be denied");
    }
    for operation in ["get", "exists", "set", "get-or-set", "delete"] {
        let denied = executor
            .invoke_and_await_agent(
                &component,
                &key_value,
                "cache_probe",
                data_value!(operation, "probe-key", vec![1u8]),
            )
            .await?
            .into_typed::<Result<(), String>>()?;
        assert!(denied.is_err(), "cache {operation} must be denied");
    }

    let partial_kv_component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .unique()
        .name("keyvalue-batch-preflight")
        .without_default_host_permissions("KeyValue")
        .update_agent_provision_config("KeyValue", |config| {
            config.initial_permissions.lower_bound.positive.extend([
                kv_permission("read", "atomic-store", "allowed-key"),
                kv_permission("write", "atomic-store", "allowed-key"),
            ]);
        })
        .store()
        .await?;
    let partial_kv = agent_id!("KeyValue", "keyvalue-batch-preflight");
    configure_scope_card_root(&authority, &partial_kv_component, &partial_kv)?;
    executor
        .start_agent(&partial_kv_component.id, partial_kv.clone())
        .await?;
    let denied_batch = executor
        .invoke_and_await_agent(
            &partial_kv_component,
            &partial_kv,
            "set_many_result",
            data_value!(
                "atomic-store",
                vec![
                    ("allowed-key".to_string(), vec![1u8]),
                    ("denied-key".to_string(), vec![2u8]),
                ]
            ),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(
        denied_batch.is_err(),
        "one denied key must reject the complete KV batch"
    );
    assert_eq!(
        executor
            .invoke_and_await_agent(
                &partial_kv_component,
                &partial_kv,
                "get_result",
                data_value!("atomic-store", "allowed-key"),
            )
            .await?
            .into_typed::<Result<Option<Vec<u8>>, String>>()?,
        Ok(None),
        "the admitted key must not be written when another batch key is denied"
    );

    let blobstore = agent_id!("BlobStore", "every-protected-blobstore-import");
    configure_scope_card_root(&authority, &component, &blobstore)?;
    executor
        .start_agent(&component.id, blobstore.clone())
        .await?;
    for operation in [
        "create-container",
        "get-container",
        "delete-container",
        "container-exists",
        "copy-object",
        "move-object",
    ] {
        let denied = executor
            .invoke_and_await_agent(
                &component,
                &blobstore,
                "blobstore_probe",
                data_value!(
                    operation,
                    "probe-container",
                    "probe-object",
                    "probe-destination",
                    "probe-destination-object"
                ),
            )
            .await?
            .into_typed::<Result<(), String>>()?;
        assert!(denied.is_err(), "blobstore {operation} must be denied");
    }

    let write_component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .unique()
        .name("blob-container-read-list-denied")
        .without_default_host_permissions("BlobStore")
        .update_agent_provision_config("BlobStore", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .push(blob_permission("write", "*.**"));
        })
        .store()
        .await?;
    let write_agent = agent_id!("BlobStore", "blob-container-read-list-denied");
    configure_scope_card_root(&authority, &write_component, &write_agent)?;
    executor
        .start_agent(&write_component.id, write_agent.clone())
        .await?;
    for operation in ["get-data", "has-object", "object-info", "list-objects"] {
        let denied = executor
            .invoke_and_await_agent(
                &write_component,
                &write_agent,
                "container_probe",
                data_value!(
                    operation,
                    format!("probe-container-{operation}"),
                    "probe-object",
                    vec!["probe-object".to_string()],
                    vec![1u8]
                ),
            )
            .await?
            .into_typed::<Result<(), String>>()?;
        assert!(denied.is_err(), "blob container {operation} must be denied");
    }
    let created = executor
        .invoke_and_await_agent(
            &write_component,
            &write_agent,
            "blobstore_probe",
            data_value!("create-container", "mutation-container", "", "", ""),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert_eq!(created, Ok(()));

    let read_component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .unique()
        .name("blob-container-write-delete-denied")
        .without_default_host_permissions("BlobStore")
        .update_agent_provision_config("BlobStore", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .push(blob_permission("read", "mutation-container.**"));
        })
        .store()
        .await?;
    let read_agent = agent_id!("BlobStore", "blob-container-write-delete-denied");
    configure_scope_card_root(&authority, &read_component, &read_agent)?;
    executor
        .start_agent(&read_component.id, read_agent.clone())
        .await?;
    for operation in ["write-data", "delete-object", "delete-objects", "clear"] {
        let denied = executor
            .invoke_and_await_agent(
                &read_component,
                &read_agent,
                "container_probe",
                data_value!(
                    operation,
                    "mutation-container",
                    "probe-object",
                    vec!["probe-object".to_string()],
                    vec![1u8]
                ),
            )
            .await?
            .into_typed::<Result<(), String>>()?;
        assert!(denied.is_err(), "blob container {operation} must be denied");
    }

    let seeded_blob_component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .unique()
        .name("blob-delete-batch-preflight-seed")
        .without_default_host_permissions("BlobStore")
        .update_agent_provision_config("BlobStore", |config| {
            config.initial_permissions.lower_bound.positive.extend([
                blob_permission("read", "atomic-container.**"),
                blob_permission("write", "atomic-container.**"),
            ]);
        })
        .store()
        .await?;
    let seeded_blob = agent_id!("BlobStore", "blob-delete-batch-preflight-seed");
    configure_scope_card_root(&authority, &seeded_blob_component, &seeded_blob)?;
    executor
        .start_agent(&seeded_blob_component.id, seeded_blob.clone())
        .await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(
                &seeded_blob_component,
                &seeded_blob,
                "blobstore_probe",
                data_value!("create-container", "atomic-container", "", "", ""),
            )
            .await?
            .into_typed::<Result<(), String>>()?,
        Ok(())
    );
    for (object, contents) in [("allowed-object", vec![1u8]), ("denied-object", vec![2u8])] {
        assert_eq!(
            executor
                .invoke_and_await_agent(
                    &seeded_blob_component,
                    &seeded_blob,
                    "write_data_result",
                    data_value!("atomic-container", object, contents),
                )
                .await?
                .into_typed::<Result<(), String>>()?,
            Ok(())
        );
    }

    let partial_blob_component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .unique()
        .name("blob-delete-batch-preflight")
        .without_default_host_permissions("BlobStore")
        .update_agent_provision_config("BlobStore", |config| {
            config.initial_permissions.lower_bound.positive.extend([
                blob_permission("read", "atomic-container.**"),
                blob_permission("delete", "atomic-container.allowed-object"),
            ]);
        })
        .store()
        .await?;
    let partial_blob = agent_id!("BlobStore", "blob-delete-batch-preflight");
    configure_scope_card_root(&authority, &partial_blob_component, &partial_blob)?;
    executor
        .start_agent(&partial_blob_component.id, partial_blob.clone())
        .await?;
    let denied_delete_batch = executor
        .invoke_and_await_agent(
            &partial_blob_component,
            &partial_blob,
            "container_probe",
            data_value!(
                "delete-objects",
                "atomic-container",
                "",
                vec!["allowed-object".to_string(), "denied-object".to_string()],
                Vec::<u8>::new()
            ),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(
        denied_delete_batch.is_err(),
        "one denied object must reject the complete blob delete batch"
    );
    for (object, contents) in [("allowed-object", vec![1u8]), ("denied-object", vec![2u8])] {
        assert_eq!(
            executor
                .invoke_and_await_agent(
                    &partial_blob_component,
                    &partial_blob,
                    "get_data_result",
                    data_value!("atomic-container", object),
                )
                .await?
                .into_typed::<Result<Vec<u8>, String>>()?,
            Ok(contents),
            "no object may be deleted when any batch object is denied"
        );
    }

    let config = agent_id!("WasiConfig", "every-protected-config-import");
    configure_scope_card_root(&authority, &component, &config)?;
    executor.start_agent(&component.id, config.clone()).await?;
    let get_denied = executor
        .invoke_and_await_agent(&component, &config, "get_result", data_value!("k1"))
        .await?
        .into_typed::<Result<Option<String>, String>>()?;
    assert!(get_denied.is_err(), "config get must return a typed denial");
    let get_all = executor
        .invoke_and_await_agent(&component, &config, "get_all_result", data_value!())
        .await?
        .into_typed::<Result<Vec<(String, String)>, String>>()?;
    assert_eq!(get_all, Ok(Vec::new()));

    let secret_component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .unique()
        .without_default_host_permissions("SecretHandleAgent")
        .update_agent_provision_config("SecretHandleAgent", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .extend([config_permission("*"), secret_permission("hold", "*")]);
        })
        .store()
        .await?;
    let secret = agent_id!("SecretHandleAgent", "every-protected-secret-import");
    configure_scope_card_root(&authority, &secret_component, &secret)?;
    executor
        .start_agent(&secret_component.id, secret.clone())
        .await?;
    for method in ["secret_id_result", "secret_metadata_result"] {
        let admitted = executor
            .invoke_and_await_agent(&secret_component, &secret, method, data_value!())
            .await?
            .into_typed::<Result<String, String>>()?;
        assert!(
            admitted.is_ok(),
            "secret {method} must be available after handle admission"
        );
    }
    let reveal_denied = executor
        .invoke_and_await_agent(
            &secret_component,
            &secret,
            "reveal_secret_result",
            data_value!(),
        )
        .await?
        .into_typed::<Result<String, String>>()?;
    assert!(reveal_denied.is_err(), "secret reveal must be denied");
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn every_protected_rdbms_agent_rpc_tool_and_oplog_import_enforces_permissions(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let environment_state_service = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            create_card_service: Some(Arc::new({
                let authority = authority.clone();
                move || {
                    Arc::new(ScopeCardService {
                        authority: authority.clone(),
                    })
                }
            })),
            environment_state_service: Some(environment_state_service.clone()),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .name("every-protected-agent-imports")
        .without_default_host_permissions("RelationalDatabases")
        .without_default_host_permissions("GolemHostApi")
        .update_agent_provision_config("GolemHostApi", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .retain(|permission| !matches!(permission, PolymorphicPermissionPattern::Agent(_)));
            config.initial_permissions.lower_bound.positive.push(
                parse_polymorphic_permission("agent(?agent) @ * : view : *")
                    .expect("valid self-view permission"),
            );
        })
        .store()
        .await?;

    let rdbms = agent_id!("RelationalDatabases", "every-protected-rdbms-import");
    configure_scope_card_root(&authority, &component, &rdbms)?;
    executor.start_agent(&component.id, rdbms.clone()).await?;
    for (method, address) in [
        ("postgres_operation_result", "postgres://127.0.0.1:9/probe"),
        ("mysql_operation_result", "mysql://127.0.0.1:9/probe"),
        ("ignite_operation_result", "ignite://127.0.0.1:9"),
    ] {
        let opened = executor
            .invoke_and_await_agent(&component, &rdbms, method, data_value!("open", address, ""))
            .await?
            .into_typed::<Result<String, String>>()?;
        assert_eq!(opened, Ok("open".to_string()));
        for (operation, statement) in [
            ("connection-query", "SELECT * FROM protected_table"),
            ("connection-query-stream", "SELECT * FROM protected_table"),
            (
                "connection-execute",
                "INSERT INTO protected_table VALUES (1)",
            ),
            ("connection-begin-transaction", ""),
        ] {
            let denied = executor
                .invoke_and_await_agent(
                    &component,
                    &rdbms,
                    method,
                    data_value!(operation, address, statement),
                )
                .await?
                .into_typed::<Result<String, String>>()?;
            assert!(
                denied
                    .as_ref()
                    .is_err_and(|error| error.to_ascii_lowercase().contains("permission")),
                "{method} {operation} must return a typed permission denial, got {denied:?}"
            );
        }
    }

    let caller = agent_id!("GolemHostApi", "every-protected-agent-import");
    configure_scope_card_root(&authority, &component, &caller)?;
    executor.start_agent(&component.id, caller.clone()).await?;
    let target = agent_id!("GolemHostApi", "protected-target");
    let target_id = golem_common::base_model::AgentId {
        component_id: component.id,
        agent_id: target.to_string(),
    };
    let caller_id = golem_common::base_model::AgentId {
        component_id: component.id,
        agent_id: caller.to_string(),
    };
    let denied_agents = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "get_agents_next_result",
            data_value!(component.id),
        )
        .await?
        .into_typed::<Result<u64, String>>()?;
    assert!(denied_agents.is_err(), "agent enumeration must be denied");
    let denied_self = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "get_self_metadata_result",
            data_value!(),
        )
        .await?
        .into_typed::<Result<String, String>>()?;
    assert_eq!(denied_self, Ok(caller.to_string()));
    let denied_strict = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "resolve_agent_id_strict_result",
            data_value!("every-protected-agent-imports", target.to_string()),
        )
        .await?
        .into_typed::<Result<bool, String>>()?;
    assert_eq!(
        denied_strict,
        Ok(false),
        "legacy strict agent resolution must map denial to none"
    );
    let denied_metadata = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "get_agent_metadata_result",
            data_value!(target_id.clone()),
        )
        .await?
        .into_typed::<Result<bool, String>>()?;
    assert_eq!(
        denied_metadata,
        Ok(false),
        "legacy agent metadata lookup must map denial to none"
    );
    let denied_self_fork = executor
        .invoke_and_await_agent(&component, &caller, "self_fork_result", data_value!())
        .await?
        .into_typed::<Result<String, String>>()?;
    assert!(denied_self_fork.is_err(), "self fork must be denied");
    for (method, params) in [
        ("update_agent_result", data_value!(target_id.clone(), 1u64)),
        (
            "fork_agent_result",
            data_value!(caller_id.clone(), target_id.clone(), 0u64),
        ),
        ("revert_agent_result", data_value!(target_id.clone(), 0u64)),
    ] {
        let denied = executor
            .invoke_and_await_agent(&component, &caller, method, params)
            .await?
            .into_typed::<Result<(), String>>()?;
        assert!(denied.is_err(), "agent operation {method} must be denied");
    }

    for method in ["read_oplog_result", "search_oplog_result"] {
        let params = if method == "read_oplog_result" {
            data_value!(target_id.clone())
        } else {
            data_value!(target_id.clone(), "payload")
        };
        let denied = executor
            .invoke_and_await_agent(&component, &caller, method, params)
            .await?
            .into_typed::<Result<(), String>>()?;
        assert!(denied.is_err(), "oplog operation {method} must be denied");
    }
    let denied_config = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "get_config_value_result",
            data_value!(vec!["private".to_string()]),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(denied_config.is_err(), "agent config lookup must be denied");
    let promise = executor
        .invoke_and_await_agent(&component, &caller, "create_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let denied_webhook = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "create_webhook_result",
            data_value!(promise),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(denied_webhook.is_err(), "webhook creation must be denied");

    for method in [
        "outbound_agent_rpc_invoke_result",
        "outbound_agent_rpc_async_invoke_and_await_result",
        "outbound_agent_rpc_invoke_and_await_result",
        "outbound_agent_rpc_schedule_result",
        "outbound_agent_rpc_schedule_cancelable_result",
    ] {
        let denied = executor
            .invoke_and_await_agent(
                &component,
                &caller,
                method,
                data_value!("GolemHostApi", "protected-target", "get_self_uri"),
            )
            .await?
            .into_typed::<Result<(), String>>()?;
        assert!(
            denied.as_ref().is_err_and(|error| error.contains("Denied")),
            "agent RPC {method} must return RpcError::Denied, got {denied:?}"
        );
    }

    let tool_component_id = ComponentId::new();
    let tool_name = "every-protected-tool";
    environment_state_service.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(deployment_state(
            &AgentTypeName("GolemHostApi".to_string()),
            1,
            &[(tool_name, tool_component_id, true)],
        )),
    );
    for method in [
        "tool_rpc_invoke_result",
        "tool_rpc_async_invoke_and_await_result",
        "tool_rpc_invoke_and_await_result",
    ] {
        let denied = executor
            .invoke_and_await_agent(
                &component,
                &caller,
                method,
                data_value!(tool_name, Vec::<String>::new(), "input"),
            )
            .await?
            .into_typed::<Result<(), String>>()?;
        assert!(
            denied.as_ref().is_err_and(|error| error.contains("Denied")),
            "tool RPC {method} must return RpcError::Denied, got {denied:?}"
        );
    }
    Ok(())
}

#[test]
#[timeout("5m")]
#[tracing::instrument]
async fn every_protected_rdbms_transaction_import_enforces_permissions(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    permission_postgres: &DockerPostgresRdb,
    permission_mysql: &DockerMysqlRdb,
    permission_ignite: &DockerIgniteRdb,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .without_default_host_permissions("RelationalDatabases")
        .try_update_agent_provision_config("RelationalDatabases", |config| {
            for grant in [
                "rdbms(?env) @ * : query : postgres.*.*",
                "rdbms(?env) @ * : mutate : postgres.*.*",
                "rdbms(?env) @ * : query : mysql.*.*",
                "rdbms(?env) @ * : mutate : mysql.*.*",
                "rdbms(?env) @ * : query : default.*.*",
                "rdbms(?env) @ * : mutate : default.*.*",
            ] {
                config
                    .initial_permissions
                    .lower_bound
                    .positive
                    .push(parse_polymorphic_permission(grant)?);
            }
            for grant in [
                "rdbms(?env) @ * : query : postgres.blocked.denied_table",
                "rdbms(?env) @ * : mutate : postgres.blocked.denied_table",
                "rdbms(?env) @ * : query : blocked.blocked.denied_table",
                "rdbms(?env) @ * : mutate : blocked.blocked.denied_table",
                "rdbms(?env) @ * : query : default.blocked.denied_table",
                "rdbms(?env) @ * : mutate : default.blocked.denied_table",
            ] {
                config
                    .initial_permissions
                    .lower_bound
                    .negative
                    .push(parse_polymorphic_permission(grant)?);
            }
            Ok::<_, golem_common::model::card::CardParseError>(())
        })?
        .store()
        .await?;
    let agent = agent_id!(
        "RelationalDatabases",
        "every-protected-rdbms-transaction-import"
    );
    configure_scope_card_root(&authority, &component, &agent)?;
    executor.start_agent(&component.id, agent.clone()).await?;

    for (method, address) in [
        (
            "postgres_operation_result",
            permission_postgres.public_connection_string(),
        ),
        (
            "mysql_operation_result",
            permission_mysql.public_connection_string(),
        ),
        (
            "ignite_operation_result",
            permission_ignite.connection_url(),
        ),
    ] {
        for (operation, statement) in [
            ("transaction-query", "SELECT * FROM blocked.denied_table"),
            (
                "transaction-query-stream",
                "SELECT * FROM blocked.denied_table",
            ),
            (
                "transaction-execute",
                "INSERT INTO blocked.denied_table VALUES (1)",
            ),
        ] {
            let denied = executor
                .invoke_and_await_agent(
                    &component,
                    &agent,
                    method,
                    data_value!(operation, address.clone(), statement),
                )
                .await?
                .into_typed::<Result<String, String>>()?;
            assert!(
                denied
                    .as_ref()
                    .is_err_and(|error| error.to_ascii_lowercase().contains("permission")),
                "{method} {operation} must return a typed permission denial, got {denied:?}"
            );
        }
        for operation in ["transaction-commit", "transaction-rollback"] {
            let allowed = executor
                .invoke_and_await_agent(
                    &component,
                    &agent,
                    method,
                    data_value!(operation, address.clone(), ""),
                )
                .await?
                .into_typed::<Result<String, String>>()?;
            assert_eq!(
                allowed,
                Ok(operation.trim_start_matches("transaction-").to_string()),
                "{method} {operation} must inherit the admitted transaction"
            );
        }
    }
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn blobstore_authorization_preserves_valid_utf8_container_names(
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
    let agent = agent_id!("BlobStore", "utf8-container-name");
    configure_scope_card_root(&authority, &component, &agent)?;
    executor.start_agent(&component.id, agent.clone()).await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "blobstore_probe",
            data_value!("create-container", "valid.name", "", "", ""),
        )
        .await?
        .into_typed::<Result<(), String>>()?;

    assert_eq!(result, Ok(()));
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn remaining_host_facing_permission_classes_allow_their_backends(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let environment_state_service = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            create_card_service: Some(Arc::new({
                let authority = authority.clone();
                move || {
                    Arc::new(ScopeCardService {
                        authority: authority.clone(),
                    })
                }
            })),
            environment_state_service: Some(environment_state_service.clone()),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .name("remaining-host-permission-allows")
        .without_default_host_permissions("Environment")
        .without_default_host_permissions("KeyValue")
        .without_default_host_permissions("WasiConfig")
        .without_default_host_permissions("GolemHostApi")
        .with_env(
            "Environment",
            vec![
                ("ALLOWED_ENV".to_string(), "visible".to_string()),
                ("DENIED_ENV".to_string(), "hidden".to_string()),
            ],
        )
        .with_agent_config(
            "WasiConfig",
            vec![
                AgentConfigEntryDto {
                    path: vec!["k1".to_string()],
                    value: serde_json::Value::String("visible".to_string()).into(),
                },
                AgentConfigEntryDto {
                    path: vec!["k2".to_string()],
                    value: serde_json::Value::String("hidden".to_string()).into(),
                },
            ],
        )
        .update_agent_provision_config("Environment", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .push(env_permission("ALLOWED_ENV"));
        })
        .update_agent_provision_config("KeyValue", |config| {
            config.initial_permissions.lower_bound.positive.extend([
                kv_permission("write", "allowed-store", "allowed-key"),
                kv_permission("read", "allowed-store", "allowed-key"),
            ]);
        })
        .update_agent_provision_config("WasiConfig", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .push(config_permission("k1"));
        })
        .try_update_agent_provision_config("GolemHostApi", |config| {
            for grant in [
                "agent(?agent) @ * : view : *",
                "config(?agent) @ * : read : private",
                "oplog(?agent) @ * : read : *",
                "tool(?env/*/*) @ * : invoke : *",
            ] {
                config
                    .initial_permissions
                    .lower_bound
                    .positive
                    .push(parse_polymorphic_permission(grant)?);
            }
            Ok::<_, golem_common::model::card::CardParseError>(())
        })?
        .store()
        .await?;

    let environment = agent_id!("Environment", "allowed-environment");
    configure_scope_card_root(&authority, &component, &environment)?;
    executor
        .start_agent(&component.id, environment.clone())
        .await?;
    for method in ["get_environment", "get_environment_p3"] {
        let visible = executor
            .invoke_and_await_agent(&component, &environment, method, data_value!())
            .await?
            .into_typed::<Result<Vec<(String, String)>, String>>()?
            .expect("environment getter must remain available");
        assert!(
            visible
                .iter()
                .any(|(name, value)| name == "ALLOWED_ENV" && value == "visible")
        );
        assert!(visible.iter().all(|(name, _)| name != "DENIED_ENV"));
    }

    let key_value = agent_id!("KeyValue", "allowed-key-value");
    configure_scope_card_root(&authority, &component, &key_value)?;
    executor
        .start_agent(&component.id, key_value.clone())
        .await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(
                &component,
                &key_value,
                "set_result",
                data_value!("allowed-store", "allowed-key", vec![1u8, 2, 3]),
            )
            .await?
            .into_typed::<Result<(), String>>()?,
        Ok(())
    );
    assert_eq!(
        executor
            .invoke_and_await_agent(
                &component,
                &key_value,
                "get_result",
                data_value!("allowed-store", "allowed-key"),
            )
            .await?
            .into_typed::<Result<Option<Vec<u8>>, String>>()?,
        Ok(Some(vec![1, 2, 3]))
    );

    let config = agent_id!("WasiConfig", "allowed-config");
    configure_scope_card_root(&authority, &component, &config)?;
    executor.start_agent(&component.id, config.clone()).await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &config, "get_result", data_value!("k1"))
            .await?
            .into_typed::<Result<Option<String>, String>>()?,
        Ok(Some("visible".to_string()))
    );

    let host = agent_id!("GolemHostApi", "allowed-oplog-and-tool");
    configure_scope_card_root(&authority, &component, &host)?;
    executor.start_agent(&component.id, host.clone()).await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &host, "read_own_oplog_result", data_value!())
            .await?
            .into_typed::<Result<(), String>>()?,
        Ok(())
    );
    assert_eq!(
        executor
            .invoke_and_await_agent(
                &component,
                &host,
                "search_own_oplog_result",
                data_value!("payload"),
            )
            .await?
            .into_typed::<Result<(), String>>()?,
        Ok(())
    );

    let tool_component_id = ComponentId::new();
    let tool_name = "allowed-tool";
    environment_state_service.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(deployment_state(
            &AgentTypeName("GolemHostApi".to_string()),
            1,
            &[(tool_name, tool_component_id, true)],
        )),
    );
    for method in [
        "tool_rpc_invoke_result",
        "tool_rpc_async_invoke_and_await_result",
        "tool_rpc_invoke_and_await_result",
    ] {
        let tool_result = executor
            .invoke_and_await_agent(
                &component,
                &host,
                method,
                data_value!(tool_name, Vec::<String>::new(), String::new()),
            )
            .await?
            .into_typed::<Result<(), String>>()?;
        assert!(
            tool_result.as_ref().is_err_and(|error| {
                error.contains("RemoteInternalError") && !error.contains("Denied")
            }),
            "an allowed tool call must pass authorization and reach the current invocation backend: {tool_result:?}"
        );
    }
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn protected_host_families_return_typed_default_denials(
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
        .without_default_host_permissions("KeyValue")
        .without_default_host_permissions("BlobStore")
        .without_default_host_permissions("WasiConfig")
        .without_default_host_permissions("Environment")
        .without_default_host_permissions("Networking")
        .without_default_host_permissions("RawWasiHttp")
        .without_default_host_permissions("WebsocketTest")
        .without_default_host_permissions("RelationalDatabases")
        .without_default_host_permissions("GolemHostApi")
        .with_agent_config(
            "WasiConfig",
            vec![
                AgentConfigEntryDto {
                    path: vec!["k1".to_string()],
                    value: serde_json::Value::String("private-1".to_string()).into(),
                },
                AgentConfigEntryDto {
                    path: vec!["k2".to_string()],
                    value: serde_json::Value::String("private-2".to_string()).into(),
                },
            ],
        )
        .with_env(
            "Environment",
            vec![("PRIVATE_ENV".to_string(), "must-not-leak".to_string())],
        )
        .update_agent_provision_config("GolemHostApi", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .retain(|permission| !matches!(permission, PolymorphicPermissionPattern::Agent(_)));
            config.initial_permissions.lower_bound.positive.push(
                parse_polymorphic_permission("agent(?agent) @ * : view : *")
                    .expect("valid self-view permission"),
            );
        })
        .store()
        .await?;

    let key_value = agent_id!("KeyValue", "typed-denial-kv");
    configure_scope_card_root(&authority, &component, &key_value)?;
    executor
        .start_agent(&component.id, key_value.clone())
        .await?;
    let kv_denied = executor
        .invoke_and_await_agent(
            &component,
            &key_value,
            "set_result",
            data_value!("denied-bucket", "denied-key", vec![1u8, 2, 3]),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(kv_denied.is_err(), "KV denial must be a typed error");
    let cache_denied = executor
        .invoke_and_await_agent(
            &component,
            &key_value,
            "cache_set_result",
            data_value!("denied-cache-key", vec![1u8, 2, 3]),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(
        cache_denied.is_err(),
        "cache KV denial must be a typed error"
    );

    let blobstore = agent_id!("BlobStore", "typed-denial-blob");
    configure_scope_card_root(&authority, &component, &blobstore)?;
    executor
        .start_agent(&component.id, blobstore.clone())
        .await?;
    let blob_denied = executor
        .invoke_and_await_agent(
            &component,
            &blobstore,
            "container_exists_result",
            data_value!("denied-container"),
        )
        .await?
        .into_typed::<Result<bool, String>>()?;
    assert!(blob_denied.is_err(), "blob denial must be a typed error");

    let config = agent_id!("WasiConfig", "typed-denial-config");
    configure_scope_card_root(&authority, &component, &config)?;
    executor.start_agent(&component.id, config.clone()).await?;
    let config_denied = executor
        .invoke_and_await_agent(&component, &config, "get_result", data_value!("k1"))
        .await?
        .into_typed::<Result<Option<String>, String>>()?;
    assert!(
        config_denied.is_err(),
        "config denial must be a typed error"
    );

    let environment = agent_id!("Environment", "filtered-environment");
    configure_scope_card_root(&authority, &component, &environment)?;
    executor
        .start_agent(&component.id, environment.clone())
        .await?;
    for method in ["get_environment", "get_environment_p3"] {
        let visible = executor
            .invoke_and_await_agent(&component, &environment, method, data_value!())
            .await?
            .into_typed::<Result<Vec<(String, String)>, String>>()?
            .expect("environment access itself must remain available");
        assert!(visible.iter().all(|(name, _)| name != "PRIVATE_ENV"));
    }

    let networking = agent_id!("Networking", "typed-denial-network");
    configure_scope_card_root(&authority, &component, &networking)?;
    executor
        .start_agent(&component.id, networking.clone())
        .await?;
    let dns_denied = executor
        .invoke_and_await_agent(
            &component,
            &networking,
            "resolve_p3",
            data_value!("localhost"),
        )
        .await?
        .into_typed::<Result<Vec<String>, String>>()?;
    assert!(dns_denied.is_err(), "DNS denial must be a typed error");
    let udp_denied = executor
        .invoke_and_await_agent(
            &component,
            &networking,
            "udp_send_p3",
            data_value!(9u16, vec![1u8]),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(udp_denied.is_err(), "UDP denial must be a typed error");

    let http = agent_id!("RawWasiHttp", "typed-denial-http");
    configure_scope_card_root(&authority, &component, &http)?;
    executor.start_agent(&component.id, http.clone()).await?;
    let http_denied = executor
        .invoke_and_await_agent(
            &component,
            &http,
            "dispatch_result",
            data_value!("127.0.0.1:9"),
        )
        .await?
        .into_typed::<Result<u16, String>>()?;
    assert!(http_denied.is_err(), "HTTP denial must be a typed error");

    let websocket = agent_id!("WebsocketTest", "typed-denial-websocket");
    configure_scope_card_root(&authority, &component, &websocket)?;
    executor
        .start_agent(&component.id, websocket.clone())
        .await?;
    let websocket_denied = executor
        .invoke_and_await_agent(
            &component,
            &websocket,
            "connect_result",
            data_value!("ws://127.0.0.1:9"),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(
        websocket_denied.is_err(),
        "WebSocket denial must be a typed error"
    );

    let rdbms = agent_id!("RelationalDatabases", "typed-denial-rdbms");
    configure_scope_card_root(&authority, &component, &rdbms)?;
    executor.start_agent(&component.id, rdbms.clone()).await?;
    let rdbms_denied = executor
        .invoke_and_await_agent(
            &component,
            &rdbms,
            "postgres_operation_result",
            data_value!(
                "connection-query",
                "postgres://127.0.0.1:9/denied",
                "SELECT * FROM denied_table"
            ),
        )
        .await?
        .into_typed::<Result<String, String>>()?;
    assert!(rdbms_denied.is_err(), "RDBMS denial must be a typed error");
    let mysql_denied = executor
        .invoke_and_await_agent(
            &component,
            &rdbms,
            "mysql_operation_result",
            data_value!(
                "connection-query",
                "mysql://127.0.0.1:9/denied",
                "SELECT * FROM denied_table"
            ),
        )
        .await?
        .into_typed::<Result<String, String>>()?;
    assert!(mysql_denied.is_err(), "MySQL denial must be a typed error");
    let ignite_denied = executor
        .invoke_and_await_agent(
            &component,
            &rdbms,
            "ignite_operation_result",
            data_value!(
                "connection-query",
                "ignite://127.0.0.1:9",
                "SELECT * FROM denied_table"
            ),
        )
        .await?
        .into_typed::<Result<String, String>>()?;
    assert!(
        ignite_denied.is_err(),
        "Ignite denial must be a typed error"
    );

    let rpc = agent_id!("GolemHostApi", "typed-denial-rpc");
    configure_scope_card_root(&authority, &component, &rpc)?;
    executor.start_agent(&component.id, rpc.clone()).await?;
    let rpc_denied = executor
        .invoke_and_await_agent(
            &component,
            &rpc,
            "outbound_agent_rpc_invoke_result",
            data_value!("ScopeCardAgent", "never-started", "wallet_card_count"),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(
        rpc_denied.is_err(),
        "agent RPC denial must be a typed error"
    );
    let oplog_denied = executor
        .invoke_and_await_agent(&component, &rpc, "read_own_oplog_result", data_value!())
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(oplog_denied.is_err(), "oplog denial must be a typed error");

    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn denied_tool_invocation_does_not_start_the_tool_component(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let environment_state_service = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            create_card_service: Some(Arc::new({
                let authority = authority.clone();
                move || {
                    Arc::new(ScopeCardService {
                        authority: authority.clone(),
                    })
                }
            })),
            environment_state_service: Some(environment_state_service.clone()),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .without_default_host_permissions("GolemHostApi")
        .update_agent_provision_config("GolemHostApi", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .extend(scope_card_initial_permissions());
        })
        .store()
        .await?;
    let tool_component_id = ComponentId::new();
    let tool_name = "scope-tool";
    environment_state_service.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(deployment_state(
            &AgentTypeName("GolemHostApi".to_string()),
            1,
            &[(tool_name, tool_component_id, true)],
        )),
    );
    let agent = agent_id!("GolemHostApi", "denied-tool-invocation");
    configure_scope_card_root(&authority, &component, &agent)?;
    executor.start_agent(&component.id, agent.clone()).await?;

    assert!(
        executor
            .get_running_workers_metadata(&tool_component_id, None)
            .await?
            .is_empty()
    );
    let denied = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "tool_rpc_invoke_and_await_result",
            data_value!(tool_name, Vec::<String>::new(), String::new()),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(
        denied.as_ref().is_err_and(|error| error.contains("Denied")),
        "unexpected tool invocation result: {denied:?}"
    );
    assert!(
        executor
            .get_running_workers_metadata(&tool_component_id, None)
            .await?
            .is_empty(),
        "denied tool invocation must not activate the tool component"
    );
    assert_eq!(authority.check_cards_count(), 1);
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn secret_reveal_authorizes_before_secret_revision_lookup(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let environment_state_service = Arc::new(TestEnvironmentStateService::default());
    let secret_path = CanonicalAgentSecretPath(vec!["secretPath".to_string()]);
    let secret_value = "secret-value-that-must-not-leak";
    environment_state_service.set_agent_secret(AgentSecret {
        id: AgentSecretId::new(),
        environment_id: context.default_environment_id,
        path: secret_path,
        revision: AgentSecretRevision::INITIAL,
        secret_type: SchemaGraph::anonymous(SchemaType::string()),
        secret_value: Some(SchemaValue::String(secret_value.to_string())),
    });
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            create_card_service: Some(Arc::new({
                let authority = authority.clone();
                move || {
                    Arc::new(ScopeCardService {
                        authority: authority.clone(),
                    })
                }
            })),
            environment_state_service: Some(environment_state_service.clone()),
            ..Default::default()
        },
    )
    .await?;

    let denied_component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .unique()
        .without_default_host_permissions("SecretHandleAgent")
        .update_agent_provision_config("SecretHandleAgent", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .extend([config_permission("*"), secret_permission("hold", "*")]);
        })
        .store()
        .await?;
    let denied_agent = agent_id!("SecretHandleAgent", "denied-secret-reveal");
    configure_scope_card_root(&authority, &denied_component, &denied_agent)?;
    executor
        .start_agent(&denied_component.id, denied_agent.clone())
        .await?;
    authority.reset_check_cards_count();

    let denied = executor
        .invoke_and_await_agent(
            &denied_component,
            &denied_agent,
            "reveal_secret_result",
            data_value!(),
        )
        .await?
        .into_typed::<Result<String, String>>()?;
    assert!(
        denied
            .as_ref()
            .is_err_and(|error| { error.contains("Unavailable") && !error.contains(secret_value) }),
        "unexpected secret denial: {denied:?}"
    );
    assert_eq!(
        authority.check_cards_count(),
        0,
        "stable secret authorization must not refresh card authority"
    );
    assert_eq!(environment_state_service.agent_secret_revision_calls(), 0);

    let allowed_component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .unique()
        .without_default_host_permissions("SecretHandleAgent")
        .update_agent_provision_config("SecretHandleAgent", |config| {
            config.initial_permissions.lower_bound.positive.extend([
                config_permission("*"),
                secret_permission("hold", "*"),
                secret_permission("reveal", "secretPath"),
            ]);
        })
        .store()
        .await?;
    let allowed_agent = agent_id!("SecretHandleAgent", "allowed-secret-reveal");
    configure_scope_card_root(&authority, &allowed_component, &allowed_agent)?;
    executor
        .start_agent(&allowed_component.id, allowed_agent.clone())
        .await?;
    authority.reset_check_cards_count();

    let allowed = executor
        .invoke_and_await_agent(
            &allowed_component,
            &allowed_agent,
            "reveal_secret_result",
            data_value!(),
        )
        .await?
        .into_typed::<Result<String, String>>()?;
    assert_eq!(allowed, Ok(secret_value.to_string()));
    assert_eq!(
        authority.check_cards_count(),
        0,
        "stable secret authorization must not refresh card authority"
    );
    assert_eq!(environment_state_service.agent_secret_revision_calls(), 1);
    Ok(())
}

#[test]
#[timeout("3m")]
#[tracing::instrument]
async fn snapshot_restores_admitted_secret_handle_without_reauthorization(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let environment_state_service = Arc::new(TestEnvironmentStateService::default());
    let secret_value = "snapshot-secret-value";
    environment_state_service.set_agent_secret(AgentSecret {
        id: AgentSecretId::new(),
        environment_id: context.default_environment_id,
        path: CanonicalAgentSecretPath(vec!["secretPath".to_string()]),
        revision: AgentSecretRevision::INITIAL,
        secret_type: SchemaGraph::anonymous(SchemaType::string()),
        secret_value: Some(SchemaValue::String(secret_value.to_string())),
    });
    let make_overrides = || TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.oplog.default_snapshotting = SnapshotPolicy::EveryNInvocation { count: 1 };
        })),
        create_card_service: Some(Arc::new({
            let authority = authority.clone();
            move || {
                Arc::new(ScopeCardService {
                    authority: authority.clone(),
                })
            }
        })),
        environment_state_service: Some(environment_state_service.clone()),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, make_overrides()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .unique()
        .without_default_host_permissions("SecretHandleAgent")
        .update_agent_provision_config("SecretHandleAgent", |config| {
            config.initial_permissions.lower_bound.positive.extend([
                config_permission("*"),
                secret_permission("hold", "*"),
                secret_permission("reveal", "secretPath"),
            ]);
        })
        .store()
        .await?;
    let agent = agent_id!("SecretHandleAgent", "snapshot-secret-handle");
    configure_scope_card_root(&authority, &component, &agent)?;
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;
    authority.reset_check_cards_count();

    let before_restart = executor
        .invoke_and_await_agent(&component, &agent, "reveal_secret_result", data_value!())
        .await?
        .into_typed::<Result<String, String>>()?;
    assert_eq!(before_restart, Ok(secret_value.to_string()));
    assert_eq!(
        authority.check_cards_count(),
        0,
        "stable secret authorization must not refresh card authority"
    );
    assert_eq!(environment_state_service.agent_secret_revision_calls(), 1);
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(
        oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Snapshot(_))),
        "secret-handle agent must have a recovery snapshot before restart"
    );

    drop(executor);
    let executor = start_with_overrides(deps, &context, make_overrides()).await?;
    let mut events = executor.capture_output(&worker_id).await?;
    authority.reset_check_cards_count();
    let after_restart = executor
        .invoke_and_await_agent(&component, &agent, "reveal_secret_result", data_value!())
        .await?
        .into_typed::<Result<String, String>>()?;
    assert_snapshot_recovery_loaded(&mut events).await;

    assert_eq!(after_restart, Ok(secret_value.to_string()));
    assert_eq!(
        authority.check_cards_count(),
        1,
        "snapshot reconstruction must not perform an extra authorization"
    );
    assert_eq!(environment_state_service.agent_secret_revision_calls(), 2);
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn golem_host_agent_operations_are_typed_default_deny_and_allow_when_granted(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;

    let denied_component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .name("gated-host-api-denied")
        .without_default_host_permissions("GolemHostApi")
        .update_agent_provision_config("GolemHostApi", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .retain(|permission| !matches!(permission, PolymorphicPermissionPattern::Agent(_)));
            config.initial_permissions.lower_bound.positive.push(
                parse_polymorphic_permission("agent(?agent) @ * : view : *")
                    .expect("valid self-view permission"),
            );
        })
        .store()
        .await?;
    let denied_agent = agent_id!("GolemHostApi", "host-operation-denied");
    configure_scope_card_root(&authority, &denied_component, &denied_agent)?;
    executor
        .start_agent(&denied_component.id, denied_agent.clone())
        .await?;

    let denied_get_next = executor
        .invoke_and_await_agent(
            &denied_component,
            &denied_agent,
            "get_agents_next_result",
            data_value!(denied_component.id),
        )
        .await?
        .into_typed::<Result<u64, String>>()?;
    assert!(
        denied_get_next.is_err(),
        "get-next must return a typed denial"
    );
    let denied_self_metadata = executor
        .invoke_and_await_agent(
            &denied_component,
            &denied_agent,
            "get_self_metadata_result",
            data_value!(),
        )
        .await?
        .into_typed::<Result<String, String>>()?;
    assert_eq!(denied_self_metadata, Ok(denied_agent.to_string()));
    let denied_strict = executor
        .invoke_and_await_agent(
            &denied_component,
            &denied_agent,
            "resolve_agent_id_strict_result",
            data_value!("gated-host-api-denied", "GolemHostApi(\"other\")"),
        )
        .await?
        .into_typed::<Result<bool, String>>()?;
    assert_eq!(
        denied_strict,
        Ok(false),
        "legacy strict resolution must map denial to none"
    );
    let workers_before_denied_fork = executor
        .get_running_workers_metadata(&denied_component.id, None)
        .await?
        .len();
    let denied_fork = executor
        .invoke_and_await_agent(
            &denied_component,
            &denied_agent,
            "self_fork_result",
            data_value!(),
        )
        .await?
        .into_typed::<Result<String, String>>()?;
    assert!(denied_fork.is_err(), "fork must return a typed denial");
    assert_eq!(
        executor
            .get_running_workers_metadata(&denied_component.id, None)
            .await?
            .len(),
        workers_before_denied_fork,
        "fork authorization must run before creating the forked agent"
    );

    let allowed_component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .unique()
        .name("gated-host-api-allowed")
        .without_default_host_permissions("GolemHostApi")
        .try_update_agent_provision_config("GolemHostApi", |config| {
            for grant in [
                "agent(?env/*/*) @ * : view : *",
                "agent(?agent) @ * : fork :",
            ] {
                config
                    .initial_permissions
                    .lower_bound
                    .positive
                    .push(parse_polymorphic_permission(grant)?);
            }
            Ok::<_, golem_common::model::card::CardParseError>(())
        })?
        .store()
        .await?;
    let allowed_agent = agent_id!("GolemHostApi", "host-operation-allowed");
    configure_scope_card_root(&authority, &allowed_component, &allowed_agent)?;
    executor
        .start_agent(&allowed_component.id, allowed_agent.clone())
        .await?;

    assert!(
        executor
            .invoke_and_await_agent(
                &allowed_component,
                &allowed_agent,
                "get_agents_next_result",
                data_value!(allowed_component.id),
            )
            .await?
            .into_typed::<Result<u64, String>>()?
            .is_ok()
    );
    assert_eq!(
        executor
            .invoke_and_await_agent(
                &allowed_component,
                &allowed_agent,
                "get_self_metadata_result",
                data_value!(),
            )
            .await?
            .into_typed::<Result<String, String>>()?,
        Ok(allowed_agent.to_string())
    );
    assert_eq!(
        executor
            .invoke_and_await_agent(
                &allowed_component,
                &allowed_agent,
                "resolve_agent_id_strict_result",
                data_value!("gated-host-api-allowed", allowed_agent.to_string()),
            )
            .await?
            .into_typed::<Result<bool, String>>()?,
        Ok(false)
    );
    let allowed_fork = executor
        .invoke_and_await_agent(
            &allowed_component,
            &allowed_agent,
            "self_fork_result",
            data_value!(),
        )
        .await?
        .into_typed::<Result<String, String>>()?;
    assert!(
        !allowed_fork
            .as_ref()
            .is_err_and(|error| error.contains("PermissionDenied")),
        "fork grant must pass authorization: {allowed_fork:?}"
    );
    Ok(())
}

async fn assert_environment_observes_revocation_at_the_next_host_boundary(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    p3: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .without_default_host_permissions("Environment")
        .store()
        .await?;
    let agent = agent_id!(
        "Environment",
        if p3 {
            "p3-revocation-boundary"
        } else {
            "p2-revocation-boundary"
        }
    );
    let root_card_id = configure_scope_card_root(&authority, &component, &agent)?;
    let worker = executor.start_agent(&component.id, agent.clone()).await?;
    let release = executor
        .invoke_and_await_agent(&component, &agent, "create_release_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let key = IdempotencyKey::fresh();
    let params = data_value!(release.clone(), p3);

    executor
        .invoke_agent_with_key(
            &component,
            &agent,
            &key,
            "get_environment_after_promise",
            params.clone(),
        )
        .await?;
    executor
        .wait_for_status(&worker, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;
    authority.revoke();
    executor
        .queue_card_revocation(&worker, root_card_id)
        .await?;
    executor.complete_promise(&release, vec![1]).await?;

    let environment = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent,
            &key,
            "get_environment_after_promise",
            params,
        )
        .await?
        .into_typed::<Result<Vec<(String, String)>, String>>()?
        .map_err(anyhow::Error::msg)?;
    assert!(
        environment.iter().all(|(name, _)| name != "GOLEM_AGENT_ID"),
        "revoked environment authority must disappear at the next host boundary"
    );
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn p2_and_p3_environment_observe_revocation_at_the_next_host_boundary(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_environment_observes_revocation_at_the_next_host_boundary(
        last_unique_id,
        deps,
        host_api_tests,
        false,
    )
    .await?;
    assert_environment_observes_revocation_at_the_next_host_boundary(
        last_unique_id,
        deps,
        host_api_tests,
        true,
    )
    .await
}

async fn assert_environment_replay_uses_the_recorded_filtered_result(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    p3: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let executor = start_scope_card_executor(deps, &context, authority.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .without_default_host_permissions("Environment")
        .store()
        .await?;
    let agent = agent_id!(
        "Environment",
        if p3 {
            "p3-environment-replay"
        } else {
            "p2-environment-replay"
        }
    );
    configure_scope_card_root(&authority, &component, &agent)?;
    let worker = executor.start_agent(&component.id, agent.clone()).await?;
    let release = executor
        .invoke_and_await_agent(&component, &agent, "create_release_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let key = IdempotencyKey::fresh();
    let params = data_value!(release.clone(), p3);

    executor
        .invoke_agent_with_key(
            &component,
            &agent,
            &key,
            "get_environment_before_promise",
            params.clone(),
        )
        .await?;
    executor
        .wait_for_status(&worker, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;
    executor.check_oplog_is_queryable(&worker).await?;
    authority.revoke();
    drop(executor);

    let executor = start_scope_card_executor(deps, &context, authority).await?;
    executor.complete_promise(&release, vec![1]).await?;
    let environment = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent,
            &key,
            "get_environment_before_promise",
            params,
        )
        .await?
        .into_typed::<Result<Vec<(String, String)>, String>>()?
        .map_err(anyhow::Error::msg)?;
    assert!(
        environment
            .iter()
            .any(|(name, value)| { name == "GOLEM_AGENT_ID" && value == &agent.to_string() }),
        "replay must return the environment response recorded before revocation"
    );
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn p2_and_p3_environment_replay_uses_the_recorded_filtered_result(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_environment_replay_uses_the_recorded_filtered_result(
        last_unique_id,
        deps,
        host_api_tests,
        false,
    )
    .await?;
    assert_environment_replay_uses_the_recorded_filtered_result(
        last_unique_id,
        deps,
        host_api_tests,
        true,
    )
    .await
}
