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

use crate::repo::{Deps, TestDb, test_environment_default_card_record};
use assert2::{assert, check, let_assert};
use chrono::{Datelike, Utc};
use futures::future::join_all;
use golem_common::base_model::Empty;
use golem_common::base_model::agent::{AgentMode, AgentTypeName, Snapshotting};
use golem_common::base_model::component_metadata::KnownExports;
use golem_common::model::account::{AccountId, AccountRevision, AccountSetPlan};
use golem_common::model::account_usage::{
    AccountUsagePeriod, AdminResourceGrantDimension, AdminResourceGrantEventType,
    AdminResourceGrantReason, EFFECTIVELY_UNLIMITED_STORAGE_LIMIT, MonthlyUsageMode,
    MonthlyUsageModeTransitionSource,
};
use golem_common::model::agent_secret::{
    AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
};
use golem_common::model::application::{ApplicationId, ApplicationName};
use golem_common::model::card::owner::{ComponentOwnerPattern, EnvironmentOwnerPattern};
use golem_common::model::card::{
    Card, CardId, CardManagedBy, CardManagedByAccountRoot, CardManagedByAgentInitial,
    CardManagedByRuntimeDerived, PermissionPattern, PolymorphicCard, StoredCard,
};
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::component_metadata::{AgentTypeProvisionConfig, ComponentMetadata};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::{
    EnvironmentCreation, EnvironmentId, EnvironmentName, EnvironmentUpdate,
};
use golem_common::model::http_api_deployment::HttpApiDeploymentAgentOptions;
use golem_common::model::json::NormalizedJsonValue;
use golem_common::model::plan::PlanId;
use golem_common::model::tool::{
    CompiledToolBinding, RegisteredTool, SecretKeyScope, TOOL_METADATA_WIT_VERSION,
    ToolDeploymentMetadata, ToolName, ToolProvisionConfig, ToolSource,
};
use golem_common::model::{AgentId, IdempotencyKey, OplogIndex};
use golem_common::schema::tool::{CommandNode, CommandTree, Doc, Globals, Tool};
use golem_common::schema::{AgentConstructorSchema, AgentTypeSchema, InputSchema, SchemaGraph};
use golem_registry_service::config::RegistryServiceConfig;
use golem_registry_service::repo::account::DbAccountRepo;
use golem_registry_service::repo::account_resource_override::{
    AdminResourceGrantRepoError, OverridePolicyViolation, SetAccountResourceOverrideError,
};
use golem_registry_service::repo::account_usage::{DbAccountUsageRepo, SetMonthlyUsageModeError};
use golem_registry_service::repo::application::DbApplicationRepo;
use golem_registry_service::repo::card::{CardRepo, DbCardRepo};
use golem_registry_service::repo::component::{ComponentRepo, DbComponentRepo};
use golem_registry_service::repo::deployment::DbDeploymentRepo;
use golem_registry_service::repo::environment::{
    DbEnvironmentRepo, EnvironmentExtRevisionRecord, EnvironmentRevisionRecord,
    EnvironmentVisibilityFilter, EnvironmentVisibilityScope,
};
use golem_registry_service::repo::model::account::{
    AccountExtRevisionRecord, AccountRepoError, AccountRevisionRecord,
};
use golem_registry_service::repo::model::account_resource_override::{
    AccountResourceOverrideDimension, AccountResourceOverrideReason, AccountResourceOverrideRecord,
    AccountResourceOverrideSource,
};
use golem_registry_service::repo::model::account_usage::{
    MonthlyUsageAttribution, UsageTracking, UsageType,
};
use golem_registry_service::repo::model::agent_secrets::{
    AgentSecretCreationRecord, AgentSecretRevisionRecord,
};
use golem_registry_service::repo::model::application::{
    ApplicationExtRevisionRecord, ApplicationRepoError, ApplicationRevisionRecord,
};
use golem_registry_service::repo::model::audit::{
    DeletableRevisionAuditFields, ImmutableAuditFields,
};
use golem_registry_service::repo::model::card::CardRecord;
use golem_registry_service::repo::model::component::{ComponentRepoError, ComponentRevisionRecord};
use golem_registry_service::repo::model::deployment::{
    DeploymentAgentToolBindingRecord, DeploymentComponentRevisionRecord,
    DeploymentRegisteredAgentTypeRecord, DeploymentRegisteredToolRecord,
    DeploymentRevisionCreationRecord,
};
use golem_registry_service::repo::model::environment::EnvironmentRepoError;
use golem_registry_service::repo::model::hash::SqlBlake3Hash;
use golem_registry_service::repo::model::http_api_deployment::{
    HttpApiDeploymentData, HttpApiDeploymentRepoError, HttpApiDeploymentRevisionRecord,
};
use golem_registry_service::repo::model::mcp_deployment::{
    McpDeploymentData, McpDeploymentRepoError, McpDeploymentRevisionRecord,
};
use golem_registry_service::repo::model::new_repo_uuid;
use golem_registry_service::repo::model::plan::PlanRecord;
use golem_registry_service::repo::model::plugin::PluginRecord;
use golem_registry_service::repo::permission_share::DbPermissionShareRepo;
use golem_registry_service::repo::plan::{DbPlanRepo, PlanRepo};
use golem_registry_service::repo::plugin::DbPluginRepo;
use golem_registry_service::repo::registry_change::{
    ChangeEventId, NewRegistryChangeEvent, RegistryChangeEvent,
};
use golem_registry_service::services::component_object_store::ComponentObjectStore;
use golem_registry_service::services::registry_change_notifier::RequiresNotificationSignalExt;
use golem_registry_service::services::registry_change_notifier::{
    RegistryChangeNotifier, SqliteRegistryChangeNotifier,
};
use golem_registry_service::services::{
    account::AccountService,
    account_usage::{AccountUsageService, ResourceUsageUpdate},
    application::ApplicationService,
    card::{AccountCardFilter, CardError, CardService},
    component::ComponentService,
    deployment::DeploymentService,
    environment::{EnvironmentError, EnvironmentService},
    permission_share::PermissionShareService,
    plan::PlanService,
};
use golem_service_base::clients::registry::ResourceUsageMetering;
use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi};
use golem_service_base::db::{postgres::PostgresPool, sqlite::SqlitePool};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::repo::Blob;
use golem_service_base::repo::SqlDateTime;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use heck::ToKebabCase;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::default::Default;
use std::sync::Arc;
use strum::IntoEnumIterator;
use uuid::Uuid;
// Common test cases -------------------------------------------------------------------------------

fn runtime_card(card_id: CardId, parent_ids: Vec<CardId>) -> StoredCard {
    StoredCard::Concrete(Card {
        card_id,
        parent_ids,
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: chrono::DateTime::from_timestamp(1_700_000_000, 123_456_789).unwrap(),
        expires_at: Some(chrono::DateTime::from_timestamp(1_800_000_000, 987_654_321).unwrap()),
        system_card: false,
        managed_by: None,
    })
}

fn runtime_card_provenance() -> CardManagedByRuntimeDerived {
    CardManagedByRuntimeDerived {
        environment_id: EnvironmentId::new(),
        agent_id: AgentId {
            component_id: ComponentId::new(),
            agent_id: "runtime-card-source".to_string(),
        },
        invocation_key: IdempotencyKey::new("runtime-card-invocation".to_string()),
        oplog_index: OplogIndex::from_u64(42),
    }
}

pub async fn test_runtime_card_creation_is_idempotent_and_rejects_conflicts(deps: &Deps) {
    let services = environment_service_deps(deps);
    let parent_ids = vec![CardId::new(), CardId::new()];
    let provenance = runtime_card_provenance();
    for parent_id in &parent_ids {
        services
            .card_service
            .create_runtime_card(
                runtime_card(*parent_id, Vec::new()),
                provenance.clone(),
                &AuthCtx::System,
            )
            .await
            .unwrap();
    }

    let card_id = CardId::new();
    let card = runtime_card(card_id, parent_ids.clone());

    let created = services
        .card_service
        .create_runtime_card(card.clone(), provenance.clone(), &AuthCtx::System)
        .await
        .unwrap();
    let retried_from_original_request = services
        .card_service
        .create_runtime_card(card.clone(), provenance.clone(), &AuthCtx::System)
        .await
        .unwrap();
    let retried = services
        .card_service
        .create_runtime_card(created.clone(), provenance.clone(), &AuthCtx::System)
        .await
        .unwrap();
    let mut reordered = card.clone().into_concrete().unwrap();
    reordered.parent_ids.reverse();
    let retried_with_reordered_parents = services
        .card_service
        .create_runtime_card(reordered.into(), provenance.clone(), &AuthCtx::System)
        .await
        .unwrap();

    assert_eq!(created, retried_from_original_request);
    assert_eq!(created, retried);
    assert_eq!(created, retried_with_reordered_parents);
    let mut conflicting = card.clone().into_concrete().unwrap();
    conflicting.expires_at = None;
    assert!(matches!(
        services
            .card_service
            .create_runtime_card(conflicting.into(), provenance.clone(), &AuthCtx::System)
            .await,
        Err(CardError::RuntimeCardConflict(id)) if id == card_id
    ));

    assert_eq!(
        services
            .card_service
            .revoke_card(card_id, &AuthCtx::System)
            .await
            .unwrap(),
        vec![card_id]
    );
    assert!(matches!(
        services
            .card_service
            .create_runtime_card(card, provenance, &AuthCtx::System)
            .await,
        Err(CardError::RuntimeCardRevoked(id)) if id == card_id
    ));
}

pub async fn test_runtime_card_creation_rejects_system_cards(deps: &Deps) {
    let services = environment_service_deps(deps);
    let card_id = CardId::new();
    let mut card = runtime_card(card_id, Vec::new()).into_concrete().unwrap();
    card.system_card = true;

    assert!(matches!(
        services
            .card_service
            .create_runtime_card(card.into(), runtime_card_provenance(), &AuthCtx::System)
            .await
            .unwrap_err(),
        CardError::RuntimeCardCannotBeSystemCard
    ));
}

pub async fn test_runtime_card_creation_requires_system_auth(deps: &Deps) {
    let services = environment_service_deps(deps);
    let auth = AuthCtx::agent_with_effective_surface(
        AccountId::new(),
        golem_common::model::account::AccountEmail::new("runtime-card-test@example.com"),
        Default::default(),
    );

    assert!(matches!(
        services
            .card_service
            .create_runtime_card(
                runtime_card(CardId::new(), Vec::new()),
                runtime_card_provenance(),
                &auth,
            )
            .await
            .unwrap_err(),
        CardError::Unauthorized(AuthorizationError::SystemOnlyActionNotAllowed(_))
    ));
}

pub async fn test_runtime_card_parent_integrity(deps: &Deps) {
    let services = environment_service_deps(deps);
    let card_repo: Arc<dyn CardRepo> = match &deps.test_db {
        TestDb::Postgres(pool) => Arc::new(DbCardRepo::new(pool.clone())),
        TestDb::Sqlite(pool) => Arc::new(DbCardRepo::new(pool.clone())),
    };
    let provenance = runtime_card_provenance();
    let parent_ids = (0..4).map(|_| CardId::new()).collect::<Vec<_>>();

    for parent_id in &parent_ids {
        services
            .card_service
            .create_runtime_card(
                runtime_card(*parent_id, Vec::new()),
                provenance.clone(),
                &AuthCtx::System,
            )
            .await
            .unwrap();
    }

    let first_child_id = CardId::new();
    services
        .card_service
        .create_runtime_card(
            runtime_card(first_child_id, vec![parent_ids[0], parent_ids[1]]),
            provenance.clone(),
            &AuthCtx::System,
        )
        .await
        .unwrap();
    let second_child_id = CardId::new();
    services
        .card_service
        .create_runtime_card(
            runtime_card(second_child_id, vec![parent_ids[2], parent_ids[3]]),
            provenance.clone(),
            &AuthCtx::System,
        )
        .await
        .unwrap();

    let notifier = deps.test_registry_change_notifier();
    let first_deleted = card_repo
        .delete(parent_ids[0])
        .await
        .unwrap()
        .signal_new_events_available(notifier.as_ref());
    let second_deleted = card_repo
        .delete(parent_ids[3])
        .await
        .unwrap()
        .signal_new_events_available(notifier.as_ref());
    assert!(first_deleted.contains(&first_child_id));
    assert!(second_deleted.contains(&second_child_id));

    let rejected_card_id = CardId::new();
    let error = services
        .card_service
        .create_runtime_card(
            runtime_card(rejected_card_id, vec![parent_ids[1], parent_ids[3]]),
            provenance,
            &AuthCtx::System,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, CardError::CardNotFound(id) if id == parent_ids[3]));
    assert!(card_repo.get(rejected_card_id).await.unwrap().is_none());
    assert!(card_repo.get(parent_ids[1]).await.unwrap().is_some());

    let self_parented_card_id = CardId::new();
    let error = services
        .card_service
        .create_runtime_card(
            runtime_card(self_parented_card_id, vec![self_parented_card_id]),
            runtime_card_provenance(),
            &AuthCtx::System,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, CardError::CardNotFound(id) if id == self_parented_card_id));
    assert!(
        card_repo
            .get(self_parented_card_id)
            .await
            .unwrap()
            .is_none()
    );

    let expired_parent_id = CardId::new();
    let mut expired_parent = runtime_card(expired_parent_id, Vec::new())
        .into_concrete()
        .unwrap();
    expired_parent.expires_at = Some(chrono::DateTime::from_timestamp(1_600_000_000, 0).unwrap());
    services
        .card_service
        .create_runtime_card(
            expired_parent.into(),
            runtime_card_provenance(),
            &AuthCtx::System,
        )
        .await
        .unwrap();
    let child_of_expired_parent = CardId::new();
    assert!(matches!(
        services
            .card_service
            .create_runtime_card(
                runtime_card(child_of_expired_parent, vec![expired_parent_id]),
                runtime_card_provenance(),
                &AuthCtx::System,
            )
            .await,
        Err(CardError::CardNotFound(id)) if id == expired_parent_id
    ));
    assert!(
        card_repo
            .get(child_of_expired_parent)
            .await
            .unwrap()
            .is_none()
    );
}

pub async fn test_card_dag_traversal_deduplicates_diamonds(deps: &Deps) {
    let services = environment_service_deps(deps);
    let card_repo: Arc<dyn CardRepo> = match &deps.test_db {
        TestDb::Postgres(pool) => Arc::new(DbCardRepo::new(pool.clone())),
        TestDb::Sqlite(pool) => Arc::new(DbCardRepo::new(pool.clone())),
    };
    let provenance = runtime_card_provenance();
    let root = CardId::new();
    let left = CardId::new();
    let right = CardId::new();
    let leaf = CardId::new();

    for (card_id, parent_ids) in [
        (root, Vec::new()),
        (left, vec![root]),
        (right, vec![root]),
        (leaf, vec![left, right]),
    ] {
        services
            .card_service
            .create_runtime_card(
                runtime_card(card_id, parent_ids),
                provenance.clone(),
                &AuthCtx::System,
            )
            .await
            .unwrap();
    }

    let sorted = |mut card_ids: Vec<CardId>| {
        card_ids.sort_unstable();
        card_ids
    };
    let all = sorted(vec![root, left, right, leaf]);

    assert_eq!(
        card_repo.ancestor_ids_including_self(leaf).await.unwrap(),
        all
    );
    assert_eq!(
        card_repo.descendant_ids_including_self(root).await.unwrap(),
        all
    );
    assert_eq!(
        card_repo.descendant_ids_including_self(left).await.unwrap(),
        sorted(vec![left, leaf])
    );
    assert_eq!(
        card_repo.ancestor_ids_including_self(root).await.unwrap(),
        vec![root]
    );

    let missing = CardId::new();
    assert!(
        card_repo
            .ancestor_ids_including_self(missing)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        card_repo
            .descendant_ids_including_self(missing)
            .await
            .unwrap()
            .is_empty()
    );
}

pub async fn test_card_dag_revoke_transaction_rolls_back_atomically(deps: &Deps) {
    let services = environment_service_deps(deps);
    let card_repo: Arc<dyn CardRepo> = match &deps.test_db {
        TestDb::Postgres(pool) => Arc::new(DbCardRepo::new(pool.clone())),
        TestDb::Sqlite(pool) => Arc::new(DbCardRepo::new(pool.clone())),
    };
    let provenance = runtime_card_provenance();
    let root = CardId::new();
    let left = CardId::new();
    let right = CardId::new();
    let leaf = CardId::new();
    let cards = [
        runtime_card(root, Vec::new()),
        runtime_card(left, vec![root]),
        runtime_card(right, vec![root]),
        runtime_card(leaf, vec![left, right]),
    ];

    for card in &cards {
        services
            .card_service
            .create_runtime_card(card.clone(), provenance.clone(), &AuthCtx::System)
            .await
            .unwrap();
    }

    let baseline = deps
        .registry_change_repo
        .get_latest_event_id()
        .await
        .unwrap()
        .unwrap_or(ChangeEventId(0));
    let deleted = match &deps.test_db {
        TestDb::Postgres(pool) => {
            let mut tx = pool
                .with_rw("test", "rollback_card_dag_revoke")
                .begin()
                .await
                .unwrap();
            let deleted = DbCardRepo::<PostgresPool>::delete_tree_in_tx(&mut tx, root)
                .await
                .unwrap();
            tx.rollback().await.unwrap();
            deleted
        }
        TestDb::Sqlite(pool) => {
            let mut tx = pool
                .with_rw("test", "rollback_card_dag_revoke")
                .begin()
                .await
                .unwrap();
            let deleted = DbCardRepo::<SqlitePool>::delete_tree_in_tx(&mut tx, root)
                .await
                .unwrap();
            tx.rollback().await.unwrap();
            deleted
        }
    };
    let mut expected = vec![root, left, right, leaf];
    expected.sort_unstable();

    assert_eq!(deleted, expected);
    assert_eq!(
        card_repo.existing(expected.clone()).await.unwrap(),
        expected
    );
    let events = deps
        .registry_change_repo
        .get_events_since(baseline)
        .await
        .unwrap();
    assert!(events.iter().all(|event| {
        !matches!(
            event,
            RegistryChangeEvent::CardRevoked { card_ids, .. }
                if card_ids.iter().any(|card_id| expected.contains(&CardId(*card_id)))
        )
    }));

    for card in cards {
        assert_eq!(
            services
                .card_service
                .create_runtime_card(card.clone(), provenance.clone(), &AuthCtx::System)
                .await
                .unwrap()
                .card_id(),
            card.card_id()
        );
    }
}

pub async fn test_component_deletion_racing_runtime_derivation(deps: &Deps) {
    let TestDb::Postgres(pool) = &deps.test_db else {
        panic!("this race depends on PostgreSQL row-lock and statement-snapshot semantics");
    };
    let pool = pool.clone();
    let services = environment_service_deps(deps);
    let card_service = Arc::new(services.card_service);
    let component_repo = Arc::new(DbComponentRepo::new(pool.clone()));

    let owner = deps.create_account().await;
    let app = deps.create_application(owner.revision.account_id).await;
    let env = deps.create_env(app.revision.application_id).await;
    let component_id = ComponentId(new_repo_uuid());
    let agent_type = AgentTypeName("agent".to_string());
    let initial_card = PolymorphicCard {
        card_id: CardId(new_repo_uuid()),
        parent_ids: Vec::new(),
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: Utc::now(),
        expires_at: None,
        system_card: false,
    };
    let initial_card_id = initial_card.card_id;
    let metadata = ComponentMetadata::from_parts(
        KnownExports::default(),
        vec![],
        None,
        None,
        vec![],
        BTreeMap::from([(
            agent_type.clone(),
            AgentTypeProvisionConfig {
                initial_permissions: initial_card.clone(),
                env: BTreeMap::new(),
                config: Vec::new(),
                plugins: Vec::new(),
                files: Vec::new(),
            },
        )]),
    );
    component_repo
        .create(
            env.revision.environment_id,
            "component-delete-runtime-race",
            ComponentRevisionRecord {
                component_id: component_id.0,
                revision_id: ComponentRevision::INITIAL.into(),
                hash: SqlBlake3Hash::empty(),
                audit: DeletableRevisionAuditFields::new(owner.revision.account_id),
                size: 0.into(),
                metadata: Blob::new(metadata),
                object_store_key: "component-delete-runtime-race".to_string(),
                binary_hash: SqlBlake3Hash::empty(),
            },
            vec![CardRecord::polymorphic_creation(
                initial_card.clone(),
                Some(CardManagedBy::AgentInitial(CardManagedByAgentInitial {
                    component_id,
                    component_revision: ComponentRevision::INITIAL,
                    agent_type,
                })),
            )],
        )
        .await
        .unwrap();

    let mut second_parent_id = CardId::new();
    while second_parent_id <= initial_card_id {
        second_parent_id = CardId::new();
    }
    let mut provenance = runtime_card_provenance();
    provenance.environment_id = EnvironmentId(env.revision.environment_id);
    card_service
        .create_runtime_card(
            runtime_card(second_parent_id, Vec::new()),
            provenance.clone(),
            &AuthCtx::System,
        )
        .await
        .unwrap();

    let mut blocker = pool
        .with_rw("test", "block_second_runtime_parent")
        .begin()
        .await
        .unwrap();
    blocker
        .execute(
            sqlx::query("SELECT card_id FROM cards WHERE card_id = $1 FOR UPDATE")
                .bind(second_parent_id.0),
        )
        .await
        .unwrap();

    let child_id = CardId::new();
    let create_task = tokio::spawn({
        let card_service = card_service.clone();
        async move {
            card_service
                .create_runtime_card(
                    runtime_card(child_id, vec![initial_card_id, second_parent_id]),
                    provenance,
                    &AuthCtx::System,
                )
                .await
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let delete_task = tokio::spawn({
        let component_repo = component_repo.clone();
        async move {
            component_repo
                .delete(owner.revision.account_id, component_id.0, 1)
                .await
        }
    });

    let mut deletion_is_blocked = false;
    for _ in 0..100 {
        let mut api = pool.with_ro("test", "observe_blocked_component_delete");
        if api
            .fetch_optional(sqlx::query(
                "SELECT 1 FROM pg_stat_activity WHERE wait_event_type = 'Lock' AND query LIKE '%SELECT card_id, data, created_at, expires_at, system_card, managed_by%' AND query LIKE '%FOR UPDATE%'",
            ))
            .await
            .unwrap()
            .is_some()
        {
            deletion_is_blocked = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        deletion_is_blocked,
        "failed to block component deletion on the AgentInitial root held by runtime derivation"
    );

    blocker.commit().await.unwrap();
    create_task.await.unwrap().unwrap();
    match delete_task.await.unwrap() {
        Ok(_) => assert!(
            card_service
                .existing(vec![initial_card_id, child_id])
                .await
                .unwrap()
                .is_empty(),
            "a successful deletion must remove the AgentInitial root and concurrently committed runtime descendant"
        ),
        Err(ComponentRepoError::ConcurrentModification) => {}
        Err(error) => panic!(
            "a create/delete race must either complete atomically or use the component concurrency error category, but returned {error:?}"
        ),
    }
}

pub async fn test_component_deletion_racing_multi_parent_runtime_derivation(deps: &Deps) {
    let TestDb::Postgres(pool) = &deps.test_db else {
        panic!("this race depends on PostgreSQL row-lock semantics");
    };
    let pool = pool.clone();
    let services = environment_service_deps(deps);
    let card_service = Arc::new(services.card_service);
    let component_repo = Arc::new(DbComponentRepo::new(pool.clone()));

    let owner = deps.create_account().await;
    let app = deps.create_application(owner.revision.account_id).await;
    let env = deps.create_env(app.revision.application_id).await;
    let component_id = ComponentId(new_repo_uuid());
    let low_card_id = CardId(Uuid::from_u128(1));
    let high_card_id = CardId(Uuid::from_u128(u128::MAX));
    let high_agent_type = AgentTypeName("a-high-card".to_string());
    let low_agent_type = AgentTypeName("b-low-card".to_string());
    let initial_card = |card_id| PolymorphicCard {
        card_id,
        parent_ids: Vec::new(),
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: Utc::now(),
        expires_at: None,
        system_card: false,
    };
    let high_card = initial_card(high_card_id);
    let low_card = initial_card(low_card_id);
    let provision_config = |card: PolymorphicCard| AgentTypeProvisionConfig {
        initial_permissions: card,
        env: BTreeMap::new(),
        config: Vec::new(),
        plugins: Vec::new(),
        files: Vec::new(),
    };
    let metadata = ComponentMetadata::from_parts(
        KnownExports::default(),
        vec![],
        None,
        None,
        vec![],
        BTreeMap::from([
            (high_agent_type.clone(), provision_config(high_card.clone())),
            (low_agent_type.clone(), provision_config(low_card.clone())),
        ]),
    );
    let card_record = |card: &PolymorphicCard, agent_type| {
        CardRecord::polymorphic_creation(
            card.clone(),
            Some(CardManagedBy::AgentInitial(CardManagedByAgentInitial {
                component_id,
                component_revision: ComponentRevision::INITIAL,
                agent_type,
            })),
        )
    };
    component_repo
        .create(
            env.revision.environment_id,
            "component-delete-multi-parent-runtime-race",
            ComponentRevisionRecord {
                component_id: component_id.0,
                revision_id: ComponentRevision::INITIAL.into(),
                hash: SqlBlake3Hash::empty(),
                audit: DeletableRevisionAuditFields::new(owner.revision.account_id),
                size: 0.into(),
                metadata: Blob::new(metadata),
                object_store_key: "component-delete-multi-parent-runtime-race".to_string(),
                binary_hash: SqlBlake3Hash::empty(),
            },
            vec![
                card_record(&high_card, high_agent_type),
                card_record(&low_card, low_agent_type),
            ],
        )
        .await
        .unwrap();

    let mut blocker = pool
        .with_rw("test", "block_high_runtime_parent")
        .begin()
        .await
        .unwrap();
    blocker
        .execute(
            sqlx::query("SELECT card_id FROM cards WHERE card_id = $1 FOR UPDATE")
                .bind(high_card_id.0),
        )
        .await
        .unwrap();

    let delete_task = tokio::spawn({
        let component_repo = component_repo.clone();
        async move {
            component_repo
                .delete(owner.revision.account_id, component_id.0, 1)
                .await
        }
    });
    let mut deletion_is_blocked = false;
    for _ in 0..100 {
        let mut api = pool.with_ro("test", "observe_multi_parent_component_delete");
        if api
            .fetch_optional(sqlx::query(
                "SELECT 1 FROM pg_stat_activity WHERE wait_event_type = 'Lock' AND query LIKE '%FROM cards%' AND query LIKE '%FOR UPDATE%'",
            ))
            .await
            .unwrap()
            .is_some()
        {
            deletion_is_blocked = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        deletion_is_blocked,
        "failed to block deletion on its first AgentInitial root"
    );

    let child_id = CardId::new();
    let mut provenance = runtime_card_provenance();
    provenance.environment_id = EnvironmentId(env.revision.environment_id);
    let create_task = tokio::spawn({
        let card_service = card_service.clone();
        async move {
            card_service
                .create_runtime_card(
                    runtime_card(child_id, vec![high_card_id, low_card_id]),
                    provenance,
                    &AuthCtx::System,
                )
                .await
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    blocker.commit().await.unwrap();

    let create_result = tokio::time::timeout(std::time::Duration::from_secs(5), create_task)
        .await
        .expect("runtime creation did not finish after PostgreSQL deadlock detection")
        .unwrap();
    let delete_result = tokio::time::timeout(std::time::Duration::from_secs(5), delete_task)
        .await
        .expect("component deletion did not finish after PostgreSQL deadlock detection")
        .unwrap();

    if let Err(error) = &create_result {
        assert!(
            !matches!(error, CardError::InternalError(_)),
            "runtime derivation versus component deletion must never surface an internal error, but returned {error:?}"
        );
    }
    match delete_result {
        Ok(_) => {
            assert!(
                card_service
                    .existing(vec![low_card_id, high_card_id, child_id])
                    .await
                    .unwrap()
                    .is_empty(),
                "a successful deletion must remove both roots and their runtime descendant"
            );
        }
        Err(ComponentRepoError::ConcurrentModification) => {}
        Err(error) => panic!(
            "runtime derivation versus component deletion must use the component concurrency category, but returned {error:?}"
        ),
    }
}

pub async fn test_component_deletion_racing_runtime_derivation_from_descendant(deps: &Deps) {
    let TestDb::Postgres(pool) = &deps.test_db else {
        panic!("this race depends on PostgreSQL row-lock semantics");
    };
    let pool = pool.clone();
    let services = environment_service_deps(deps);
    let card_service = Arc::new(services.card_service);
    let component_repo = Arc::new(DbComponentRepo::new(pool.clone()));

    let owner = deps.create_account().await;
    let app = deps.create_application(owner.revision.account_id).await;
    let env = deps.create_env(app.revision.application_id).await;
    let component_id = ComponentId(new_repo_uuid());
    let root_id = CardId(Uuid::from_u128(u128::MAX));
    let descendant_id = CardId(Uuid::from_u128(2));
    let agent_type = AgentTypeName("agent".to_string());
    let initial_card = PolymorphicCard {
        card_id: root_id,
        parent_ids: Vec::new(),
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: Utc::now(),
        expires_at: None,
        system_card: false,
    };
    let metadata = ComponentMetadata::from_parts(
        KnownExports::default(),
        vec![],
        None,
        None,
        vec![],
        BTreeMap::from([(
            agent_type.clone(),
            AgentTypeProvisionConfig {
                initial_permissions: initial_card.clone(),
                env: BTreeMap::new(),
                config: Vec::new(),
                plugins: Vec::new(),
                files: Vec::new(),
            },
        )]),
    );
    component_repo
        .create(
            env.revision.environment_id,
            "component-delete-descendant-parent-race",
            ComponentRevisionRecord {
                component_id: component_id.0,
                revision_id: ComponentRevision::INITIAL.into(),
                hash: SqlBlake3Hash::empty(),
                audit: DeletableRevisionAuditFields::new(owner.revision.account_id),
                size: 0.into(),
                metadata: Blob::new(metadata),
                object_store_key: "component-delete-descendant-parent-race".to_string(),
                binary_hash: SqlBlake3Hash::empty(),
            },
            vec![CardRecord::polymorphic_creation(
                initial_card,
                Some(CardManagedBy::AgentInitial(CardManagedByAgentInitial {
                    component_id,
                    component_revision: ComponentRevision::INITIAL,
                    agent_type,
                })),
            )],
        )
        .await
        .unwrap();

    let mut provenance = runtime_card_provenance();
    provenance.environment_id = EnvironmentId(env.revision.environment_id);
    card_service
        .create_runtime_card(
            runtime_card(descendant_id, vec![root_id]),
            provenance.clone(),
            &AuthCtx::System,
        )
        .await
        .unwrap();

    let mut blocker = pool
        .with_rw("test", "block_runtime_descendant_parent")
        .begin()
        .await
        .unwrap();
    blocker
        .execute(
            sqlx::query("SELECT card_id FROM cards WHERE card_id = $1 FOR UPDATE")
                .bind(descendant_id.0),
        )
        .await
        .unwrap();

    let child_id = CardId::new();
    let create_task = tokio::spawn({
        let card_service = card_service.clone();
        async move {
            card_service
                .create_runtime_card(
                    runtime_card(child_id, vec![descendant_id, root_id]),
                    provenance,
                    &AuthCtx::System,
                )
                .await
        }
    });
    let mut creation_is_blocked = false;
    for _ in 0..100 {
        let mut api = pool.with_ro("test", "observe_blocked_descendant_parent_create");
        if api
            .fetch_optional(sqlx::query(
                "SELECT 1 FROM pg_stat_activity WHERE wait_event_type = 'Lock' AND query LIKE '%FROM cards%' AND query LIKE '%FOR UPDATE%'",
            ))
            .await
            .unwrap()
            .is_some()
        {
            creation_is_blocked = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        creation_is_blocked,
        "failed to block creation on the descendant"
    );

    let delete_task = tokio::spawn({
        let component_repo = component_repo.clone();
        async move {
            component_repo
                .delete(owner.revision.account_id, component_id.0, 1)
                .await
        }
    });
    let mut deletion_is_blocked = false;
    for _ in 0..100 {
        let mut api = pool.with_ro("test", "observe_descendant_component_delete");
        if api
            .fetch_optional(sqlx::query(
                "SELECT 1 FROM pg_stat_activity WHERE wait_event_type = 'Lock' AND query LIKE '%WITH RECURSIVE to_delete%'",
            ))
            .await
            .unwrap()
            .is_some()
        {
            deletion_is_blocked = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        deletion_is_blocked,
        "failed to block deletion on the runtime descendant"
    );

    blocker.commit().await.unwrap();
    let create_result = tokio::time::timeout(std::time::Duration::from_secs(5), create_task)
        .await
        .expect("runtime creation did not finish after PostgreSQL deadlock detection")
        .unwrap();
    let delete_result = tokio::time::timeout(std::time::Duration::from_secs(5), delete_task)
        .await
        .expect("component deletion did not finish after PostgreSQL deadlock detection")
        .unwrap();

    if let Err(error) = &create_result {
        assert!(
            !matches!(error, CardError::InternalError(_)),
            "runtime derivation versus component deletion must never surface an internal error, but returned {error:?}"
        );
    }
    match delete_result {
        Ok(_) => assert!(
            card_service
                .existing(vec![root_id, descendant_id, child_id])
                .await
                .unwrap()
                .is_empty(),
            "a successful deletion must remove the root and all runtime descendants"
        ),
        Err(ComponentRepoError::ConcurrentModification) => {}
        Err(error) => panic!(
            "runtime derivation versus component deletion must use the component concurrency category, but returned {error:?}"
        ),
    }
}

pub async fn test_environment_deletion_racing_runtime_derivation_from_descendant(deps: &Deps) {
    let TestDb::Postgres(pool) = &deps.test_db else {
        panic!("this race depends on PostgreSQL row-lock and statement-snapshot semantics");
    };
    let pool = pool.clone();
    let services = environment_service_deps(deps);
    let environment_service = services.environment_service.clone();
    let card_service = Arc::new(services.card_service);

    let owner = deps.create_account().await;
    let app = deps.create_application(owner.revision.account_id).await;
    let env = environment_service
        .create(
            ApplicationId(app.revision.application_id),
            EnvironmentCreation {
                name: EnvironmentName("environment-runtime-delete-race".to_string()),
                compatibility_check: false,
                version_check: false,
                security_overrides: false,
            },
            &AuthCtx::System,
        )
        .await
        .unwrap();
    let default_card_id = deps
        .environment_repo
        .get_default_card_ref_by_environment(env.id.0)
        .await
        .unwrap()
        .unwrap()
        .card_id;

    let descendant_id = CardId::new();
    let mut provenance = runtime_card_provenance();
    provenance.environment_id = env.id;
    card_service
        .create_runtime_card(
            runtime_card(descendant_id, vec![default_card_id]),
            provenance.clone(),
            &AuthCtx::System,
        )
        .await
        .unwrap();

    let mut blocker = pool
        .with_rw("test", "block_environment_runtime_descendant_parent")
        .begin()
        .await
        .unwrap();
    blocker
        .execute(
            sqlx::query("SELECT card_id FROM cards WHERE card_id = $1 FOR UPDATE")
                .bind(descendant_id.0),
        )
        .await
        .unwrap();

    let child_id = CardId::new();
    let create_task = tokio::spawn({
        let card_service = card_service.clone();
        async move {
            card_service
                .create_runtime_card(
                    runtime_card(child_id, vec![descendant_id]),
                    provenance,
                    &AuthCtx::System,
                )
                .await
        }
    });
    let mut creation_is_blocked = false;
    for _ in 0..100 {
        let mut api = pool.with_ro("test", "observe_blocked_environment_child_create");
        if api
            .fetch_optional(sqlx::query(
                "SELECT 1 FROM pg_stat_activity WHERE wait_event_type = 'Lock' AND query LIKE '%FROM cards%' AND query LIKE '%FOR UPDATE%'",
            ))
            .await
            .unwrap()
            .is_some()
        {
            creation_is_blocked = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        creation_is_blocked,
        "failed to block runtime creation on the existing descendant"
    );

    let delete_task = tokio::spawn({
        let environment_service = environment_service.clone();
        async move {
            environment_service
                .delete(env.id, env.revision, &AuthCtx::System)
                .await
        }
    });
    let mut deletion_is_blocked = false;
    for _ in 0..100 {
        let mut api = pool.with_ro("test", "observe_blocked_environment_delete");
        if api
            .fetch_optional(sqlx::query(
                "SELECT 1 FROM pg_stat_activity WHERE wait_event_type = 'Lock' AND query LIKE '%WITH RECURSIVE to_delete%'",
            ))
            .await
            .unwrap()
            .is_some()
        {
            deletion_is_blocked = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        deletion_is_blocked,
        "failed to block environment deletion on the runtime descendant"
    );

    blocker.commit().await.unwrap();
    create_task.await.unwrap().unwrap();
    let delete_result = delete_task.await.unwrap();

    match delete_result {
        Ok(()) => assert!(
            card_service
                .existing(vec![default_card_id, descendant_id, child_id])
                .await
                .unwrap()
                .is_empty(),
            "a successful environment deletion must remove the default-card DAG"
        ),
        Err(EnvironmentError::ConcurrentModification) => assert_eq!(
            card_service
                .existing(vec![default_card_id, descendant_id, child_id])
                .await
                .unwrap(),
            vec![default_card_id, descendant_id, child_id],
            "a rejected environment deletion must roll back the revocation boundary"
        ),
        Err(error) => panic!(
            "runtime derivation versus environment deletion must complete atomically or use the environment concurrency category, but returned {error:?}"
        ),
    }
}

pub async fn test_create_and_get_account(deps: &Deps) {
    let account = AccountRevisionRecord {
        account_id: new_repo_uuid(),
        revision_id: 0,
        email: new_repo_uuid().to_string(),
        audit: DeletableRevisionAuditFields::new(new_repo_uuid()),
        name: new_repo_uuid().to_string(),
        roles: 0,
        plan_id: deps.test_plan_id(),
    };

    let created_account = deps
        .account_repo
        .create(account.clone(), test_account_root_card(account.account_id))
        .await
        .unwrap();
    compare_created_to_requested_account(&account, &created_account);

    let result_for_same_email = deps
        .account_repo
        .create(
            AccountRevisionRecord {
                account_id: new_repo_uuid(),
                revision_id: 0,
                email: account.email.clone(),
                audit: DeletableRevisionAuditFields::new(new_repo_uuid()),
                name: new_repo_uuid().to_string(),
                roles: 0,
                plan_id: deps.test_plan_id(),
            },
            test_account_root_card(new_repo_uuid()),
        )
        .await;
    let_assert!(Err(AccountRepoError::AccountViolatesUniqueness) = result_for_same_email);

    let requested_account = deps
        .account_repo
        .get_by_id(account.account_id)
        .await
        .unwrap();
    let_assert!(Some(requested_account) = requested_account);
    compare_created_to_requested_account(&account, &requested_account);

    let requested_account = deps
        .account_repo
        .get_by_email(&account.email)
        .await
        .unwrap();
    let_assert!(Some(requested_account) = requested_account);
    compare_created_to_requested_account(&account, &requested_account);
}

pub async fn test_update(deps: &Deps) {
    let account = AccountRevisionRecord {
        account_id: new_repo_uuid(),
        revision_id: 0,
        email: new_repo_uuid().to_string(),
        audit: DeletableRevisionAuditFields::new(new_repo_uuid()),
        name: new_repo_uuid().to_string(),
        roles: 0,
        plan_id: deps.test_plan_id(),
    };

    let created_account = deps
        .account_repo
        .create(account.clone(), test_account_root_card(account.account_id))
        .await
        .unwrap();
    compare_created_to_requested_account(&account, &created_account);

    let updated_account = AccountRevisionRecord {
        revision_id: 1,
        name: "Updated name".to_string(),
        ..account
    };

    let created_updated_account = deps
        .account_repo
        .update(updated_account.clone())
        .await
        .unwrap();

    compare_created_to_requested_account(&updated_account, &created_updated_account);
}

pub async fn test_application_create(deps: &Deps) {
    let now = Utc::now();
    let owner = deps.create_account().await;
    let user = deps.create_account().await;
    let app_name = format!("app-name-{}", new_repo_uuid());

    let app = deps
        .application_repo
        .get_by_name(owner.revision.account_id, &app_name)
        .await
        .unwrap();
    assert!(app.is_none());

    let app = deps
        .application_repo
        .create(
            owner.revision.account_id,
            ApplicationRevisionRecord {
                application_id: new_repo_uuid(),
                revision_id: 0,
                name: app_name.clone(),
                audit: DeletableRevisionAuditFields::new(user.revision.account_id),
            },
        )
        .await
        .unwrap();

    check!(app.revision.name == app_name);
    check!(app.account_id == owner.revision.account_id);
    check!(app.revision.audit.created_by == user.revision.account_id);
    check!(app.revision.audit.created_at.as_utc() >= &now);
    check!(app.entity_created_at == app.revision.audit.created_at);
    check!(!app.revision.audit.deleted);

    let app_2 = deps
        .application_repo
        .get_by_name(owner.revision.account_id, &app_name)
        .await
        .unwrap();
    let_assert!(Some(app_2) = app_2);

    check!(app.account_id == app_2.account_id);
    check!(app.entity_created_at == app_2.entity_created_at);
    check!(app.revision == app_2.revision);
}

pub async fn test_application_create_concurrent(deps: &Deps) {
    let owner = deps.create_account().await;
    let user = deps.create_account().await;
    let app_name = format!("app-name-{}", new_repo_uuid());
    let concurrency = 20;

    let results = join_all(
        (0..concurrency)
            .map(|_| async {
                deps.application_repo
                    .create(
                        owner.revision.account_id,
                        ApplicationRevisionRecord {
                            application_id: new_repo_uuid(),
                            revision_id: 0,
                            name: app_name.clone(),
                            audit: DeletableRevisionAuditFields::new(user.revision.account_id),
                        },
                    )
                    .await
            })
            .collect::<Vec<_>>(),
    )
    .await;

    assert_eq!(results.len(), concurrency);
    let created = results.iter().filter(|result| result.is_ok()).count();
    let skipped = results
        .iter()
        .filter(|result| {
            matches!(
                result,
                Err(ApplicationRepoError::ApplicationViolatesUniqueness)
            )
        })
        .count();
    check!(created == 1);
    check!(skipped == concurrency - 1);
}

pub async fn test_application_delete(deps: &Deps) {
    let user = deps.create_account().await;
    let app = deps.create_application(user.revision.account_id).await;

    let deleted_app = ApplicationRevisionRecord {
        revision_id: app.revision.revision_id + 1,
        ..app.revision.clone()
    };

    let _ = deps
        .application_repo
        .delete(deleted_app.clone())
        .await
        .unwrap();

    let get_by_id = deps
        .application_repo
        .get_by_id(app.revision.application_id)
        .await
        .unwrap();
    assert!(get_by_id.is_none());
    let get_by_name = deps
        .application_repo
        .get_by_name(user.revision.account_id, &app.revision.name)
        .await
        .unwrap();
    assert!(get_by_name.is_none());

    // Delete app again, should fail
    {
        let result = deps.application_repo.delete(deleted_app).await;
        assert!(let Err(ApplicationRepoError::ConcurrentModification) = result);
    }

    let new_app_with_same_name = deps
        .application_repo
        .create(
            user.revision.account_id,
            ApplicationRevisionRecord {
                application_id: new_repo_uuid(),
                revision_id: 0,
                name: app.revision.name.clone(),
                audit: DeletableRevisionAuditFields::new(user.revision.account_id),
            },
        )
        .await
        .unwrap();

    check!(new_app_with_same_name.revision.name == app.revision.name);
    check!(new_app_with_same_name.revision.application_id != app.revision.application_id);
}

pub async fn test_environment_create(deps: &Deps) {
    let user = deps.create_account().await;
    let app = deps.create_application(user.revision.account_id).await;

    let env_name = "local";

    assert!(
        deps.environment_repo
            .get_by_name(app.revision.application_id, env_name)
            .await
            .unwrap()
            .is_none()
    );

    let revision_0 = EnvironmentRevisionRecord {
        environment_id: new_repo_uuid(),
        name: env_name.to_string(),
        revision_id: 0,
        audit: DeletableRevisionAuditFields::new(user.revision.account_id),
        compatibility_check: false,
        version_check: false,
        security_overrides: false,
        hash: SqlBlake3Hash::empty(),
    }
    .with_updated_hash()
    .unwrap();

    let env = deps
        .environment_repo
        .create(
            app.revision.application_id,
            revision_0.clone(),
            test_environment_default_card_record(revision_0.environment_id),
        )
        .await
        .unwrap();

    check!(env.application_id == app.revision.application_id);
    check!(env.revision == revision_0);

    let env_by_name = deps
        .environment_repo
        .get_by_name(app.revision.application_id, env_name)
        .await
        .unwrap();
    let_assert!(Some(env_by_name) = env_by_name);
    check!(env.application_id == env_by_name.application_id);
    check!(env.revision == env_by_name.revision);

    let env_by_id = deps
        .environment_repo
        .get_by_id(env.revision.environment_id, false)
        .await
        .unwrap();
    let_assert!(Some(env_by_id) = env_by_id);
    check!(env.application_id == env_by_id.application_id);
    check!(env.revision == env_by_id.revision);
}

pub async fn test_environment_list_visible_to_account_uses_visibility_filter(deps: &Deps) {
    let owner_1 = deps
        .create_account_with_email("visibility-owner-1@golem")
        .await;
    let owner_2 = deps
        .create_account_with_email("visibility-owner-2@golem")
        .await;

    let app_1 = deps.create_application(owner_1.revision.account_id).await;
    let app_2 = deps.create_application(owner_1.revision.account_id).await;
    let app_3 = deps.create_application(owner_2.revision.account_id).await;

    let env_1 = deps.create_env(app_1.revision.application_id).await;
    let env_2 = deps.create_env(app_2.revision.application_id).await;
    let env_3 = deps.create_env(app_3.revision.application_id).await;

    let account_filter =
        EnvironmentVisibilityFilter::from_scopes([EnvironmentVisibilityScope::account(
            owner_1.revision.email.clone(),
        )]);
    let account_filtered = environment_ids(
        deps.environment_repo
            .list_visible_to_account(new_repo_uuid(), &account_filter, None, None, None)
            .await
            .unwrap(),
    );
    check!(
        account_filtered
            == BTreeSet::from([env_1.revision.environment_id, env_2.revision.environment_id])
    );

    let account_filter_with_request_filters = environment_ids(
        deps.environment_repo
            .list_visible_to_account(
                new_repo_uuid(),
                &account_filter,
                Some(&owner_1.revision.email),
                Some(&app_2.revision.name),
                None,
            )
            .await
            .unwrap(),
    );
    check!(account_filter_with_request_filters == BTreeSet::from([env_2.revision.environment_id]));

    let application_filter =
        EnvironmentVisibilityFilter::from_scopes([EnvironmentVisibilityScope::application(
            owner_1.revision.email.clone(),
            app_1.revision.name.clone(),
            None,
        )]);
    let application_filtered = environment_ids(
        deps.environment_repo
            .list_visible_to_account(new_repo_uuid(), &application_filter, None, None, None)
            .await
            .unwrap(),
    );
    check!(application_filtered == BTreeSet::from([env_1.revision.environment_id]));

    let environment_filter =
        EnvironmentVisibilityFilter::from_scopes([EnvironmentVisibilityScope::application(
            owner_2.revision.email.clone(),
            app_3.revision.name.clone(),
            Some(env_3.revision.name.clone()),
        )]);
    let environment_filtered = environment_ids(
        deps.environment_repo
            .list_visible_to_account(new_repo_uuid(), &environment_filter, None, None, None)
            .await
            .unwrap(),
    );
    check!(environment_filtered == BTreeSet::from([env_3.revision.environment_id]));

    let none_filtered = deps
        .environment_repo
        .list_visible_to_account(
            new_repo_uuid(),
            &EnvironmentVisibilityFilter::None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    check!(none_filtered.is_empty());

    let all_with_request_account_filter = environment_ids(
        deps.environment_repo
            .list_visible_to_account(
                new_repo_uuid(),
                &EnvironmentVisibilityFilter::All,
                Some(&owner_2.revision.email),
                None,
                None,
            )
            .await
            .unwrap(),
    );
    check!(all_with_request_account_filter == BTreeSet::from([env_3.revision.environment_id]));
}

fn environment_ids(
    records: Vec<golem_registry_service::repo::model::environment::EnvironmentWithDetailsRecord>,
) -> BTreeSet<Uuid> {
    records
        .into_iter()
        .map(|record| record.environment_id)
        .collect()
}

struct TestAgentSecret {
    owner: AccountExtRevisionRecord,
    app: ApplicationExtRevisionRecord,
    env: EnvironmentExtRevisionRecord,
    id: AgentSecretId,
    path: CanonicalAgentSecretPath,
}

async fn create_test_agent_secret(deps: &Deps) -> TestAgentSecret {
    let owner = deps.create_account().await;
    let app = deps.create_application(owner.revision.account_id).await;
    let env = deps.create_env(app.revision.application_id).await;
    let id = AgentSecretId::new();
    let path = CanonicalAgentSecretPath(vec![format!("secret-{}", new_repo_uuid())]);

    let _ = deps
        .agent_secret_repo
        .create(AgentSecretCreationRecord::new(
            id,
            EnvironmentId(env.revision.environment_id),
            path.clone(),
            SchemaGraph::empty(),
            None,
            AccountId(owner.revision.account_id),
        ))
        .await
        .unwrap();

    TestAgentSecret {
        owner,
        app,
        env,
        id,
        path,
    }
}

async fn get_agent_secret_initial_revision(
    deps: &Deps,
    secret: &TestAgentSecret,
    include_deleted: bool,
) -> bool {
    deps.agent_secret_repo
        .get_revision(
            secret.env.revision.environment_id,
            secret.id,
            secret.path.0.clone(),
            AgentSecretRevision::INITIAL,
            include_deleted,
        )
        .await
        .unwrap()
        .is_some()
}

pub async fn test_agent_secret_get_revision_include_deleted(deps: &Deps) {
    let secret = create_test_agent_secret(deps).await;
    check!(get_agent_secret_initial_revision(deps, &secret, false).await);
    check!(get_agent_secret_initial_revision(deps, &secret, true).await);

    let secret = create_test_agent_secret(deps).await;
    let deleted_revision = AgentSecretRevision::INITIAL.next().unwrap();
    let _ = deps
        .agent_secret_repo
        .delete(
            AgentSecretRevisionRecord::delete(
                secret.id,
                AgentSecretRevision::INITIAL,
                AccountId(secret.owner.revision.account_id),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    check!(!get_agent_secret_initial_revision(deps, &secret, false).await);
    check!(get_agent_secret_initial_revision(deps, &secret, true).await);
    check!(
        deps.agent_secret_repo
            .get_revision(
                secret.env.revision.environment_id,
                secret.id,
                secret.path.0.clone(),
                deleted_revision,
                true,
            )
            .await
            .unwrap()
            .is_none()
    );

    let secret = create_test_agent_secret(deps).await;
    let _ = deps
        .environment_repo
        .delete(EnvironmentRevisionRecord {
            revision_id: secret.env.revision.revision_id + 1,
            ..secret.env.revision.clone()
        })
        .await
        .unwrap();
    check!(!get_agent_secret_initial_revision(deps, &secret, false).await);
    check!(get_agent_secret_initial_revision(deps, &secret, true).await);

    let secret = create_test_agent_secret(deps).await;
    let _ = deps
        .application_repo
        .delete(ApplicationRevisionRecord {
            revision_id: secret.app.revision.revision_id + 1,
            ..secret.app.revision.clone()
        })
        .await
        .unwrap();
    check!(!get_agent_secret_initial_revision(deps, &secret, false).await);
    check!(get_agent_secret_initial_revision(deps, &secret, true).await);

    let secret = create_test_agent_secret(deps).await;
    let _ = deps
        .account_repo
        .delete(AccountRevisionRecord {
            revision_id: secret.owner.revision.revision_id + 1,
            ..secret.owner.revision.clone()
        })
        .await
        .unwrap();
    check!(!get_agent_secret_initial_revision(deps, &secret, false).await);
    check!(get_agent_secret_initial_revision(deps, &secret, true).await);
}

pub async fn test_environment_create_concurrently(deps: &Deps) {
    let user = deps.create_account().await;
    let app = deps.create_application(user.revision.account_id).await;
    let concurrency = 20;

    let results = join_all(
        (0..concurrency)
            .map(|_| async move {
                let revision = EnvironmentRevisionRecord {
                    environment_id: new_repo_uuid(),
                    revision_id: 0,
                    name: "local".to_string(),
                    audit: DeletableRevisionAuditFields::new(user.revision.account_id),
                    compatibility_check: false,
                    version_check: false,
                    security_overrides: false,
                    hash: SqlBlake3Hash::empty(),
                };
                deps.environment_repo
                    .create(
                        app.revision.application_id,
                        revision.clone(),
                        test_environment_default_card_record(revision.environment_id),
                    )
                    .await
            })
            .collect::<Vec<_>>(),
    )
    .await;

    assert_eq!(results.len(), concurrency);
    let created = results.iter().filter(|result| result.is_ok()).count();
    let skipped = results
        .iter()
        .filter(|result| {
            matches!(
                result,
                Err(EnvironmentRepoError::EnvironmentViolatesUniqueness)
            )
        })
        .count();
    check!(created == 1);
    check!(skipped == concurrency - 1);
}

pub async fn test_environment_update(deps: &Deps) {
    let user = deps.create_account().await;
    let app = deps.create_application(user.revision.account_id).await;
    let env_rev_0 = deps.create_env(app.revision.application_id).await;

    let env_rev_1 = EnvironmentRevisionRecord {
        environment_id: env_rev_0.revision.environment_id,
        revision_id: 1,
        name: env_rev_0.revision.name.clone(),
        audit: DeletableRevisionAuditFields::new(user.revision.account_id),
        compatibility_check: true,
        version_check: true,
        security_overrides: false,
        hash: SqlBlake3Hash::empty(),
    }
    .with_updated_hash()
    .unwrap();

    let revision_1_created = deps
        .environment_repo
        .update(env_rev_1.clone())
        .await
        .unwrap();

    assert!(env_rev_1 == revision_1_created.revision);
    assert!(env_rev_0.revision.name == revision_1_created.revision.name);
    assert!(env_rev_0.application_id == revision_1_created.application_id);

    let revision_1_retry = deps.environment_repo.update(env_rev_1.clone()).await;

    assert!(let Err(EnvironmentRepoError::ConcurrentModification) = revision_1_retry);

    let rev_1_by_name = deps
        .environment_repo
        .get_by_name(env_rev_0.application_id, &env_rev_0.revision.name)
        .await
        .unwrap();
    let_assert!(Some(rev_1_by_name) = rev_1_by_name);
    assert!(env_rev_1 == rev_1_by_name.revision);
    assert!(env_rev_0.revision.name == rev_1_by_name.revision.name);
    assert!(env_rev_0.application_id == rev_1_by_name.application_id);

    let rev_1_by_id = deps
        .environment_repo
        .get_by_id(env_rev_1.environment_id, false)
        .await
        .unwrap();
    let_assert!(Some(rev_1_by_id) = rev_1_by_id);
    assert!(env_rev_1 == rev_1_by_id.revision);
    assert!(env_rev_0.revision.name == rev_1_by_id.revision.name);
    assert!(env_rev_0.application_id == rev_1_by_id.application_id);

    let env_rev_2 = EnvironmentRevisionRecord {
        environment_id: env_rev_0.revision.environment_id,
        revision_id: 2,
        name: env_rev_1.name.clone(),
        audit: DeletableRevisionAuditFields::new(user.revision.account_id),
        compatibility_check: true,
        version_check: true,
        security_overrides: false,
        hash: SqlBlake3Hash::empty(),
    }
    .with_updated_hash()
    .unwrap();

    let revision_2_created = deps
        .environment_repo
        .update(env_rev_2.clone())
        .await
        .unwrap();

    assert!(env_rev_2 == revision_2_created.revision);
    assert!(env_rev_0.revision.name == revision_2_created.revision.name);
    assert!(env_rev_0.application_id == revision_2_created.application_id);

    let revision_1_retry = deps.environment_repo.update(env_rev_1.clone()).await;
    assert!(let Err(EnvironmentRepoError::ConcurrentModification) = revision_1_retry);

    let revision_2_retry = deps.environment_repo.update(env_rev_2.clone()).await;
    assert!(let Err(EnvironmentRepoError::ConcurrentModification) = revision_2_retry);

    let rev_2_by_name = deps
        .environment_repo
        .get_by_name(env_rev_0.application_id, &env_rev_0.revision.name)
        .await
        .unwrap();
    let_assert!(Some(rev_2_by_name) = rev_2_by_name);
    assert!(env_rev_2 == rev_2_by_name.revision);
    assert!(env_rev_0.revision.name == rev_2_by_name.revision.name);
    assert!(env_rev_0.application_id == rev_2_by_name.application_id);

    let rev_2_by_id = deps
        .environment_repo
        .get_by_id(env_rev_2.environment_id, false)
        .await
        .unwrap();
    let_assert!(Some(rev_2_by_id) = rev_2_by_id);
    assert!(env_rev_2 == rev_2_by_id.revision);
    assert!(env_rev_0.revision.name == rev_2_by_id.revision.name);
    assert!(env_rev_0.application_id == rev_2_by_id.application_id);
}

pub async fn test_environment_update_concurrently(deps: &Deps) {
    let user = deps.create_account().await;
    let app = deps.create_application(user.revision.account_id).await;
    let env_rev_0 = deps.create_env(app.revision.application_id).await;
    let concurrency = 20;

    let results = join_all(
        (0..concurrency)
            .map(|_| async {
                let revision = EnvironmentRevisionRecord {
                    environment_id: env_rev_0.revision.environment_id,
                    revision_id: 1,
                    name: env_rev_0.revision.name.clone(),
                    audit: DeletableRevisionAuditFields::new(user.revision.account_id),
                    compatibility_check: false,
                    version_check: false,
                    security_overrides: false,
                    hash: SqlBlake3Hash::empty(),
                };
                deps.environment_repo.update(revision.clone()).await
            })
            .collect::<Vec<_>>(),
    )
    .await;

    let created_count = results.iter().filter(|result| result.is_ok()).count();
    let skipped_count = results
        .iter()
        .filter(|result| matches!(result, Err(EnvironmentRepoError::ConcurrentModification)))
        .count();

    check!(created_count == 1);
    check!(skipped_count == concurrency - 1);
}

pub async fn test_environment_default_card_ids_by_account_excludes_deleted(deps: &Deps) {
    let owner = deps.create_account().await;
    let app = deps.create_application(owner.revision.account_id).await;
    let env = deps.create_env(app.revision.application_id).await;

    let refs = deps
        .environment_repo
        .list_default_card_refs_by_account(owner.revision.account_id)
        .await
        .unwrap();
    assert_eq!(refs.len(), 1);

    let _ = deps
        .environment_repo
        .delete(EnvironmentRevisionRecord {
            revision_id: env.revision.revision_id + 1,
            ..env.revision.clone()
        })
        .await
        .unwrap();

    let refs = deps
        .environment_repo
        .list_default_card_refs_by_account(owner.revision.account_id)
        .await
        .unwrap();
    assert!(refs.is_empty());
}

pub async fn test_deleted_environment_default_card_revoke_returns_not_found(deps: &Deps) {
    let owner = deps.create_account().await;
    let app = deps.create_application(owner.revision.account_id).await;
    let services = environment_service_deps(deps);

    let env = services
        .environment_service
        .create(
            ApplicationId(app.revision.application_id),
            EnvironmentCreation {
                name: EnvironmentName("env".to_string()),
                compatibility_check: false,
                version_check: false,
                security_overrides: false,
            },
            &AuthCtx::System,
        )
        .await
        .unwrap();

    let refs = deps
        .environment_repo
        .list_default_card_refs_by_account(owner.revision.account_id)
        .await
        .unwrap();
    assert_eq!(refs.len(), 1);
    let default_card_id = refs[0].card_id;

    services
        .environment_service
        .delete(env.id, env.revision, &AuthCtx::System)
        .await
        .unwrap();

    let get_error = services
        .card_service
        .get_card(default_card_id, &AuthCtx::System)
        .await
        .unwrap_err();
    assert!(matches!(get_error, CardError::CardNotFound(card_id) if card_id == default_card_id));

    let revoke_error = services
        .card_service
        .revoke_card(default_card_id, &AuthCtx::System)
        .await
        .unwrap_err();
    match revoke_error {
        CardError::CardNotFound(card_id) if card_id == default_card_id => {}
        other => panic!(
            "deleted environment default cards should not remain revokable through the raw card row; expected CardNotFound({default_card_id}), got {other:?}"
        ),
    }
}

pub async fn test_deleted_environment_default_card_is_not_reported_existing(deps: &Deps) {
    let owner = deps.create_account().await;
    let app = deps.create_application(owner.revision.account_id).await;
    let services = environment_service_deps(deps);

    let env = services
        .environment_service
        .create(
            ApplicationId(app.revision.application_id),
            EnvironmentCreation {
                name: EnvironmentName("env".to_string()),
                compatibility_check: false,
                version_check: false,
                security_overrides: false,
            },
            &AuthCtx::System,
        )
        .await
        .unwrap();

    let refs = deps
        .environment_repo
        .list_default_card_refs_by_account(owner.revision.account_id)
        .await
        .unwrap();
    assert_eq!(refs.len(), 1);
    let default_card_id = refs[0].card_id;

    services
        .environment_service
        .delete(env.id, env.revision, &AuthCtx::System)
        .await
        .unwrap();

    let get_error = services
        .card_service
        .get_card(default_card_id, &AuthCtx::System)
        .await
        .unwrap_err();
    assert!(matches!(get_error, CardError::CardNotFound(card_id) if card_id == default_card_id));

    assert!(
        services
            .card_service
            .get_cards(vec![default_card_id])
            .await
            .unwrap()
            .is_empty()
    );

    let existing = services
        .card_service
        .existing(vec![default_card_id])
        .await
        .unwrap();

    assert!(
        existing.is_empty(),
        "deleted environment-default cards are logically not found and must not be reported as existing"
    );
}

pub async fn test_runtime_descendants_of_deleted_environment_default_card_are_not_live(
    deps: &Deps,
) {
    let owner = deps.create_account().await;
    let app = deps.create_application(owner.revision.account_id).await;
    let services = environment_service_deps(deps);

    let env = services
        .environment_service
        .create(
            ApplicationId(app.revision.application_id),
            EnvironmentCreation {
                name: EnvironmentName("env".to_string()),
                compatibility_check: false,
                version_check: false,
                security_overrides: false,
            },
            &AuthCtx::System,
        )
        .await
        .unwrap();

    let refs = deps
        .environment_repo
        .list_default_card_refs_by_account(owner.revision.account_id)
        .await
        .unwrap();
    assert_eq!(refs.len(), 1);
    let default_card_id = refs[0].card_id;

    let child_id = CardId::new();
    let mut provenance = runtime_card_provenance();
    provenance.environment_id = env.id;
    services
        .card_service
        .create_runtime_card(
            runtime_card(child_id, vec![default_card_id]),
            provenance.clone(),
            &AuthCtx::System,
        )
        .await
        .unwrap();

    services
        .environment_service
        .delete(env.id, env.revision, &AuthCtx::System)
        .await
        .unwrap();

    assert!(
        services
            .card_service
            .existing(vec![default_card_id, child_id])
            .await
            .unwrap()
            .is_empty(),
        "revoking a parent must also make its runtime descendants non-live"
    );

    let grandchild_id = CardId::new();
    let error = services
        .card_service
        .create_runtime_card(
            runtime_card(grandchild_id, vec![child_id]),
            provenance,
            &AuthCtx::System,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, CardError::CardNotFound(card_id) if card_id == child_id));
}

pub async fn test_environment_deletion_revokes_component_card_runtime_descendants(deps: &Deps) {
    let owner = deps.create_account().await;
    let app = deps.create_application(owner.revision.account_id).await;
    let services = environment_service_deps(deps);
    let env = services
        .environment_service
        .create(
            ApplicationId(app.revision.application_id),
            EnvironmentCreation {
                name: EnvironmentName("env-with-component-card".to_string()),
                compatibility_check: false,
                version_check: false,
                security_overrides: false,
            },
            &AuthCtx::System,
        )
        .await
        .unwrap();

    let component_id = ComponentId(new_repo_uuid());
    let agent_type = AgentTypeName("agent".to_string());
    let initial_card = PolymorphicCard {
        card_id: CardId(new_repo_uuid()),
        parent_ids: Vec::new(),
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: Utc::now(),
        expires_at: None,
        system_card: false,
    };
    let metadata = ComponentMetadata::from_parts(
        KnownExports::default(),
        vec![],
        None,
        None,
        vec![],
        BTreeMap::from([(
            agent_type.clone(),
            AgentTypeProvisionConfig {
                initial_permissions: initial_card.clone(),
                env: BTreeMap::new(),
                config: Vec::new(),
                plugins: Vec::new(),
                files: Vec::new(),
            },
        )]),
    );
    deps.component_repo
        .create(
            env.id.0,
            "component",
            ComponentRevisionRecord {
                component_id: component_id.0,
                revision_id: ComponentRevision::INITIAL.into(),
                hash: SqlBlake3Hash::empty(),
                audit: DeletableRevisionAuditFields::new(owner.revision.account_id),
                size: 0.into(),
                metadata: Blob::new(metadata),
                object_store_key: "component".to_string(),
                binary_hash: SqlBlake3Hash::empty(),
            },
            vec![CardRecord::polymorphic_creation(
                initial_card.clone(),
                Some(CardManagedBy::AgentInitial(CardManagedByAgentInitial {
                    component_id,
                    component_revision: ComponentRevision::INITIAL,
                    agent_type,
                })),
            )],
        )
        .await
        .unwrap();

    let child_id = CardId::new();
    let mut provenance = runtime_card_provenance();
    provenance.environment_id = env.id;
    services
        .card_service
        .create_runtime_card(
            runtime_card(child_id, vec![initial_card.card_id]),
            provenance.clone(),
            &AuthCtx::System,
        )
        .await
        .unwrap();

    services
        .environment_service
        .delete(env.id, env.revision, &AuthCtx::System)
        .await
        .unwrap();

    let grandchild_id = CardId::new();
    let creation = services
        .card_service
        .create_runtime_card(
            runtime_card(grandchild_id, vec![child_id]),
            provenance,
            &AuthCtx::System,
        )
        .await;
    let existing = services
        .card_service
        .existing(vec![initial_card.card_id, child_id, grandchild_id])
        .await
        .unwrap();

    assert!(
        matches!(creation, Err(CardError::CardNotFound(card_id)) if card_id == child_id)
            && existing.is_empty(),
        "environment deletion must revoke component cards and their runtime descendants; creation result: {creation:?}, existing cards: {existing:?}"
    );
}

pub async fn test_environment_default_card_tracks_environment_rename(deps: &Deps) {
    let owner = deps.create_account().await;
    let app = deps.create_application(owner.revision.account_id).await;
    let services = environment_service_deps(deps);

    let env = services
        .environment_service
        .create(
            ApplicationId(app.revision.application_id),
            EnvironmentCreation {
                name: EnvironmentName("old-env".to_string()),
                compatibility_check: false,
                version_check: false,
                security_overrides: false,
            },
            &AuthCtx::System,
        )
        .await
        .unwrap();

    services
        .environment_service
        .update(
            env.id,
            EnvironmentUpdate {
                current_revision: env.revision,
                name: Some(EnvironmentName("new-env".to_string())),
                compatibility_check: None,
                version_check: None,
                security_overrides: None,
            },
            &AuthCtx::System,
        )
        .await
        .unwrap();

    let refs = deps
        .environment_repo
        .list_default_card_refs_by_account(owner.revision.account_id)
        .await
        .unwrap();
    assert_eq!(refs.len(), 1);

    let card = services
        .card_service
        .get_card(refs[0].card_id, &AuthCtx::System)
        .await
        .unwrap();

    assert!(
        default_card_contains_current_environment_permissions(
            &card,
            &EnvironmentName("new-env".to_string())
        ),
        "environment default card should grant permissions for the current environment name after rename"
    );
}

pub async fn test_environment_default_card_tracks_application_rename(deps: &Deps) {
    let owner = deps.create_account().await;
    let app = deps.create_application(owner.revision.account_id).await;
    let services = environment_service_deps(deps);

    let env = services
        .environment_service
        .create(
            ApplicationId(app.revision.application_id),
            EnvironmentCreation {
                name: EnvironmentName("env".to_string()),
                compatibility_check: false,
                version_check: false,
                security_overrides: false,
            },
            &AuthCtx::System,
        )
        .await
        .unwrap();

    deps.application_repo
        .update(ApplicationRevisionRecord {
            application_id: app.revision.application_id,
            revision_id: app.revision.revision_id + 1,
            name: "new-app".to_string(),
            audit: DeletableRevisionAuditFields::new(owner.revision.account_id),
        })
        .await
        .unwrap();

    let refs = deps
        .environment_repo
        .list_default_card_refs_by_account(owner.revision.account_id)
        .await
        .unwrap();
    assert_eq!(refs.len(), 1);

    let card = services
        .card_service
        .get_card(refs[0].card_id, &AuthCtx::System)
        .await
        .unwrap();

    assert!(
        default_card_contains_current_application_permissions(
            &card,
            &ApplicationName("new-app".to_string()),
            &env.name,
        ),
        "environment default card should grant permissions for the current application name after rename"
    );
}

struct EnvironmentServiceDeps {
    environment_service: Arc<EnvironmentService>,
    card_service: CardService,
}

fn environment_service_deps(deps: &Deps) -> EnvironmentServiceDeps {
    match &deps.test_db {
        TestDb::Postgres(pool) => {
            let notifier = deps.test_registry_change_notifier();
            let plan_service = Arc::new(PlanService::new(Arc::new(DbPlanRepo::new(pool.clone()))));
            let account_service = Arc::new(AccountService::new(
                Arc::new(DbAccountRepo::new(pool.clone())),
                plan_service,
                golem_common::model::plan::PlanId(deps.test_plan_id()),
                notifier.clone(),
            ));
            let account_usage_service = Arc::new(AccountUsageService::new(
                Arc::new(DbAccountUsageRepo::new(pool.clone())),
                account_service.clone(),
            ));
            let application_service = Arc::new(ApplicationService::new(
                Arc::new(DbApplicationRepo::new(pool.clone())),
                account_service.clone(),
                account_usage_service.clone(),
                notifier.clone(),
            ));
            let environment_service = EnvironmentService::new(
                Arc::new(DbEnvironmentRepo::new(pool.clone())),
                application_service.clone(),
                account_usage_service,
                Arc::new(DbPluginRepo::new(pool.clone())),
                AccountId::SYSTEM,
                notifier.clone(),
            );
            let environment_service = Arc::new(environment_service);
            let permission_share_service = Arc::new(PermissionShareService::new(
                Arc::new(DbPermissionShareRepo::new(pool.clone())),
                account_service.clone(),
                notifier.clone(),
            ));
            let deployment_service = Arc::new(DeploymentService::new(
                environment_service.clone(),
                application_service,
                Arc::new(DbDeploymentRepo::new(pool.clone())),
            ));
            let component_service = Arc::new(ComponentService::new(
                Arc::new(DbComponentRepo::new(pool.clone())),
                Arc::new(ComponentObjectStore::new(Arc::new(
                    InMemoryBlobStorage::new(),
                ))),
                environment_service.clone(),
                deployment_service,
            ));
            let card_service = CardService::new(
                Arc::new(DbCardRepo::new(pool.clone())),
                account_service,
                permission_share_service,
                component_service,
                environment_service.clone(),
                notifier,
            );

            EnvironmentServiceDeps {
                environment_service,
                card_service,
            }
        }
        TestDb::Sqlite(pool) => {
            let notifier = deps.test_registry_change_notifier();
            let plan_service = Arc::new(PlanService::new(Arc::new(DbPlanRepo::new(pool.clone()))));
            let account_service = Arc::new(AccountService::new(
                Arc::new(DbAccountRepo::new(pool.clone())),
                plan_service,
                golem_common::model::plan::PlanId(deps.test_plan_id()),
                notifier.clone(),
            ));
            let account_usage_service = Arc::new(AccountUsageService::new(
                Arc::new(DbAccountUsageRepo::new(pool.clone())),
                account_service.clone(),
            ));
            let application_service = Arc::new(ApplicationService::new(
                Arc::new(DbApplicationRepo::new(pool.clone())),
                account_service.clone(),
                account_usage_service.clone(),
                notifier.clone(),
            ));
            let environment_service = EnvironmentService::new(
                Arc::new(DbEnvironmentRepo::new(pool.clone())),
                application_service.clone(),
                account_usage_service,
                Arc::new(DbPluginRepo::new(pool.clone())),
                AccountId::SYSTEM,
                notifier.clone(),
            );
            let environment_service = Arc::new(environment_service);
            let permission_share_service = Arc::new(PermissionShareService::new(
                Arc::new(DbPermissionShareRepo::new(pool.clone())),
                account_service.clone(),
                notifier.clone(),
            ));
            let deployment_service = Arc::new(DeploymentService::new(
                environment_service.clone(),
                application_service,
                Arc::new(DbDeploymentRepo::new(pool.clone())),
            ));
            let component_service = Arc::new(ComponentService::new(
                Arc::new(DbComponentRepo::new(pool.clone())),
                Arc::new(ComponentObjectStore::new(Arc::new(
                    InMemoryBlobStorage::new(),
                ))),
                environment_service.clone(),
                deployment_service,
            ));
            let card_service = CardService::new(
                Arc::new(DbCardRepo::new(pool.clone())),
                account_service,
                permission_share_service,
                component_service,
                environment_service.clone(),
                notifier,
            );

            EnvironmentServiceDeps {
                environment_service,
                card_service,
            }
        }
    }
}

fn default_card_contains_current_environment_permissions(
    card: &StoredCard,
    environment_name: &EnvironmentName,
) -> bool {
    let StoredCard::Concrete(card) = card else {
        return false;
    };

    let has_environment_permissions = card.lower_positive.iter().any(|permission| {
        matches!(
            permission,
            PermissionPattern::Environment(pattern)
                if matches!(
                    &pattern.owner,
                    EnvironmentOwnerPattern::Environment { environment, .. }
                        if environment == environment_name
                )
        )
    });
    let has_component_permissions = card.lower_positive.iter().any(|permission| {
        matches!(
            permission,
            PermissionPattern::Component(pattern)
                if matches!(
                    &pattern.owner,
                    ComponentOwnerPattern::EnvironmentComponents { environment, .. }
                        if environment == environment_name
                )
        )
    });

    has_environment_permissions && has_component_permissions
}

fn default_card_contains_current_application_permissions(
    card: &StoredCard,
    application_name: &ApplicationName,
    environment_name: &EnvironmentName,
) -> bool {
    let StoredCard::Concrete(card) = card else {
        return false;
    };

    let has_environment_permissions = card.lower_positive.iter().any(|permission| {
        matches!(
            permission,
            PermissionPattern::Environment(pattern)
                if matches!(
                    &pattern.owner,
                    EnvironmentOwnerPattern::Environment { application, environment, .. }
                        if application == application_name && environment == environment_name
                )
        )
    });
    let has_component_permissions = card.lower_positive.iter().any(|permission| {
        matches!(
            permission,
            PermissionPattern::Component(pattern)
                if matches!(
                    &pattern.owner,
                    ComponentOwnerPattern::EnvironmentComponents { application, environment, .. }
                        if application == application_name && environment == environment_name
                )
        )
    });

    has_environment_permissions && has_component_permissions
}

pub async fn test_component_stage(deps: &Deps) {
    let user = deps.create_account().await;
    let app = deps.create_application(user.revision.account_id).await;
    let env = deps.create_env(app.revision.application_id).await;
    let app = deps
        .application_repo
        .get_by_id(env.application_id)
        .await
        .unwrap()
        .unwrap();
    let component_name = "test-component";
    let component_id = new_repo_uuid();

    deps.plugin_repo
        .create(PluginRecord {
            plugin_id: new_repo_uuid(),
            account_id: app.account_id,
            name: "a".to_string(),
            version: "1.0.0".to_string(),
            audit: ImmutableAuditFields::new(user.revision.account_id),
            description: "".to_string(),
            icon: vec![],
            homepage: "".to_string(),
            plugin_type: 0,
            provided_wit_package: None,
            json_schema: None,
            validate_url: None,
            transform_url: None,
            component_id: None,
            component_revision_id: None,
            wasm_content_hash: None,
        })
        .await
        .unwrap()
        .unwrap();

    deps.plugin_repo
        .create(PluginRecord {
            plugin_id: new_repo_uuid(),
            account_id: app.account_id,
            name: "b".to_string(),
            version: "1.0.0".to_string(),
            audit: ImmutableAuditFields::new(user.revision.account_id),
            description: "".to_string(),
            icon: vec![],
            homepage: "".to_string(),
            plugin_type: 0,
            provided_wit_package: None,
            json_schema: None,
            validate_url: None,
            transform_url: None,
            component_id: None,
            component_revision_id: None,
            wasm_content_hash: None,
        })
        .await
        .unwrap()
        .unwrap();

    let revision_0 = ComponentRevisionRecord {
        component_id,
        revision_id: 0,
        hash: SqlBlake3Hash::empty(),
        audit: DeletableRevisionAuditFields::new(user.revision.account_id),
        size: 10.into(),
        metadata: Blob::new(ComponentMetadata::from_parts_with_tools(
            KnownExports::default(),
            vec![],
            Some("test".to_string()),
            Some("1.0".to_string()),
            vec![],
            std::collections::BTreeMap::new(),
            BTreeMap::from([(
                ToolName::try_from("grep").unwrap(),
                ToolDeploymentMetadata {
                    definition: Tool {
                        version: "1.0.0".to_string(),
                        commands: CommandTree {
                            nodes: vec![CommandNode {
                                name: "grep".to_string(),
                                aliases: Vec::new(),
                                doc: Doc::default(),
                                globals: Globals::default(),
                                subcommands: Vec::new(),
                                body: None,
                            }],
                        },
                        schema: SchemaGraph::empty(),
                    },
                    provision: ToolProvisionConfig::default(),
                    environment_binding: None,
                    agent_bindings: BTreeMap::new(),
                },
            )]),
        )),

        object_store_key: "xys".to_string(),
        binary_hash: blake3::hash("test".as_bytes()).into(),
    }
    .with_updated_hash()
    .unwrap();

    let created_revision_0 = deps
        .component_repo
        .create(
            env.revision.environment_id,
            component_name,
            revision_0.clone(),
            Vec::new(),
        )
        .await
        .unwrap();
    let_assert!(created_revision_0 = created_revision_0);
    assert!(revision_0 == created_revision_0.revision);
    assert!(created_revision_0.environment_id == env.revision.environment_id);
    assert!(created_revision_0.name == component_name);

    let recreate = deps
        .component_repo
        .create(
            env.revision.environment_id,
            component_name,
            revision_0.clone(),
            Vec::new(),
        )
        .await;
    let_assert!(Err(ComponentRepoError::ComponentViolatesUniqueness) = recreate);

    let get_revision_0 = deps
        .component_repo
        .get_staged_by_id(component_id)
        .await
        .unwrap();
    let_assert!(Some(get_revision_0) = get_revision_0);
    assert!(revision_0 == get_revision_0.component.revision);
    assert!(get_revision_0.component.environment_id == env.revision.environment_id);
    assert!(get_revision_0.component.name == component_name);

    let get_revision_0 = deps
        .component_repo
        .get_staged_by_name(env.revision.environment_id, component_name)
        .await
        .unwrap();
    let_assert!(Some(get_revision_0) = get_revision_0);
    assert!(revision_0 == get_revision_0.revision);
    assert!(get_revision_0.environment_id == env.revision.environment_id);
    assert!(get_revision_0.name == component_name);

    let components = deps
        .component_repo
        .list_staged(env.revision.environment_id)
        .await
        .unwrap();
    assert!(components.len() == 1);
    assert!(components[0].revision == revision_0);
    assert!(components[0].environment_id == env.revision.environment_id);
    assert!(components[0].name == component_name);

    let revision_1 = ComponentRevisionRecord {
        revision_id: 1,
        size: 12345.into(),
        binary_hash: SqlBlake3Hash::empty(),
        ..revision_0.clone()
    }
    .with_updated_hash()
    .unwrap();

    let created_revision_1 = deps
        .component_repo
        .update(revision_1.clone(), Vec::new())
        .await
        .unwrap();
    let_assert!(created_revision_1 = created_revision_1);
    assert!(revision_1 == created_revision_1.revision);
    assert!(created_revision_1.environment_id == env.revision.environment_id);
    assert!(created_revision_1.name == component_name);

    let recreated_revision_1 = deps
        .component_repo
        .update(revision_1.clone(), Vec::new())
        .await;
    let_assert!(Err(ComponentRepoError::ConcurrentModification) = recreated_revision_1);

    let components = deps
        .component_repo
        .list_staged(env.revision.environment_id)
        .await
        .unwrap();
    assert!(components.len() == 1);
    assert!(components[0].revision == revision_1);

    let other_component_id = new_repo_uuid();
    let other_component_name = "test-component-other";
    let other_component_revision_0 = ComponentRevisionRecord {
        component_id: other_component_id,
        ..revision_0.clone()
    }
    .with_updated_hash()
    .unwrap();

    let created_other_component_0 = deps
        .component_repo
        .create(
            env.revision.environment_id,
            other_component_name,
            other_component_revision_0.clone(),
            Vec::new(),
        )
        .await
        .unwrap();
    assert!(created_other_component_0.revision == other_component_revision_0);

    let components = deps
        .component_repo
        .list_staged(env.revision.environment_id)
        .await
        .unwrap();

    assert!(components.len() == 2);
    assert!(components[0].revision == revision_1);
    assert!(components[1].revision == other_component_revision_0);

    let delete_with_old_revision = deps
        .component_repo
        .delete(user.revision.account_id, component_id, 1)
        .await;
    let_assert!(Err(ComponentRepoError::ConcurrentModification) = delete_with_old_revision);

    deps.component_repo
        .delete(user.revision.account_id, component_id, 2)
        .await
        .unwrap()
        .signal_new_events_available(deps.test_registry_change_notifier().as_ref());

    let components = deps
        .component_repo
        .list_staged(env.revision.environment_id)
        .await
        .unwrap();

    assert!(components.len() == 1);
    assert!(components[0].revision == other_component_revision_0);

    let revision_after_delete = ComponentRevisionRecord {
        component_id: new_repo_uuid(),
        ..revision_0.clone()
    };
    let created_after_delete = deps
        .component_repo
        .create(
            env.revision.environment_id,
            component_name,
            revision_after_delete.clone(),
            Vec::new(),
        )
        .await
        .unwrap();
    let revision_after_delete = ComponentRevisionRecord {
        component_id: revision_0.component_id,
        revision_id: 3,
        ..revision_after_delete
    }
    .with_updated_hash()
    .unwrap();
    let_assert!(created_after_delete = created_after_delete);
    assert!(created_after_delete.revision == revision_after_delete);
}

pub async fn test_initial_permission_card_ids_by_account_are_unique(deps: &Deps) {
    let owner = deps.create_account().await;
    let app = deps.create_application(owner.revision.account_id).await;
    let env = deps.create_env(app.revision.application_id).await;

    let component_id = ComponentId(new_repo_uuid());
    let agent_type = AgentTypeName("agent".to_string());
    let initial_card = PolymorphicCard {
        card_id: CardId(new_repo_uuid()),
        parent_ids: Vec::new(),
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: Utc::now(),
        expires_at: None,
        system_card: false,
    };
    let metadata = ComponentMetadata::from_parts(
        KnownExports::default(),
        vec![],
        None,
        None,
        vec![],
        BTreeMap::from([(
            agent_type.clone(),
            AgentTypeProvisionConfig {
                initial_permissions: initial_card.clone(),
                env: BTreeMap::new(),
                config: Vec::new(),
                plugins: Vec::new(),
                files: Vec::new(),
            },
        )]),
    );

    deps.component_repo
        .create(
            env.revision.environment_id,
            "component",
            ComponentRevisionRecord {
                component_id: component_id.0,
                revision_id: ComponentRevision::INITIAL.into(),
                hash: SqlBlake3Hash::empty(),
                audit: DeletableRevisionAuditFields::new(owner.revision.account_id),
                size: 0.into(),
                metadata: Blob::new(metadata.clone()),
                object_store_key: "component".to_string(),
                binary_hash: SqlBlake3Hash::empty(),
            },
            vec![CardRecord::polymorphic_creation(
                initial_card.clone(),
                Some(CardManagedBy::AgentInitial(CardManagedByAgentInitial {
                    component_id,
                    component_revision: ComponentRevision::INITIAL,
                    agent_type: agent_type.clone(),
                })),
            )],
        )
        .await
        .unwrap();

    deps.component_repo
        .update(
            ComponentRevisionRecord {
                component_id: component_id.0,
                revision_id: ComponentRevision::INITIAL.next().unwrap().into(),
                hash: SqlBlake3Hash::empty(),
                audit: DeletableRevisionAuditFields::new(owner.revision.account_id),
                size: 0.into(),
                metadata: Blob::new(metadata),
                object_store_key: "component".to_string(),
                binary_hash: SqlBlake3Hash::empty(),
            },
            Vec::new(),
        )
        .await
        .unwrap();

    let ids = deps
        .component_repo
        .list_initial_permission_card_ids_by_account(owner.revision.account_id)
        .await
        .unwrap();
    assert_eq!(ids, vec![initial_card.card_id]);
}

pub async fn test_agent_initial_card_from_older_component_revision_remains_live(deps: &Deps) {
    let owner = deps.create_account().await;
    let app = deps.create_application(owner.revision.account_id).await;
    let env = deps.create_env(app.revision.application_id).await;
    let services = environment_service_deps(deps);

    let component_id = ComponentId(new_repo_uuid());
    let agent_type = AgentTypeName("agent".to_string());
    let initial_card = PolymorphicCard {
        card_id: CardId(new_repo_uuid()),
        parent_ids: Vec::new(),
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: Utc::now(),
        expires_at: None,
        system_card: false,
    };
    let metadata_with_agent = ComponentMetadata::from_parts(
        KnownExports::default(),
        vec![],
        None,
        None,
        vec![],
        BTreeMap::from([(
            agent_type.clone(),
            AgentTypeProvisionConfig {
                initial_permissions: initial_card.clone(),
                env: BTreeMap::new(),
                config: Vec::new(),
                plugins: Vec::new(),
                files: Vec::new(),
            },
        )]),
    );

    deps.component_repo
        .create(
            env.revision.environment_id,
            "component",
            ComponentRevisionRecord {
                component_id: component_id.0,
                revision_id: ComponentRevision::INITIAL.into(),
                hash: SqlBlake3Hash::empty(),
                audit: DeletableRevisionAuditFields::new(owner.revision.account_id),
                size: 0.into(),
                metadata: Blob::new(metadata_with_agent),
                object_store_key: "component".to_string(),
                binary_hash: SqlBlake3Hash::empty(),
            },
            vec![CardRecord::polymorphic_creation(
                initial_card.clone(),
                Some(CardManagedBy::AgentInitial(CardManagedByAgentInitial {
                    component_id,
                    component_revision: ComponentRevision::INITIAL,
                    agent_type: agent_type.clone(),
                })),
            )],
        )
        .await
        .unwrap();

    let derived_parent_id = CardId::new();
    let mut provenance = runtime_card_provenance();
    provenance.environment_id = EnvironmentId(env.revision.environment_id);
    services
        .card_service
        .create_runtime_card(
            runtime_card(derived_parent_id, vec![initial_card.card_id]),
            provenance.clone(),
            &AuthCtx::System,
        )
        .await
        .unwrap();

    deps.component_repo
        .update(
            ComponentRevisionRecord {
                component_id: component_id.0,
                revision_id: ComponentRevision::INITIAL.next().unwrap().into(),
                hash: SqlBlake3Hash::empty(),
                audit: DeletableRevisionAuditFields::new(owner.revision.account_id),
                size: 0.into(),
                metadata: Blob::new(ComponentMetadata::default()),
                object_store_key: "component".to_string(),
                binary_hash: SqlBlake3Hash::empty(),
            },
            Vec::new(),
        )
        .await
        .unwrap();

    assert!(
        deps.component_repo
            .list_staged(env.revision.environment_id)
            .await
            .unwrap()[0]
            .revision
            .metadata
            .clone()
            .into_value()
            .agent_type_provision_configs()
            .is_empty(),
        "the current live component revision no longer has an agent-initial source entry"
    );

    let ids = deps
        .component_repo
        .list_initial_permission_card_ids_by_account(owner.revision.account_id)
        .await
        .unwrap();
    assert_eq!(
        ids,
        Vec::<CardId>::new(),
        "the staged-card listing follows the current component revision"
    );

    assert_eq!(
        services
            .card_service
            .get_cards(vec![initial_card.card_id])
            .await
            .unwrap()
            .into_iter()
            .map(|card| card.card_id())
            .collect::<Vec<_>>(),
        vec![initial_card.card_id],
        "agents can keep using cards from their component revision after a newer revision is staged"
    );
    assert_eq!(
        services
            .card_service
            .existing(vec![initial_card.card_id])
            .await
            .unwrap(),
        vec![initial_card.card_id],
        "older-revision agent-initial cards remain authoritative"
    );

    let card_repo: Arc<dyn CardRepo> = match &deps.test_db {
        TestDb::Postgres(pool) => Arc::new(DbCardRepo::new(pool.clone())),
        TestDb::Sqlite(pool) => Arc::new(DbCardRepo::new(pool.clone())),
    };
    assert!(
        card_repo.get(initial_card.card_id).await.unwrap().is_some(),
        "the older revision's agent-initial card remains physically stored"
    );
    let child_id = CardId::new();
    services
        .card_service
        .create_runtime_card(
            runtime_card(child_id, vec![initial_card.card_id]),
            runtime_card_provenance(),
            &AuthCtx::System,
        )
        .await
        .unwrap();
    assert!(
        card_repo.get(child_id).await.unwrap().is_some(),
        "runtime derivation from an older-revision agent-initial card remains valid"
    );

    let transitive_child_id = CardId::new();
    services
        .card_service
        .create_runtime_card(
            runtime_card(transitive_child_id, vec![derived_parent_id]),
            provenance,
            &AuthCtx::System,
        )
        .await
        .unwrap();
    assert!(
        card_repo.get(transitive_child_id).await.unwrap().is_some(),
        "runtime descendants of an older-revision card remain live"
    );

    let account_cards = services
        .card_service
        .list_account_cards(
            AccountId(owner.revision.account_id),
            AccountCardFilter {
                root: false,
                permission_share: false,
                environment_default: false,
                agent_initial: true,
            },
            &AuthCtx::System,
        )
        .await
        .unwrap();
    assert!(
        account_cards.is_empty(),
        "account card listing is a view of the currently staged component revision"
    );
}

pub async fn test_initial_permission_card_ids_by_account_excludes_deleted_components(deps: &Deps) {
    let owner = deps.create_account().await;
    let app = deps.create_application(owner.revision.account_id).await;
    let env = deps.create_env(app.revision.application_id).await;

    let component_id = ComponentId(new_repo_uuid());
    let agent_type = AgentTypeName("agent".to_string());
    let initial_card = PolymorphicCard {
        card_id: CardId(new_repo_uuid()),
        parent_ids: Vec::new(),
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: Utc::now(),
        expires_at: None,
        system_card: false,
    };
    let metadata = ComponentMetadata::from_parts(
        KnownExports::default(),
        vec![],
        None,
        None,
        vec![],
        BTreeMap::from([(
            agent_type.clone(),
            AgentTypeProvisionConfig {
                initial_permissions: initial_card.clone(),
                env: BTreeMap::new(),
                config: Vec::new(),
                plugins: Vec::new(),
                files: Vec::new(),
            },
        )]),
    );

    deps.component_repo
        .create(
            env.revision.environment_id,
            "component",
            ComponentRevisionRecord {
                component_id: component_id.0,
                revision_id: ComponentRevision::INITIAL.into(),
                hash: SqlBlake3Hash::empty(),
                audit: DeletableRevisionAuditFields::new(owner.revision.account_id),
                size: 0.into(),
                metadata: Blob::new(metadata),
                object_store_key: "component".to_string(),
                binary_hash: SqlBlake3Hash::empty(),
            },
            vec![CardRecord::polymorphic_creation(
                initial_card.clone(),
                Some(CardManagedBy::AgentInitial(CardManagedByAgentInitial {
                    component_id,
                    component_revision: ComponentRevision::INITIAL,
                    agent_type: agent_type.clone(),
                })),
            )],
        )
        .await
        .unwrap();

    let ids = deps
        .component_repo
        .list_initial_permission_card_ids_by_account(owner.revision.account_id)
        .await
        .unwrap();
    assert_eq!(ids, vec![initial_card.card_id]);

    deps.component_repo
        .delete(owner.revision.account_id, component_id.0, 1)
        .await
        .unwrap()
        .signal_new_events_available(deps.test_registry_change_notifier().as_ref());

    let ids = deps
        .component_repo
        .list_initial_permission_card_ids_by_account(owner.revision.account_id)
        .await
        .unwrap();
    assert!(ids.is_empty());
}

pub async fn test_deleted_component_agent_initial_card_is_not_reported_existing(deps: &Deps) {
    let owner = deps.create_account().await;
    let app = deps.create_application(owner.revision.account_id).await;
    let env = deps.create_env(app.revision.application_id).await;
    let services = environment_service_deps(deps);

    let component_id = ComponentId(new_repo_uuid());
    let agent_type = AgentTypeName("agent".to_string());
    let initial_card = PolymorphicCard {
        card_id: CardId(new_repo_uuid()),
        parent_ids: Vec::new(),
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: Utc::now(),
        expires_at: None,
        system_card: false,
    };
    let metadata = ComponentMetadata::from_parts(
        KnownExports::default(),
        vec![],
        None,
        None,
        vec![],
        BTreeMap::from([(
            agent_type.clone(),
            AgentTypeProvisionConfig {
                initial_permissions: initial_card.clone(),
                env: BTreeMap::new(),
                config: Vec::new(),
                plugins: Vec::new(),
                files: Vec::new(),
            },
        )]),
    );

    deps.component_repo
        .create(
            env.revision.environment_id,
            "component",
            ComponentRevisionRecord {
                component_id: component_id.0,
                revision_id: ComponentRevision::INITIAL.into(),
                hash: SqlBlake3Hash::empty(),
                audit: DeletableRevisionAuditFields::new(owner.revision.account_id),
                size: 0.into(),
                metadata: Blob::new(metadata),
                object_store_key: "component".to_string(),
                binary_hash: SqlBlake3Hash::empty(),
            },
            vec![CardRecord::polymorphic_creation(
                initial_card.clone(),
                Some(CardManagedBy::AgentInitial(CardManagedByAgentInitial {
                    component_id,
                    component_revision: ComponentRevision::INITIAL,
                    agent_type: agent_type.clone(),
                })),
            )],
        )
        .await
        .unwrap();

    assert_eq!(
        deps.component_repo
            .list_initial_permission_card_ids_by_account(owner.revision.account_id)
            .await
            .unwrap(),
        vec![initial_card.card_id]
    );

    deps.component_repo
        .delete(owner.revision.account_id, component_id.0, 1)
        .await
        .unwrap()
        .signal_new_events_available(deps.test_registry_change_notifier().as_ref());

    assert!(
        deps.component_repo
            .list_initial_permission_card_ids_by_account(owner.revision.account_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        services
            .card_service
            .get_cards(vec![initial_card.card_id])
            .await
            .unwrap()
            .is_empty(),
        "deleted component agent-initial cards are no longer account cards and must not remain live through batch_get_cards"
    );
    assert!(
        services
            .card_service
            .existing(vec![initial_card.card_id])
            .await
            .unwrap()
            .is_empty(),
        "deleted component agent-initial cards are no longer account cards and must not remain live through existing"
    );
}

pub async fn test_component_delete_does_not_revoke_reused_agent_initial_card_id(deps: &Deps) {
    let owner = deps.create_account().await;
    let app = deps.create_application(owner.revision.account_id).await;
    let env = deps.create_env(app.revision.application_id).await;
    let services = environment_service_deps(deps);

    let component_id = ComponentId(new_repo_uuid());
    let agent_type = AgentTypeName("agent".to_string());
    let reused_card_id = CardId(new_repo_uuid());
    let initial_card = PolymorphicCard {
        card_id: reused_card_id,
        parent_ids: Vec::new(),
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: Utc::now(),
        expires_at: None,
        system_card: false,
    };
    let metadata = ComponentMetadata::from_parts(
        KnownExports::default(),
        vec![],
        None,
        None,
        vec![],
        BTreeMap::from([(
            agent_type.clone(),
            AgentTypeProvisionConfig {
                initial_permissions: initial_card.clone(),
                env: BTreeMap::new(),
                config: Vec::new(),
                plugins: Vec::new(),
                files: Vec::new(),
            },
        )]),
    );

    deps.component_repo
        .create(
            env.revision.environment_id,
            "component-with-reused-card-id",
            ComponentRevisionRecord {
                component_id: component_id.0,
                revision_id: ComponentRevision::INITIAL.into(),
                hash: SqlBlake3Hash::empty(),
                audit: DeletableRevisionAuditFields::new(owner.revision.account_id),
                size: 0.into(),
                metadata: Blob::new(metadata),
                object_store_key: "component-with-reused-card-id".to_string(),
                binary_hash: SqlBlake3Hash::empty(),
            },
            vec![CardRecord::polymorphic_creation(
                initial_card.clone(),
                Some(CardManagedBy::AgentInitial(CardManagedByAgentInitial {
                    component_id,
                    component_revision: ComponentRevision::INITIAL,
                    agent_type,
                })),
            )],
        )
        .await
        .unwrap();

    services
        .card_service
        .revoke_card(reused_card_id, &AuthCtx::System)
        .await
        .unwrap();

    let mut provenance = runtime_card_provenance();
    provenance.environment_id = EnvironmentId(env.revision.environment_id);
    services
        .card_service
        .create_runtime_card(
            runtime_card(reused_card_id, Vec::new()),
            provenance.clone(),
            &AuthCtx::System,
        )
        .await
        .unwrap();
    let child_id = CardId::new();
    services
        .card_service
        .create_runtime_card(
            runtime_card(child_id, vec![reused_card_id]),
            provenance,
            &AuthCtx::System,
        )
        .await
        .unwrap();

    let deleted = deps
        .component_repo
        .delete(owner.revision.account_id, component_id.0, 1)
        .await
        .unwrap()
        .signal_new_events_available(deps.test_registry_change_notifier().as_ref());

    assert!(
        !deleted.contains(&reused_card_id) && !deleted.contains(&child_id),
        "component deletion must only revoke roots still managed as AgentInitial cards and their descendants"
    );
    assert_eq!(
        services
            .card_service
            .existing(vec![reused_card_id, child_id])
            .await
            .unwrap(),
        vec![reused_card_id, child_id],
        "reusing a revoked card ID for an unrelated runtime card must not make it part of the component's revocation tree"
    );
}

pub async fn test_initial_permission_card_ids_by_account_excludes_pre_recreate_revisions(
    deps: &Deps,
) {
    let owner = deps.create_account().await;
    let app = deps.create_application(owner.revision.account_id).await;
    let env = deps.create_env(app.revision.application_id).await;
    let services = environment_service_deps(deps);

    let component_id = ComponentId(new_repo_uuid());
    let agent_type = AgentTypeName("agent".to_string());
    let initial_card = PolymorphicCard {
        card_id: CardId(new_repo_uuid()),
        parent_ids: Vec::new(),
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: Utc::now(),
        expires_at: None,
        system_card: false,
    };
    let metadata = ComponentMetadata::from_parts(
        KnownExports::default(),
        vec![],
        None,
        None,
        vec![],
        BTreeMap::from([(
            agent_type.clone(),
            AgentTypeProvisionConfig {
                initial_permissions: initial_card.clone(),
                env: BTreeMap::new(),
                config: Vec::new(),
                plugins: Vec::new(),
                files: Vec::new(),
            },
        )]),
    );

    deps.component_repo
        .create(
            env.revision.environment_id,
            "component",
            ComponentRevisionRecord {
                component_id: component_id.0,
                revision_id: ComponentRevision::INITIAL.into(),
                hash: SqlBlake3Hash::empty(),
                audit: DeletableRevisionAuditFields::new(owner.revision.account_id),
                size: 0.into(),
                metadata: Blob::new(metadata),
                object_store_key: "component".to_string(),
                binary_hash: SqlBlake3Hash::empty(),
            },
            vec![CardRecord::polymorphic_creation(
                initial_card.clone(),
                Some(CardManagedBy::AgentInitial(CardManagedByAgentInitial {
                    component_id,
                    component_revision: ComponentRevision::INITIAL,
                    agent_type: agent_type.clone(),
                })),
            )],
        )
        .await
        .unwrap();

    let descendant_id = CardId::new();
    let mut provenance = runtime_card_provenance();
    provenance.environment_id = EnvironmentId(env.revision.environment_id);
    services
        .card_service
        .create_runtime_card(
            runtime_card(descendant_id, vec![initial_card.card_id]),
            provenance.clone(),
            &AuthCtx::System,
        )
        .await
        .unwrap();

    deps.component_repo
        .delete(owner.revision.account_id, component_id.0, 1)
        .await
        .unwrap()
        .signal_new_events_available(deps.test_registry_change_notifier().as_ref());

    deps.component_repo
        .create(
            env.revision.environment_id,
            "component",
            ComponentRevisionRecord {
                component_id: new_repo_uuid(),
                revision_id: ComponentRevision::INITIAL.into(),
                hash: SqlBlake3Hash::empty(),
                audit: DeletableRevisionAuditFields::new(owner.revision.account_id),
                size: 0.into(),
                metadata: Blob::new(ComponentMetadata::default()),
                object_store_key: "component-recreated".to_string(),
                binary_hash: SqlBlake3Hash::empty(),
            },
            Vec::new(),
        )
        .await
        .unwrap();

    let ids = deps
        .component_repo
        .list_initial_permission_card_ids_by_account(owner.revision.account_id)
        .await
        .unwrap();
    assert_eq!(
        ids,
        Vec::<CardId>::new(),
        "agent-initial cards from a deleted component incarnation must not reappear after same-name recreation"
    );
    assert!(
        services
            .card_service
            .get_cards(vec![initial_card.card_id])
            .await
            .unwrap()
            .is_empty(),
        "agent-initial cards from a deleted component incarnation must not remain live through batch_get_cards after same-name recreation"
    );
    assert!(
        services
            .card_service
            .existing(vec![initial_card.card_id])
            .await
            .unwrap()
            .is_empty(),
        "agent-initial cards from a deleted component incarnation must not be reported as existing after same-name recreation"
    );

    assert!(
        services
            .card_service
            .existing(vec![descendant_id])
            .await
            .unwrap()
            .is_empty(),
        "runtime descendants from a deleted component incarnation must not remain authoritative after same-name recreation"
    );

    let rejected_card_id = CardId::new();
    let error = services
        .card_service
        .create_runtime_card(
            runtime_card(rejected_card_id, vec![descendant_id]),
            provenance,
            &AuthCtx::System,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, CardError::CardNotFound(id) if id == descendant_id));

    let card_repo: Arc<dyn CardRepo> = match &deps.test_db {
        TestDb::Postgres(pool) => Arc::new(DbCardRepo::new(pool.clone())),
        TestDb::Sqlite(pool) => Arc::new(DbCardRepo::new(pool.clone())),
    };
    assert!(
        card_repo.get(descendant_id).await.unwrap().is_none(),
        "component deletion must physically remove runtime descendants"
    );
    assert!(card_repo.get(rejected_card_id).await.unwrap().is_none());
}

pub async fn test_http_api_deployment_stage(deps: &Deps) {
    let user = deps.create_account().await;
    let app = deps.create_application(user.revision.account_id).await;
    let env = deps.create_env(app.revision.application_id).await;
    let domain = "test-host-1.com";
    let deployment_id = new_repo_uuid();

    let revision_0 = HttpApiDeploymentRevisionRecord {
        http_api_deployment_id: deployment_id,
        revision_id: 0,
        hash: SqlBlake3Hash::empty(),
        audit: DeletableRevisionAuditFields::new(user.revision.account_id),
        data: Blob::new(HttpApiDeploymentData {
            agents: BTreeMap::from_iter([(
                AgentTypeName("test-agent".to_string()),
                HttpApiDeploymentAgentOptions::default(),
            )]),
            webhooks_prefix: "/webhooks/".to_string(),
            openapi_endpoint_prefix: "/".to_string(),
        }),
    }
    .with_updated_hash()
    .unwrap();

    let created_revision_0 = deps
        .http_api_deployment_repo
        .create(env.revision.environment_id, domain, revision_0.clone())
        .await
        .unwrap();

    assert!(revision_0 == created_revision_0.revision);
    assert!(created_revision_0.environment_id == env.revision.environment_id);
    assert!(created_revision_0.domain == domain);

    let recreate = deps
        .http_api_deployment_repo
        .create(env.revision.environment_id, domain, revision_0.clone())
        .await;

    let_assert!(Err(HttpApiDeploymentRepoError::ApiDeploymentViolatesUniqueness) = recreate);

    let get_revision_0 = deps
        .http_api_deployment_repo
        .get_staged_by_id(deployment_id)
        .await
        .unwrap();
    let_assert!(Some(get_revision_0) = get_revision_0);
    assert!(revision_0 == get_revision_0.deployment.revision);
    assert!(get_revision_0.deployment.environment_id == env.revision.environment_id);
    assert!(get_revision_0.deployment.domain == domain);

    let get_revision_0 = deps
        .http_api_deployment_repo
        .get_staged_by_domain(env.revision.environment_id, domain)
        .await
        .unwrap();
    let_assert!(Some(get_revision_0) = get_revision_0);
    assert!(revision_0 == get_revision_0.revision);
    assert!(get_revision_0.environment_id == env.revision.environment_id);
    assert!(get_revision_0.domain == domain);

    let deployments = deps
        .http_api_deployment_repo
        .list_staged(env.revision.environment_id)
        .await
        .unwrap();
    assert!(deployments.len() == 1);
    assert!(deployments[0].revision == revision_0);
    assert!(deployments[0].environment_id == env.revision.environment_id);
    assert!(deployments[0].domain == domain);

    let revision_1 = HttpApiDeploymentRevisionRecord {
        revision_id: 1,
        hash: SqlBlake3Hash::empty(),
        ..revision_0.clone()
    }
    .with_updated_hash()
    .unwrap();

    let created_revision_1 = deps
        .http_api_deployment_repo
        .update(revision_1.clone())
        .await
        .unwrap();

    assert!(revision_1 == created_revision_1.revision);
    assert!(created_revision_1.environment_id == env.revision.environment_id);
    assert!(created_revision_1.domain == domain);

    let recreated_revision_1 = deps
        .http_api_deployment_repo
        .update(revision_1.clone())
        .await;

    let_assert!(Err(HttpApiDeploymentRepoError::ConcurrentModification) = recreated_revision_1);

    let deployments = deps
        .http_api_deployment_repo
        .list_staged(env.revision.environment_id)
        .await
        .unwrap();

    assert!(deployments.len() == 1);
    assert!(deployments[0].revision == revision_1);

    let other_deployment_id = new_repo_uuid();
    let other_domain = "test-host-2.com";
    let other_deployment_revision_0 = HttpApiDeploymentRevisionRecord {
        http_api_deployment_id: other_deployment_id,
        ..revision_0.clone()
    }
    .with_updated_hash()
    .unwrap();

    let created_other_deployment_0 = deps
        .http_api_deployment_repo
        .create(
            env.revision.environment_id,
            other_domain,
            other_deployment_revision_0.clone(),
        )
        .await
        .unwrap();
    assert!(created_other_deployment_0.revision == other_deployment_revision_0);

    let deployments = deps
        .http_api_deployment_repo
        .list_staged(env.revision.environment_id)
        .await
        .unwrap();

    assert!(deployments.len() == 2);
    assert!(deployments[0].revision == revision_1);
    assert!(deployments[1].revision == other_deployment_revision_0);

    let delete_with_old_revision = deps
        .http_api_deployment_repo
        .delete(user.revision.account_id, deployment_id, 1)
        .await;

    let_assert!(Err(HttpApiDeploymentRepoError::ConcurrentModification) = delete_with_old_revision);

    deps.http_api_deployment_repo
        .delete(user.revision.account_id, deployment_id, 2)
        .await
        .unwrap();

    let deployments = deps
        .http_api_deployment_repo
        .list_staged(env.revision.environment_id)
        .await
        .unwrap();

    assert!(deployments.len() == 1);
    assert!(deployments[0].revision == other_deployment_revision_0);

    let revision_after_delete = HttpApiDeploymentRevisionRecord {
        http_api_deployment_id: new_repo_uuid(),
        ..revision_0.clone()
    };
    let created_after_delete = deps
        .http_api_deployment_repo
        .create(
            env.revision.environment_id,
            domain,
            revision_after_delete.clone(),
        )
        .await
        .unwrap();
    let revision_after_delete = HttpApiDeploymentRevisionRecord {
        http_api_deployment_id: revision_0.http_api_deployment_id,
        revision_id: 3,
        ..revision_after_delete
    };
    assert!(created_after_delete.revision == revision_after_delete);
}

pub async fn test_account_resource_override_resolution(deps: &Deps) {
    let account = deps.create_account().await;
    let now = SqlDateTime::now();

    deps.account_resource_override_repo
        .upsert(AccountResourceOverrideRecord {
            account_id: account.revision.account_id,
            dimension: AccountResourceOverrideDimension::MaxDiskSpacePerWorker,
            source: AccountResourceOverrideSource::SelfService,
            override_value: 1234.into(),
            reason: AccountResourceOverrideReason::UserSelfServe,
            expires_at: None,
            created_by: account.revision.account_id,
            created_at: now.clone(),
        })
        .await
        .unwrap();
    deps.account_resource_override_repo
        .upsert(AccountResourceOverrideRecord {
            account_id: account.revision.account_id,
            dimension: AccountResourceOverrideDimension::MaxMemoryPerWorker,
            source: AccountResourceOverrideSource::SelfService,
            override_value: 1234.into(),
            reason: AccountResourceOverrideReason::UserSelfServe,
            expires_at: None,
            created_by: account.revision.account_id,
            created_at: now.clone(),
        })
        .await
        .unwrap();
    let assertion_now = SqlDateTime::now();
    assert_eq!(
        deps.account_resource_override_repo
            .get_active_value(
                account.revision.account_id,
                AccountResourceOverrideDimension::MaxDiskSpacePerWorker,
                &assertion_now,
            )
            .await
            .unwrap()
            .map(|value| value.get()),
        Some(1234)
    );

    let usage = deps
        .account_usage_repo
        .get(account.revision.account_id, &assertion_now)
        .await
        .unwrap()
        .unwrap();
    assert!(usage.resource_limits().max_disk_space_per_worker == 1073741824);
    assert_eq!(usage.storage_limit.effective_value, Some(1073741824));
    assert_eq!(usage.storage_limit.plan_default, Some(1073741824));
    assert_eq!(usage.storage_limit.override_value, None);
    assert_eq!(usage.max_memory_per_worker.override_value, None);
    assert_eq!(usage.resource_limits().max_memory_per_worker, 4000);

    deps.account_resource_override_repo
        .upsert(AccountResourceOverrideRecord {
            account_id: account.revision.account_id,
            dimension: AccountResourceOverrideDimension::MaxDiskSpacePerWorker,
            source: AccountResourceOverrideSource::SelfService,
            override_value: 5678.into(),
            reason: AccountResourceOverrideReason::UserSelfServe,
            expires_at: Some(SqlDateTime::new(Utc::now() - chrono::Duration::seconds(1))),
            created_by: account.revision.account_id,
            created_at: now.clone(),
        })
        .await
        .unwrap();
    deps.account_resource_override_repo
        .upsert(AccountResourceOverrideRecord {
            account_id: account.revision.account_id,
            dimension: AccountResourceOverrideDimension::MaxMemoryPerWorker,
            source: AccountResourceOverrideSource::SelfService,
            override_value: 5678.into(),
            reason: AccountResourceOverrideReason::UserSelfServe,
            expires_at: Some(SqlDateTime::new(Utc::now() - chrono::Duration::seconds(1))),
            created_by: account.revision.account_id,
            created_at: now.clone(),
        })
        .await
        .unwrap();
    let expiry_assertion_now = SqlDateTime::now();
    assert_eq!(
        deps.account_resource_override_repo
            .get_active_value(
                account.revision.account_id,
                AccountResourceOverrideDimension::MaxDiskSpacePerWorker,
                &expiry_assertion_now,
            )
            .await
            .unwrap(),
        None
    );

    let usage = deps
        .account_usage_repo
        .get(account.revision.account_id, &expiry_assertion_now)
        .await
        .unwrap()
        .unwrap();
    assert!(usage.resource_limits().max_disk_space_per_worker == 1073741824);
    assert_eq!(usage.storage_limit.effective_value, Some(1073741824));
    assert_eq!(usage.storage_limit.override_value, None);
    assert_eq!(usage.max_memory_per_worker.override_value, None);
    assert_eq!(usage.resource_limits().max_memory_per_worker, 4000);
}

static ADMIN_GRANT_CLEANUP_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn assign_grant_test_plan(deps: &Deps, account: &AccountExtRevisionRecord) -> PlanRecord {
    let mut plan = deps
        .plan_repo
        .get_by_id(account.revision.plan_id)
        .await
        .unwrap()
        .unwrap();
    plan.plan_id = new_repo_uuid();
    plan.name = format!("GRANT_TEST_PLAN_{}", plan.plan_id);
    plan.monthly_compute_gcu = 2.into();
    plan.monthly_memory_gb_seconds = 6000.into();
    plan.monthly_durable_storage_gb_month = 3.into();
    plan.monthly_ephemeral_storage_gb_month = 4.into();
    plan.max_memory_per_worker = 4000.into();
    plan.max_memory_per_worker_ceiling = u64::MAX.into();
    plan.max_memory_per_worker_user_configurable = true;
    plan.max_disk_space_per_worker_enabled = true;
    plan.max_disk_space_per_worker = 1073741824.into();
    plan.max_disk_space_per_worker_ceiling = 1073741824.into();
    plan.max_disk_space_per_worker_user_configurable = false;
    deps.plan_repo.create_or_update(plan.clone()).await.unwrap();
    deps.account_service()
        .set_plan(
            AccountId(account.revision.account_id),
            AccountSetPlan {
                current_revision: AccountRevision::INITIAL,
                plan: PlanId(plan.plan_id),
            },
            &AuthCtx::System,
        )
        .await
        .unwrap();
    plan
}

pub async fn test_admin_resource_grants_resolve_all_dimensions_and_preserve_owner_values(
    deps: &Deps,
) {
    let account = deps.create_account().await;
    let account_id = account.revision.account_id;
    let mut plan = assign_grant_test_plan(deps, &account).await;
    let plan_id = plan.plan_id;
    plan.max_memory_per_worker = 100.into();
    plan.max_memory_per_worker_ceiling = 400.into();
    plan.max_memory_per_worker_user_configurable = true;
    plan.max_disk_space_per_worker_enabled = true;
    plan.max_disk_space_per_worker = 100.into();
    plan.max_disk_space_per_worker_ceiling = 200.into();
    plan.max_disk_space_per_worker_user_configurable = true;
    deps.plan_repo.create_or_update(plan).await.unwrap();

    for dimension in [
        AccountResourceOverrideDimension::MaxMemoryPerWorker,
        AccountResourceOverrideDimension::MaxDiskSpacePerWorker,
    ] {
        deps.account_resource_override_repo
            .set_user_override(account_id, dimension, 150, account_id)
            .await
            .unwrap();
    }

    let grants = [
        (AccountResourceOverrideDimension::MonthlyComputeGcu, 5),
        (
            AccountResourceOverrideDimension::MonthlyMemoryGbSeconds,
            7000,
        ),
        (
            AccountResourceOverrideDimension::MonthlyDurableStorageGbMonth,
            5,
        ),
        (
            AccountResourceOverrideDimension::MonthlyEphemeralStorageGbMonth,
            6,
        ),
        (AccountResourceOverrideDimension::MaxMemoryPerWorker, 300),
        (AccountResourceOverrideDimension::MaxDiskSpacePerWorker, 300),
    ];
    for (dimension, value) in grants {
        let change = deps
            .account_resource_override_repo
            .set_admin_grant(
                account_id,
                dimension,
                value,
                AdminResourceGrantReason::Support,
                None,
                account_id,
            )
            .await
            .unwrap();
        assert_eq!(
            change.event_type,
            AdminResourceGrantEventType::OverrideGranted
        );
        assert_eq!(change.new_value, value);
    }

    let mut updated_plan = deps.plan_repo.get_by_id(plan_id).await.unwrap().unwrap();
    updated_plan.monthly_compute_gcu = 4.into();
    updated_plan.monthly_memory_gb_seconds = 6000.into();
    updated_plan.monthly_durable_storage_gb_month = 4.into();
    updated_plan.monthly_ephemeral_storage_gb_month = 5.into();
    deps.plan_repo.create_or_update(updated_plan).await.unwrap();

    let usage = deps
        .account_usage_repo
        .get(account_id, &SqlDateTime::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(usage.max_memory_per_worker.override_value, Some(150));
    assert_eq!(usage.max_memory_per_worker.effective_value, 300);
    assert_eq!(usage.storage_limit.override_value, Some(150));
    assert_eq!(usage.storage_limit.effective_value, Some(300));
    assert_eq!(usage.resource_limits().max_memory_per_worker, 300);
    assert_eq!(usage.resource_limits().max_disk_space_per_worker, 300);

    let policy = deps
        .account_usage_service()
        .get_resource_policy(AccountId(account_id), &AuthCtx::System)
        .await
        .unwrap();
    assert_eq!(policy.admin_grants.len(), grants.len());
    assert_eq!(policy.monthly.compute_gcu.monthly_amount, Some(5));
    assert_eq!(policy.monthly.memory_gb_seconds.monthly_amount, Some(7000));
    assert_eq!(
        policy.monthly.durable_storage_gb_month.monthly_amount,
        Some(5)
    );
    assert_eq!(
        policy.monthly.ephemeral_storage_gb_month.monthly_amount,
        Some(6)
    );
    assert_eq!(policy.monthly_usage_mode, MonthlyUsageMode::HardLimit);

    deps.account_resource_override_repo
        .set_user_override(
            account_id,
            AccountResourceOverrideDimension::MaxMemoryPerWorker,
            300,
            account_id,
        )
        .await
        .unwrap();
    let active = deps
        .account_resource_override_repo
        .get_active_admin_grants(account_id, &SqlDateTime::now())
        .await
        .unwrap();
    assert_eq!(active.len(), grants.len());
    let events = deps
        .account_resource_override_repo
        .get_admin_grant_events(account_id)
        .await
        .unwrap();
    assert_eq!(events.len(), grants.len());
    assert!(
        events
            .iter()
            .all(|event| event.event_type == AdminResourceGrantEventType::OverrideGranted)
    );
    deps.account_resource_override_repo
        .set_admin_grant(
            account_id,
            AccountResourceOverrideDimension::MaxMemoryPerWorker,
            350,
            AdminResourceGrantReason::Support,
            None,
            account_id,
        )
        .await
        .unwrap();

    let clear = deps
        .account_resource_override_repo
        .clear_admin_grant(
            account_id,
            AccountResourceOverrideDimension::MaxMemoryPerWorker,
            account_id,
        )
        .await
        .unwrap();
    assert_eq!(
        clear.event_type,
        AdminResourceGrantEventType::OverrideCleared
    );
    assert_eq!(clear.old_value, 350);
    assert_eq!(clear.new_value, 300);
    let usage = deps
        .account_usage_repo
        .get(account_id, &SqlDateTime::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(usage.max_memory_per_worker.override_value, Some(300));
    assert_eq!(usage.max_memory_per_worker.effective_value, 300);

    for (dimension, value) in grants {
        if dimension == AccountResourceOverrideDimension::MaxMemoryPerWorker {
            continue;
        }
        let clear = deps
            .account_resource_override_repo
            .clear_admin_grant(account_id, dimension, account_id)
            .await
            .unwrap();
        assert_eq!(
            clear.event_type,
            AdminResourceGrantEventType::OverrideCleared
        );
        assert_eq!(clear.old_value, value);
    }

    assert!(
        deps.account_resource_override_repo
            .get_active_admin_grants(account_id, &SqlDateTime::now())
            .await
            .unwrap()
            .is_empty()
    );
    let usage = deps
        .account_usage_repo
        .get(account_id, &SqlDateTime::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(usage.storage_limit.override_value, Some(150));
    assert_eq!(usage.storage_limit.effective_value, Some(150));
    let policy = deps
        .account_usage_service()
        .get_resource_policy(AccountId(account_id), &AuthCtx::System)
        .await
        .unwrap();
    assert_eq!(policy.monthly.compute_gcu.monthly_amount, Some(4));
    assert_eq!(policy.monthly.memory_gb_seconds.monthly_amount, Some(6000));
    assert_eq!(
        policy.monthly.durable_storage_gb_month.monthly_amount,
        Some(4)
    );
    assert_eq!(
        policy.monthly.ephemeral_storage_gb_month.monthly_amount,
        Some(5)
    );
}

pub async fn test_replacing_admin_grant_must_raise_current_grant(deps: &Deps) {
    let account = deps.create_account().await;
    let account_id = account.revision.account_id;
    assign_grant_test_plan(deps, &account).await;
    let dimension = AccountResourceOverrideDimension::MonthlyComputeGcu;

    deps.account_resource_override_repo
        .set_admin_grant(
            account_id,
            dimension,
            5,
            AdminResourceGrantReason::Support,
            None,
            account_id,
        )
        .await
        .unwrap();

    for value in [4, 5] {
        assert!(matches!(
            deps.account_resource_override_repo
                .set_admin_grant(
                    account_id,
                    dimension,
                    value,
                    AdminResourceGrantReason::Support,
                    None,
                    account_id,
                )
                .await,
            Err(AdminResourceGrantRepoError::DoesNotIncreaseResolvedValue {
                value: rejected,
                current: 5,
            }) if rejected == value
        ));
    }

    let active = deps
        .account_resource_override_repo
        .get_active_admin_grants(account_id, &SqlDateTime::now())
        .await
        .unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].value, 5);
    assert_eq!(
        deps.account_resource_override_repo
            .get_admin_grant_events(account_id)
            .await
            .unwrap()
            .len(),
        1
    );

    let change = deps
        .account_resource_override_repo
        .set_admin_grant(
            account_id,
            dimension,
            6,
            AdminResourceGrantReason::Support,
            None,
            account_id,
        )
        .await
        .unwrap();
    assert_eq!((change.old_value, change.new_value), (5, 6));
}

pub async fn test_admin_resource_grant_expiry_is_immediate_and_cleanup_is_idempotent(deps: &Deps) {
    let _cleanup_guard = ADMIN_GRANT_CLEANUP_TEST_LOCK.lock().await;
    let account = deps.create_account().await;
    let account_id = account.revision.account_id;
    let mut grant_plan = assign_grant_test_plan(deps, &account).await;
    grant_plan.max_memory_per_worker = 4000.into();
    grant_plan.max_memory_per_worker_ceiling = 10_000.into();
    grant_plan.max_memory_per_worker_user_configurable = true;
    deps.plan_repo
        .create_or_update(grant_plan.clone())
        .await
        .unwrap();
    deps.account_resource_override_repo
        .set_user_override(
            account_id,
            AccountResourceOverrideDimension::MaxMemoryPerWorker,
            5000,
            account_id,
        )
        .await
        .unwrap();

    let expires_at = SqlDateTime::new(Utc::now() + chrono::Duration::hours(1));
    let granted = deps
        .account_resource_override_repo
        .set_admin_grant(
            account_id,
            AccountResourceOverrideDimension::MaxMemoryPerWorker,
            7000,
            AdminResourceGrantReason::Promotional,
            Some(expires_at.clone()),
            account_id,
        )
        .await
        .unwrap();
    deps.account_resource_override_repo
        .set_user_override(
            account_id,
            AccountResourceOverrideDimension::MaxMemoryPerWorker,
            6000,
            account_id,
        )
        .await
        .unwrap();

    let cleanup_at = SqlDateTime::new(Utc::now() + chrono::Duration::hours(2));
    assert!(
        deps.account_resource_override_repo
            .get_active_admin_grants(account_id, &cleanup_at)
            .await
            .unwrap()
            .is_empty()
    );
    let usage = deps
        .account_usage_repo
        .get_with_active_overrides_at(account_id, &SqlDateTime::now(), &cleanup_at)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(usage.max_memory_per_worker.override_value, Some(6000));
    assert_eq!(usage.max_memory_per_worker.effective_value, 6000);

    assert_eq!(
        deps.account_resource_override_repo
            .cleanup_expired_admin_grants(cleanup_at.clone())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        deps.account_resource_override_repo
            .cleanup_expired_admin_grants(cleanup_at.clone())
            .await
            .unwrap(),
        0
    );
    let events = deps
        .account_resource_override_repo
        .get_admin_grant_events(account_id)
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].account_id, AccountId(account_id));
    assert_eq!(
        events[0].dimension,
        AdminResourceGrantDimension::MaxMemoryPerAgent
    );
    assert_eq!(
        events[0].event_type,
        AdminResourceGrantEventType::OverrideGranted
    );
    assert_eq!(events[0].reason, AdminResourceGrantReason::Promotional);
    assert_eq!(events[0].actor_account_id, AccountId(account_id));
    assert_eq!(
        events[0].changed_at.timestamp_micros(),
        granted.changed_at.timestamp_micros()
    );
    assert_eq!(events[0].old_value, 5000);
    assert_eq!(events[0].new_value, 7000);
    assert_eq!(
        events[0].expires_at.map(|value| value.timestamp_micros()),
        Some(expires_at.as_utc().timestamp_micros())
    );
    assert_eq!(
        events[1].event_type,
        AdminResourceGrantEventType::OverrideExpired
    );
    assert_eq!(events[1].reason, AdminResourceGrantReason::Promotional);
    assert_eq!(events[1].old_value, 7000);
    assert_eq!(events[1].new_value, 6000);
    assert_eq!(events[1].actor_account_id, AccountId::SYSTEM);
    assert_eq!(
        events[1].changed_at.timestamp_micros(),
        cleanup_at.as_utc().timestamp_micros()
    );
    assert_eq!(
        events[1].expires_at.map(|value| value.timestamp_micros()),
        Some(expires_at.as_utc().timestamp_micros())
    );

    assert!(matches!(
        deps.account_resource_override_repo
            .set_admin_grant(
                account_id,
                AccountResourceOverrideDimension::MonthlyComputeGcu,
                3,
                AdminResourceGrantReason::Promotional,
                None,
                account_id,
            )
            .await,
        Err(AdminResourceGrantRepoError::PromotionalExpiryRequired)
    ));

    grant_plan.max_disk_space_per_worker_enabled = false;
    deps.plan_repo.create_or_update(grant_plan).await.unwrap();
    assert!(matches!(
        deps.account_resource_override_repo
            .set_admin_grant(
                account_id,
                AccountResourceOverrideDimension::MaxDiskSpacePerWorker,
                2_000_000_000,
                AdminResourceGrantReason::Support,
                None,
                account_id,
            )
            .await,
        Err(AdminResourceGrantRepoError::FeatureDisabled)
    ));
}

pub async fn test_replacing_expired_admin_grant_records_expiry_before_new_grant(deps: &Deps) {
    let _cleanup_guard = ADMIN_GRANT_CLEANUP_TEST_LOCK.lock().await;
    let account = deps.create_account().await;
    let account_id = account.revision.account_id;
    assign_grant_test_plan(deps, &account).await;
    let expires_at = SqlDateTime::new(Utc::now() + chrono::Duration::seconds(1));
    let just_before_expiry =
        SqlDateTime::new(expires_at.as_utc().to_owned() - chrono::Duration::microseconds(1));

    let granted = deps
        .account_resource_override_repo
        .set_admin_grant(
            account_id,
            AccountResourceOverrideDimension::MonthlyComputeGcu,
            3,
            AdminResourceGrantReason::Support,
            Some(expires_at.clone()),
            account_id,
        )
        .await
        .unwrap();
    let granted_at = SqlDateTime::new(granted.changed_at);
    let before_expiry = deps
        .account_usage_repo
        .get_with_active_overrides_at(account_id, &granted_at, &just_before_expiry)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        before_expiry.admin_grant_values.monthly_compute_gcu,
        Some(3)
    );
    assert_eq!(before_expiry.admin_grants.len(), 1);
    assert_eq!(before_expiry.admin_grants[0].value, 3);
    assert_eq!(
        before_expiry.admin_grants[0].dimension,
        AdminResourceGrantDimension::MonthlyComputeGcu
    );
    assert_eq!(
        before_expiry.admin_grants[0].reason,
        AdminResourceGrantReason::Support
    );
    assert_eq!(
        before_expiry.admin_grants[0].actor_account_id,
        AccountId(account_id)
    );
    assert_eq!(
        before_expiry.admin_grants[0].granted_at.timestamp_micros(),
        granted_at.as_utc().timestamp_micros()
    );
    assert_eq!(
        before_expiry.admin_grants[0]
            .expires_at
            .map(|value| value.timestamp_micros()),
        Some(expires_at.as_utc().timestamp_micros())
    );

    let after_expiry = deps
        .account_usage_repo
        .get_with_active_overrides_at(account_id, &granted_at, &expires_at)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_expiry.admin_grant_values.monthly_compute_gcu, None);
    assert!(after_expiry.admin_grants.is_empty());

    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let replacement = deps
        .account_resource_override_repo
        .set_admin_grant(
            account_id,
            AccountResourceOverrideDimension::MonthlyComputeGcu,
            4,
            AdminResourceGrantReason::Support,
            None,
            account_id,
        )
        .await
        .unwrap();
    assert_eq!(replacement.old_value, 2);
    assert_eq!(replacement.new_value, 4);

    let events = deps
        .account_resource_override_repo
        .get_admin_grant_events(account_id)
        .await
        .unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>(),
        vec![
            AdminResourceGrantEventType::OverrideGranted,
            AdminResourceGrantEventType::OverrideExpired,
            AdminResourceGrantEventType::OverrideGranted,
        ]
    );
    assert_eq!((events[0].old_value, events[0].new_value), (2, 3));
    assert_eq!((events[1].old_value, events[1].new_value), (3, 2));
    assert_eq!((events[2].old_value, events[2].new_value), (2, 4));
    assert_eq!(events[1].actor_account_id, AccountId::SYSTEM);
    assert_eq!(
        events[1].changed_at.timestamp_micros(),
        replacement.changed_at.timestamp_micros()
    );
    assert_eq!(
        events[1].expires_at.map(|value| value.timestamp_micros()),
        Some(expires_at.as_utc().timestamp_micros())
    );
    assert_eq!(
        deps.account_resource_override_repo
            .cleanup_expired_admin_grants(
                SqlDateTime::new(Utc::now() + chrono::Duration::hours(3),)
            )
            .await
            .unwrap(),
        0
    );
}

pub async fn test_admin_grant_operations_observe_time_after_locks(deps: &Deps) {
    let _cleanup_guard = ADMIN_GRANT_CLEANUP_TEST_LOCK.lock().await;
    let TestDb::Postgres(pool) = &deps.test_db else {
        panic!("this race depends on PostgreSQL row-lock semantics");
    };
    let pool = pool.clone();
    let replacement_account = deps.create_account().await;
    let plan = assign_grant_test_plan(deps, &replacement_account).await;
    let clear_account = deps.create_account().await;
    deps.account_service()
        .set_plan(
            AccountId(clear_account.revision.account_id),
            AccountSetPlan {
                current_revision: AccountRevision::INITIAL,
                plan: PlanId(plan.plan_id),
            },
            &AuthCtx::System,
        )
        .await
        .unwrap();

    let replacement_account_id = replacement_account.revision.account_id;
    let clear_account_id = clear_account.revision.account_id;
    let dimension = AccountResourceOverrideDimension::MonthlyComputeGcu;
    let expires_at = SqlDateTime::new(Utc::now() + chrono::Duration::seconds(2));
    for account_id in [replacement_account_id, clear_account_id] {
        deps.account_resource_override_repo
            .set_admin_grant(
                account_id,
                dimension,
                5,
                AdminResourceGrantReason::Support,
                Some(expires_at.clone()),
                account_id,
            )
            .await
            .unwrap();
    }

    let mut blocker = pool
        .with_rw("test", "block_admin_grant_operations_across_expiry")
        .begin()
        .await
        .unwrap();
    blocker
        .execute(
            sqlx::query("SELECT plan_id FROM plans WHERE plan_id = $1 FOR UPDATE")
                .bind(plan.plan_id),
        )
        .await
        .unwrap();

    let replacement_task = tokio::spawn({
        let repo = deps.account_resource_override_repo.clone();
        async move {
            repo.set_admin_grant(
                replacement_account_id,
                dimension,
                4,
                AdminResourceGrantReason::Support,
                None,
                replacement_account_id,
            )
            .await
        }
    });
    let clear_task = tokio::spawn({
        let repo = deps.account_resource_override_repo.clone();
        async move {
            repo.clear_admin_grant(clear_account_id, dimension, clear_account_id)
                .await
        }
    });
    wait_for_postgres_locks(&pool, "FROM plans", 2).await;
    let remaining = expires_at
        .as_utc()
        .signed_duration_since(Utc::now())
        .to_std()
        .expect("grant expired before both operations reached the Plan lock");
    tokio::time::sleep(remaining + std::time::Duration::from_millis(50)).await;
    blocker.commit().await.unwrap();

    let replacement = tokio::time::timeout(std::time::Duration::from_secs(5), replacement_task)
        .await
        .expect("admin grant replacement remained blocked")
        .unwrap()
        .unwrap();
    assert_eq!((replacement.old_value, replacement.new_value), (2, 4));
    assert!(replacement.changed_at >= *expires_at.as_utc());
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(5), clear_task)
            .await
            .expect("admin grant clear remained blocked")
            .unwrap(),
        Err(AdminResourceGrantRepoError::GrantNotFound)
    ));

    let replacement_events = deps
        .account_resource_override_repo
        .get_admin_grant_events(replacement_account_id)
        .await
        .unwrap();
    assert_eq!(
        replacement_events
            .iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>(),
        vec![
            AdminResourceGrantEventType::OverrideGranted,
            AdminResourceGrantEventType::OverrideExpired,
            AdminResourceGrantEventType::OverrideGranted,
        ]
    );
    assert_eq!(
        deps.account_resource_override_repo
            .cleanup_expired_admin_grants(SqlDateTime::now())
            .await
            .unwrap(),
        1
    );
    let clear_events = deps
        .account_resource_override_repo
        .get_admin_grant_events(clear_account_id)
        .await
        .unwrap();
    assert_eq!(
        clear_events
            .iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>(),
        vec![
            AdminResourceGrantEventType::OverrideGranted,
            AdminResourceGrantEventType::OverrideExpired,
        ]
    );
}

pub async fn test_cleanup_removes_expired_admin_grant_for_soft_deleted_account(deps: &Deps) {
    let _cleanup_guard = ADMIN_GRANT_CLEANUP_TEST_LOCK.lock().await;
    let account = deps.create_account().await;
    let account_id = account.revision.account_id;
    let mut plan = assign_grant_test_plan(deps, &account).await;
    let expires_at = SqlDateTime::new(Utc::now() + chrono::Duration::hours(1));
    let cleanup_at = SqlDateTime::new(Utc::now() + chrono::Duration::hours(2));
    deps.account_resource_override_repo
        .set_admin_grant(
            account_id,
            AccountResourceOverrideDimension::MonthlyComputeGcu,
            3,
            AdminResourceGrantReason::Support,
            Some(expires_at.clone()),
            account_id,
        )
        .await
        .unwrap();
    deps.account_resource_override_repo
        .set_admin_grant(
            account_id,
            AccountResourceOverrideDimension::MaxDiskSpacePerWorker,
            2_000_000_000,
            AdminResourceGrantReason::Support,
            Some(expires_at.clone()),
            account_id,
        )
        .await
        .unwrap();
    let current_account = deps
        .account_repo
        .get_by_id(account_id)
        .await
        .unwrap()
        .unwrap();
    let _ = deps
        .account_repo
        .delete(AccountRevisionRecord {
            revision_id: current_account.revision.revision_id + 1,
            ..current_account.revision
        })
        .await
        .unwrap();
    plan.max_disk_space_per_worker_enabled = false;
    deps.plan_repo.create_or_update(plan).await.unwrap();

    assert_eq!(
        deps.account_resource_override_repo
            .cleanup_expired_admin_grants(cleanup_at.clone())
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        deps.account_resource_override_repo
            .cleanup_expired_admin_grants(cleanup_at.clone())
            .await
            .unwrap(),
        0
    );
    let events = deps
        .account_resource_override_repo
        .get_admin_grant_events(account_id)
        .await
        .unwrap();
    assert_eq!(events.len(), 4);
    let monthly_expiry = events
        .iter()
        .find(|event| {
            event.dimension == AdminResourceGrantDimension::MonthlyComputeGcu
                && event.event_type == AdminResourceGrantEventType::OverrideExpired
        })
        .unwrap();
    assert_eq!((monthly_expiry.old_value, monthly_expiry.new_value), (3, 2));
    assert_eq!(monthly_expiry.actor_account_id, AccountId::SYSTEM);
    assert_eq!(
        monthly_expiry.changed_at.timestamp_micros(),
        cleanup_at.as_utc().timestamp_micros()
    );
    assert_eq!(
        monthly_expiry
            .expires_at
            .map(|value| value.timestamp_micros()),
        Some(expires_at.as_utc().timestamp_micros())
    );
    let storage_expiry = events
        .iter()
        .find(|event| {
            event.dimension == AdminResourceGrantDimension::MaxStoragePerAgent
                && event.event_type == AdminResourceGrantEventType::OverrideExpired
        })
        .unwrap();
    assert_eq!(
        (storage_expiry.old_value, storage_expiry.new_value),
        (2_000_000_000, EFFECTIVELY_UNLIMITED_STORAGE_LIMIT)
    );
}

pub async fn test_plan_update_preserves_active_grants(deps: &Deps) {
    let account = deps.create_account().await;
    let account_id = account.revision.account_id;
    let mut plan = deps
        .plan_repo
        .get_by_id(account.revision.plan_id)
        .await
        .unwrap()
        .unwrap();
    plan.plan_id = new_repo_uuid();
    plan.name = format!("GRANT_PLAN_UPDATE_{}", plan.plan_id);
    plan.max_memory_per_worker = 100.into();
    plan.max_memory_per_worker_ceiling = 1000.into();
    plan.max_memory_per_worker_user_configurable = true;
    plan.max_disk_space_per_worker_enabled = true;
    plan.max_disk_space_per_worker = 100.into();
    plan.max_disk_space_per_worker_ceiling = 1000.into();
    plan.max_disk_space_per_worker_user_configurable = true;
    deps.plan_repo.create_or_update(plan.clone()).await.unwrap();
    deps.account_service()
        .set_plan(
            AccountId(account_id),
            AccountSetPlan {
                current_revision: AccountRevision::INITIAL,
                plan: PlanId(plan.plan_id),
            },
            &AuthCtx::System,
        )
        .await
        .unwrap();

    for dimension in [
        AccountResourceOverrideDimension::MaxMemoryPerWorker,
        AccountResourceOverrideDimension::MaxDiskSpacePerWorker,
    ] {
        deps.account_resource_override_repo
            .set_user_override(account_id, dimension, 200, account_id)
            .await
            .unwrap();
    }

    let expires_at = SqlDateTime::new(Utc::now() + chrono::Duration::days(1));
    for (dimension, value) in [
        (AccountResourceOverrideDimension::MonthlyComputeGcu, 3),
        (
            AccountResourceOverrideDimension::MonthlyMemoryGbSeconds,
            7000,
        ),
        (AccountResourceOverrideDimension::MaxMemoryPerWorker, 300),
        (AccountResourceOverrideDimension::MaxDiskSpacePerWorker, 300),
    ] {
        deps.account_resource_override_repo
            .set_admin_grant(
                account_id,
                dimension,
                value,
                AdminResourceGrantReason::Support,
                Some(expires_at.clone()),
                account_id,
            )
            .await
            .unwrap();
    }

    plan.monthly_compute_gcu = 4.into();
    plan.max_memory_per_worker = 400.into();
    plan.max_disk_space_per_worker_enabled = false;
    deps.plan_repo.create_or_update(plan.clone()).await.unwrap();

    let active = deps
        .account_resource_override_repo
        .get_active_admin_grants(account_id, &SqlDateTime::now())
        .await
        .unwrap();
    assert_eq!(active.len(), 4);

    let events = deps
        .account_resource_override_repo
        .get_admin_grant_events(account_id)
        .await
        .unwrap();
    assert_eq!(events.len(), 4);
    assert!(
        events
            .iter()
            .all(|event| event.event_type == AdminResourceGrantEventType::OverrideGranted)
    );
    let disabled_usage = deps
        .account_usage_repo
        .get(account_id, &SqlDateTime::now())
        .await
        .unwrap()
        .unwrap();
    assert!(!disabled_usage.storage_limit.enabled);
    let policy = deps
        .account_usage_service()
        .get_resource_policy(AccountId(account_id), &AuthCtx::System)
        .await
        .unwrap();
    assert_eq!(policy.monthly.compute_gcu.monthly_amount, Some(3));
    assert_eq!(policy.max_memory_per_agent.effective_value, 300);
    assert_eq!(policy.admin_grants.len(), 4);

    plan.monthly_compute_gcu = 2.into();
    plan.max_memory_per_worker = 100.into();
    plan.max_disk_space_per_worker_enabled = true;
    deps.plan_repo.create_or_update(plan).await.unwrap();
    let active = deps
        .account_resource_override_repo
        .get_active_admin_grants(account_id, &SqlDateTime::now())
        .await
        .unwrap();
    assert_eq!(active.len(), 4);
    let policy = deps
        .account_usage_service()
        .get_resource_policy(AccountId(account_id), &AuthCtx::System)
        .await
        .unwrap();
    assert_eq!(policy.monthly.compute_gcu.monthly_amount, Some(3));
    assert_eq!(policy.max_memory_per_agent.effective_value, 300);
    assert_eq!(policy.max_storage_per_agent.effective_value, Some(300));
}

pub async fn test_account_plan_change_preserves_active_grants(deps: &Deps) {
    let account = deps.create_account().await;
    let account_id = account.revision.account_id;
    let mut source = deps
        .plan_repo
        .get_by_id(account.revision.plan_id)
        .await
        .unwrap()
        .unwrap();
    source.plan_id = new_repo_uuid();
    source.name = format!("GRANT_RECONCILIATION_SOURCE_{}", source.plan_id);
    source.monthly_compute_gcu = 2.into();
    source.max_memory_per_worker = 4000.into();
    source.max_memory_per_worker_ceiling = 10_000.into();
    deps.plan_repo
        .create_or_update(source.clone())
        .await
        .unwrap();
    let source_assignment = deps
        .account_service()
        .set_plan(
            AccountId(account_id),
            AccountSetPlan {
                current_revision: AccountRevision::INITIAL,
                plan: PlanId(source.plan_id),
            },
            &AuthCtx::System,
        )
        .await
        .unwrap();

    let mut destination = source;
    destination.plan_id = new_repo_uuid();
    destination.name = format!("GRANT_RECONCILIATION_PLAN_{}", destination.plan_id);
    destination.monthly_compute_gcu = 4.into();
    destination.max_memory_per_worker = 6000.into();
    deps.plan_repo
        .create_or_update(destination.clone())
        .await
        .unwrap();

    for (dimension, value) in [
        (AccountResourceOverrideDimension::MonthlyComputeGcu, 3),
        (AccountResourceOverrideDimension::MaxMemoryPerWorker, 5000),
    ] {
        deps.account_resource_override_repo
            .set_admin_grant(
                account_id,
                dimension,
                value,
                AdminResourceGrantReason::Support,
                None,
                account_id,
            )
            .await
            .unwrap();
    }

    deps.account_service()
        .set_plan(
            AccountId(account_id),
            AccountSetPlan {
                current_revision: source_assignment.revision,
                plan: PlanId(destination.plan_id),
            },
            &AuthCtx::System,
        )
        .await
        .unwrap();

    assert_eq!(
        deps.account_resource_override_repo
            .get_active_admin_grants(account_id, &SqlDateTime::now())
            .await
            .unwrap()
            .len(),
        2
    );
    let events = deps
        .account_resource_override_repo
        .get_admin_grant_events(account_id)
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|event| event.event_type == AdminResourceGrantEventType::OverrideGranted)
    );
    let policy = deps
        .account_usage_service()
        .get_resource_policy(AccountId(account_id), &AuthCtx::System)
        .await
        .unwrap();
    assert_eq!(policy.monthly.compute_gcu.monthly_amount, Some(3));
    assert_eq!(policy.max_memory_per_agent.effective_value, 5000);

    destination.monthly_compute_gcu = 2.into();
    destination.max_memory_per_worker = 4000.into();
    deps.plan_repo.create_or_update(destination).await.unwrap();
    assert_eq!(
        deps.account_resource_override_repo
            .get_active_admin_grants(account_id, &SqlDateTime::now())
            .await
            .unwrap()
            .len(),
        2
    );
    let policy = deps
        .account_usage_service()
        .get_resource_policy(AccountId(account_id), &AuthCtx::System)
        .await
        .unwrap();
    assert_eq!(policy.monthly.compute_gcu.monthly_amount, Some(3));
    assert_eq!(policy.max_memory_per_agent.effective_value, 5000);
}

pub async fn test_self_service_clear_serializes_with_account_plan_change(deps: &Deps) {
    let TestDb::Postgres(pool) = &deps.test_db else {
        panic!("this race depends on PostgreSQL row-lock semantics");
    };
    let pool = pool.clone();
    let account = deps.create_account().await;
    let account_id = account.revision.account_id;
    deps.account_resource_override_repo
        .upsert(AccountResourceOverrideRecord {
            account_id,
            dimension: AccountResourceOverrideDimension::MaxMemoryPerWorker,
            source: AccountResourceOverrideSource::SelfService,
            override_value: 5000.into(),
            reason: AccountResourceOverrideReason::UserSelfServe,
            expires_at: None,
            created_by: account_id,
            created_at: SqlDateTime::now(),
        })
        .await
        .unwrap();

    let mut destination = deps
        .plan_repo
        .get_by_id(account.revision.plan_id)
        .await
        .unwrap()
        .unwrap();
    destination.plan_id = new_repo_uuid();
    destination.name = format!("CLEAR_SERIALIZATION_PLAN_{}", destination.plan_id);
    deps.plan_repo
        .create_or_update(destination.clone())
        .await
        .unwrap();

    let mut blocker = pool
        .with_rw("test", "block_clear_account_plan_change")
        .begin()
        .await
        .unwrap();
    blocker
        .execute(
            sqlx::query("SELECT plan_id FROM plans WHERE plan_id = $1 FOR UPDATE")
                .bind(destination.plan_id),
        )
        .await
        .unwrap();

    let plan_change_task = tokio::spawn({
        let account_service = deps.account_service();
        async move {
            account_service
                .set_plan(
                    AccountId(account_id),
                    AccountSetPlan {
                        current_revision: AccountRevision::INITIAL,
                        plan: PlanId(destination.plan_id),
                    },
                    &AuthCtx::System,
                )
                .await
        }
    });
    wait_for_postgres_lock(&pool, "FROM plans").await;

    let clear_task = tokio::spawn({
        let repo = deps.account_resource_override_repo.clone();
        async move {
            repo.delete(
                account_id,
                AccountResourceOverrideDimension::MaxMemoryPerWorker,
            )
            .await
        }
    });
    wait_for_postgres_lock(&pool, "account_revisions.plan_id").await;
    assert!(!clear_task.is_finished());
    blocker.commit().await.unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(5), plan_change_task)
        .await
        .expect("account Plan change remained blocked")
        .unwrap()
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), clear_task)
        .await
        .expect("self-service clear remained blocked")
        .unwrap()
        .unwrap();
    assert_eq!(
        deps.account_resource_override_repo
            .get_active_value(
                account_id,
                AccountResourceOverrideDimension::MaxMemoryPerWorker,
                &SqlDateTime::now(),
            )
            .await
            .unwrap(),
        None
    );
}

pub async fn test_multi_account_expiry_cleanup_uses_deadlock_safe_lock_order(deps: &Deps) {
    let _cleanup_guard = ADMIN_GRANT_CLEANUP_TEST_LOCK.lock().await;
    let TestDb::Postgres(pool) = &deps.test_db else {
        panic!("this race depends on PostgreSQL row-lock semantics");
    };
    let pool = pool.clone();
    let mut account_ids = vec![
        deps.create_account().await.revision.account_id,
        deps.create_account().await.revision.account_id,
    ];
    account_ids.sort_unstable();
    let granted_at = SqlDateTime::now();
    let expires_at = SqlDateTime::new(granted_at.as_utc().to_owned() + chrono::Duration::hours(1));
    let cleanup_at = SqlDateTime::new(granted_at.as_utc().to_owned() + chrono::Duration::hours(2));
    for account_id in &account_ids {
        deps.account_resource_override_repo
            .upsert(AccountResourceOverrideRecord {
                account_id: *account_id,
                dimension: AccountResourceOverrideDimension::MonthlyComputeGcu,
                source: AccountResourceOverrideSource::AdminGrant,
                override_value: 3.into(),
                reason: AccountResourceOverrideReason::Support,
                expires_at: Some(expires_at.clone()),
                created_by: *account_id,
                created_at: granted_at.clone(),
            })
            .await
            .unwrap();
    }

    let mut blocker = pool
        .with_rw("test", "block_second_expired_grant_account")
        .begin()
        .await
        .unwrap();
    blocker
        .execute(
            sqlx::query("SELECT account_id FROM accounts WHERE account_id = $1 FOR UPDATE")
                .bind(account_ids[1]),
        )
        .await
        .unwrap();

    let cleanup_task = tokio::spawn({
        let repo = deps.account_resource_override_repo.clone();
        async move { repo.cleanup_expired_admin_grants(cleanup_at).await }
    });
    wait_for_postgres_lock(&pool, "account_revisions.plan_id").await;

    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        blocker.execute(
            sqlx::query("SELECT plan_id FROM plans WHERE plan_id = $1 FOR UPDATE")
                .bind(deps.test_plan_id()),
        ),
    )
    .await
    .expect("Plan lock was blocked by cleanup")
    .unwrap();
    blocker.commit().await.unwrap();

    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(5), cleanup_task)
            .await
            .expect("cleanup remained blocked")
            .unwrap()
            .unwrap(),
        2
    );
}

pub async fn test_admin_grant_cleanup_rejects_zero_interval(deps: &Deps) {
    let error = deps
        .account_resource_override_repo
        .run_admin_grant_cleanup_loop(std::time::Duration::ZERO)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Admin resource grant cleanup interval must be greater than zero"
    );
}

pub async fn test_storage_limit_discards_out_of_range_override_after_plan_update(deps: &Deps) {
    let account = deps.create_account().await;
    let account_id = account.revision.account_id;
    let mut plan = deps
        .plan_repo
        .get_by_id(account.revision.plan_id)
        .await
        .unwrap()
        .unwrap();

    deps.account_resource_override_repo
        .upsert(AccountResourceOverrideRecord {
            account_id,
            dimension: AccountResourceOverrideDimension::MaxDiskSpacePerWorker,
            source: AccountResourceOverrideSource::SelfService,
            override_value: 2000.into(),
            reason: AccountResourceOverrideReason::UserSelfServe,
            expires_at: None,
            created_by: account_id,
            created_at: SqlDateTime::now(),
        })
        .await
        .unwrap();

    plan.max_disk_space_per_worker = 500.into();
    plan.max_disk_space_per_worker_ceiling = 1000.into();
    plan.max_disk_space_per_worker_user_configurable = false;
    deps.plan_repo.create_or_update(plan).await.unwrap();

    let usage = deps
        .account_usage_repo
        .get(account_id, &SqlDateTime::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(usage.storage_limit.plan_default, Some(500));
    assert_eq!(usage.storage_limit.override_value, None);
    assert_eq!(usage.storage_limit.ceiling, Some(1000));
    assert_eq!(usage.storage_limit.effective_value, Some(500));
    assert_eq!(usage.plan.max_disk_space_per_worker.get(), 500);
    assert_eq!(usage.resource_limits().max_disk_space_per_worker, 500);

    deps.account_resource_override_repo
        .delete(
            account_id,
            AccountResourceOverrideDimension::MaxDiskSpacePerWorker,
        )
        .await
        .unwrap();
    let storage_limit = deps
        .account_usage_repo
        .get(account_id, &SqlDateTime::now())
        .await
        .unwrap()
        .unwrap()
        .storage_limit;
    assert_eq!(storage_limit.override_value, None);
    assert_eq!(storage_limit.effective_value, Some(500));

    let mut plan = deps
        .plan_repo
        .get_by_id(account.revision.plan_id)
        .await
        .unwrap()
        .unwrap();
    plan.max_disk_space_per_worker_enabled = false;
    deps.plan_repo.create_or_update(plan).await.unwrap();

    let usage = deps
        .account_usage_repo
        .get(account_id, &SqlDateTime::now())
        .await
        .unwrap()
        .unwrap();
    assert!(!usage.storage_limit.enabled);
    assert_eq!(usage.storage_limit.effective_value, None);
    assert_eq!(usage.storage_limit.plan_default, None);
    assert_eq!(usage.storage_limit.ceiling, None);
    assert_eq!(
        usage.resource_limits().max_disk_space_per_worker,
        golem_common::model::account_usage::EFFECTIVELY_UNLIMITED_STORAGE_LIMIT
    );
}

pub async fn test_plan_reseed_deletes_nonconfigurable_overrides_before_reenable(deps: &Deps) {
    let account = deps.create_account().await;
    let account_id = account.revision.account_id;
    let mut plan = RegistryServiceConfig::default()
        .initial_plans
        .remove("default")
        .unwrap();
    plan.plan_id = PlanId(account.revision.plan_id);
    plan.max_memory_per_agent = 100;
    plan.max_memory_per_agent_ceiling = 200;
    plan.max_memory_per_agent_user_configurable = true;
    plan.max_storage_per_agent_enabled = true;
    plan.max_storage_per_agent = 125;
    plan.max_storage_per_agent_ceiling = Some(200);
    plan.max_storage_per_agent_user_configurable = true;
    deps.plan_service()
        .create_initial_plans(&HashMap::from([("reseeded".to_string(), plan.clone())]))
        .await
        .unwrap();

    for (dimension, value) in [
        (AccountResourceOverrideDimension::MaxMemoryPerWorker, 150),
        (AccountResourceOverrideDimension::MaxDiskSpacePerWorker, 175),
    ] {
        deps.account_resource_override_repo
            .upsert(AccountResourceOverrideRecord {
                account_id,
                dimension,
                source: AccountResourceOverrideSource::SelfService,
                override_value: value.into(),
                reason: AccountResourceOverrideReason::UserSelfServe,
                expires_at: None,
                created_by: account_id,
                created_at: SqlDateTime::now(),
            })
            .await
            .unwrap();
    }

    plan.max_memory_per_agent_user_configurable = false;
    plan.max_storage_per_agent_user_configurable = false;

    deps.plan_service()
        .create_initial_plans(&HashMap::from([("reseeded".to_string(), plan.clone())]))
        .await
        .unwrap();

    let now = SqlDateTime::now();
    assert_eq!(
        deps.account_resource_override_repo
            .get_active_value(
                account_id,
                AccountResourceOverrideDimension::MaxMemoryPerWorker,
                &now,
            )
            .await
            .unwrap()
            .map(|value| value.get()),
        None
    );
    assert_eq!(
        deps.account_resource_override_repo
            .get_active_value(
                account_id,
                AccountResourceOverrideDimension::MaxDiskSpacePerWorker,
                &now,
            )
            .await
            .unwrap()
            .map(|value| value.get()),
        None
    );

    let usage = deps
        .account_usage_repo
        .get(account_id, &now)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(usage.max_memory_per_worker.override_value, None);
    assert_eq!(usage.max_memory_per_worker.effective_value, 100);
    assert_eq!(usage.storage_limit.override_value, None);
    assert_eq!(usage.storage_limit.effective_value, Some(125));

    plan.max_memory_per_agent_user_configurable = true;
    plan.max_storage_per_agent_user_configurable = true;
    deps.plan_service()
        .create_initial_plans(&HashMap::from([("reseeded".to_string(), plan)]))
        .await
        .unwrap();

    let usage = deps
        .account_usage_repo
        .get(account_id, &SqlDateTime::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(usage.max_memory_per_worker.override_value, None);
    assert_eq!(usage.max_memory_per_worker.effective_value, 100);
    assert_eq!(usage.storage_limit.override_value, None);
    assert_eq!(usage.storage_limit.effective_value, Some(125));
}

pub async fn test_plan_monthly_amounts_are_upserted(deps: &Deps) {
    let plan_id = deps.test_plan_id();
    let mut plan = deps.plan_repo.get_by_id(plan_id).await.unwrap().unwrap();
    plan.monthly_compute_gcu = 2.into();
    plan.monthly_memory_gb_seconds = 3.into();
    plan.monthly_durable_storage_gb_month = 5.into();
    plan.monthly_ephemeral_storage_gb_month = 7.into();
    deps.plan_repo.create_or_update(plan).await.unwrap();

    let seeded = deps.plan_repo.get_by_id(plan_id).await.unwrap().unwrap();
    assert_eq!(seeded.monthly_compute_gcu.get(), 2);
    assert_eq!(seeded.monthly_memory_gb_seconds.get(), 3);
    assert_eq!(seeded.monthly_durable_storage_gb_month.get(), 5);
    assert_eq!(seeded.monthly_ephemeral_storage_gb_month.get(), 7);

    let mut updated = seeded;
    updated.monthly_compute_gcu = 11.into();
    updated.monthly_memory_gb_seconds = 13.into();
    updated.monthly_durable_storage_gb_month = 17.into();
    updated.monthly_ephemeral_storage_gb_month = 19.into();
    deps.plan_repo.create_or_update(updated).await.unwrap();

    let loaded = deps.plan_repo.get_by_id(plan_id).await.unwrap().unwrap();
    assert_eq!(loaded.monthly_compute_gcu.get(), 11);
    assert_eq!(loaded.monthly_memory_gb_seconds.get(), 13);
    assert_eq!(loaded.monthly_durable_storage_gb_month.get(), 17);
    assert_eq!(loaded.monthly_ephemeral_storage_gb_month.get(), 19);

    let listed = deps
        .plan_repo
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|plan| plan.plan_id == plan_id)
        .expect("updated plan must be listed");
    assert_eq!(listed.monthly_compute_gcu.get(), 11);
    assert_eq!(listed.monthly_memory_gb_seconds.get(), 13);
    assert_eq!(listed.monthly_durable_storage_gb_month.get(), 17);
    assert_eq!(listed.monthly_ephemeral_storage_gb_month.get(), 19);
}

pub async fn test_plan_reseed_clamps_overrides_before_range_expansion(deps: &Deps) {
    let account = deps.create_account().await;
    let account_id = account.revision.account_id;
    let mut plan = RegistryServiceConfig::default()
        .initial_plans
        .remove("default")
        .unwrap();
    plan.plan_id = PlanId(account.revision.plan_id);
    plan.max_memory_per_agent = 25;
    plan.max_memory_per_agent_ceiling = 400;
    plan.max_memory_per_agent_user_configurable = true;
    plan.max_storage_per_agent_enabled = true;
    plan.max_storage_per_agent = 50;
    plan.max_storage_per_agent_ceiling = Some(400);
    plan.max_storage_per_agent_user_configurable = true;
    deps.plan_service()
        .create_initial_plans(&HashMap::from([("reseeded".to_string(), plan.clone())]))
        .await
        .unwrap();

    for (dimension, value) in [
        (AccountResourceOverrideDimension::MaxMemoryPerWorker, 50),
        (AccountResourceOverrideDimension::MaxDiskSpacePerWorker, 300),
    ] {
        deps.account_resource_override_repo
            .upsert(AccountResourceOverrideRecord {
                account_id,
                dimension,
                source: AccountResourceOverrideSource::SelfService,
                override_value: value.into(),
                reason: AccountResourceOverrideReason::UserSelfServe,
                expires_at: None,
                created_by: account_id,
                created_at: SqlDateTime::now(),
            })
            .await
            .unwrap();
    }

    plan.max_memory_per_agent = 100;
    plan.max_memory_per_agent_ceiling = 200;
    plan.max_storage_per_agent = 125;
    plan.max_storage_per_agent_ceiling = Some(250);

    deps.plan_service()
        .create_initial_plans(&HashMap::from([("reseeded".to_string(), plan.clone())]))
        .await
        .unwrap();

    let now = SqlDateTime::now();
    assert_eq!(
        deps.account_resource_override_repo
            .get_active_value(
                account_id,
                AccountResourceOverrideDimension::MaxMemoryPerWorker,
                &now,
            )
            .await
            .unwrap()
            .map(|value| value.get()),
        Some(100)
    );
    assert_eq!(
        deps.account_resource_override_repo
            .get_active_value(
                account_id,
                AccountResourceOverrideDimension::MaxDiskSpacePerWorker,
                &now,
            )
            .await
            .unwrap()
            .map(|value| value.get()),
        Some(250)
    );

    plan.max_memory_per_agent = 25;
    plan.max_memory_per_agent_ceiling = 400;
    plan.max_storage_per_agent = 50;
    plan.max_storage_per_agent_ceiling = Some(400);
    deps.plan_service()
        .create_initial_plans(&HashMap::from([("reseeded".to_string(), plan)]))
        .await
        .unwrap();

    let usage = deps
        .account_usage_repo
        .get(account_id, &SqlDateTime::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(usage.max_memory_per_worker.override_value, Some(100));
    assert_eq!(usage.max_memory_per_worker.effective_value, 100);
    assert_eq!(usage.storage_limit.override_value, Some(250));
    assert_eq!(usage.storage_limit.effective_value, Some(250));
}

pub async fn test_atomic_user_override_set_validates_current_policy(deps: &Deps) {
    let account = deps.create_account().await;
    let account_id = account.revision.account_id;
    let plan_id = account.revision.plan_id;
    let mut plan = deps.plan_repo.get_by_id(plan_id).await.unwrap().unwrap();
    plan.max_memory_per_worker = 100.into();
    plan.max_memory_per_worker_ceiling = 200.into();
    plan.max_memory_per_worker_user_configurable = true;
    deps.plan_repo.create_or_update(plan.clone()).await.unwrap();

    deps.account_resource_override_repo
        .set_user_override(
            account_id,
            AccountResourceOverrideDimension::MaxMemoryPerWorker,
            150,
            account_id,
        )
        .await
        .unwrap();

    plan.max_memory_per_worker = 160.into();
    plan.max_memory_per_worker_ceiling = 180.into();
    deps.plan_repo.create_or_update(plan.clone()).await.unwrap();
    assert!(matches!(
        deps.account_resource_override_repo
            .set_user_override(
                account_id,
                AccountResourceOverrideDimension::MaxMemoryPerWorker,
                150,
                account_id,
            )
            .await,
        Err(SetAccountResourceOverrideError::Policy(
            OverridePolicyViolation::BelowPlanDefault(160)
        ))
    ));
    assert_eq!(
        deps.account_resource_override_repo
            .get_active_value(
                account_id,
                AccountResourceOverrideDimension::MaxMemoryPerWorker,
                &SqlDateTime::now(),
            )
            .await
            .unwrap()
            .map(|value| value.get()),
        Some(160)
    );

    plan.max_memory_per_worker_user_configurable = false;
    deps.plan_repo.create_or_update(plan).await.unwrap();
    assert!(matches!(
        deps.account_resource_override_repo
            .set_user_override(
                account_id,
                AccountResourceOverrideDimension::MaxMemoryPerWorker,
                170,
                account_id,
            )
            .await,
        Err(SetAccountResourceOverrideError::Policy(
            OverridePolicyViolation::NotUserConfigurable
        ))
    ));
    assert_eq!(
        deps.account_resource_override_repo
            .get_active_value(
                account_id,
                AccountResourceOverrideDimension::MaxMemoryPerWorker,
                &SqlDateTime::now(),
            )
            .await
            .unwrap(),
        None
    );
}

async fn wait_for_postgres_locks(pool: &PostgresPool, query_fragment: &str, expected: i64) {
    let pattern = format!("%{query_fragment}%");
    for _ in 0..100 {
        let mut api = pool.with_ro("test", "wait_for_override_policy_lock");
        let (count,): (i64,) = api
            .fetch_one_as(
                sqlx::query_as(
                    "SELECT COUNT(*) FROM pg_stat_activity WHERE wait_event_type = 'Lock' AND query LIKE $1",
                )
                .bind(&pattern),
            )
            .await
            .unwrap();
        if count >= expected {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {expected} PostgreSQL locks matching {query_fragment}");
}

async fn wait_for_postgres_lock(pool: &PostgresPool, query_fragment: &str) {
    wait_for_postgres_locks(pool, query_fragment, 1).await;
}

pub async fn test_atomic_user_override_set_racing_account_plan_change(deps: &Deps) {
    let TestDb::Postgres(pool) = &deps.test_db else {
        panic!("this race depends on PostgreSQL row-lock semantics");
    };
    let pool = pool.clone();
    let account = deps.create_account().await;
    let account_id = account.revision.account_id;
    let mut destination_plan = deps
        .plan_repo
        .get_by_id(account.revision.plan_id)
        .await
        .unwrap()
        .unwrap();
    let destination_plan_id = new_repo_uuid();
    destination_plan.plan_id = destination_plan_id;
    destination_plan.name = format!("NONCONFIGURABLE_PLAN_{destination_plan_id}");
    destination_plan.max_memory_per_worker = 100.into();
    destination_plan.max_memory_per_worker_ceiling = 200.into();
    destination_plan.max_memory_per_worker_user_configurable = false;
    deps.plan_repo
        .create_or_update(destination_plan)
        .await
        .unwrap();

    let mut blocker = pool
        .with_rw("test", "block_nonconfigurable_account_plan_change")
        .begin()
        .await
        .unwrap();
    blocker
        .execute(
            sqlx::query("SELECT plan_id FROM plans WHERE plan_id = $1 FOR UPDATE")
                .bind(destination_plan_id),
        )
        .await
        .unwrap();

    let plan_change_task = tokio::spawn({
        let account_service = deps.account_service();
        async move {
            account_service
                .set_plan(
                    AccountId(account_id),
                    AccountSetPlan {
                        current_revision: AccountRevision::INITIAL,
                        plan: PlanId(destination_plan_id),
                    },
                    &AuthCtx::System,
                )
                .await
        }
    });
    wait_for_postgres_lock(&pool, "FROM plans").await;

    let set_task = tokio::spawn({
        let override_repo = deps.account_resource_override_repo.clone();
        async move {
            override_repo
                .set_user_override(
                    account_id,
                    AccountResourceOverrideDimension::MaxMemoryPerWorker,
                    150,
                    account_id,
                )
                .await
        }
    });
    wait_for_postgres_lock(&pool, "SELECT account_id").await;
    blocker.commit().await.unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(5), plan_change_task)
        .await
        .expect("account Plan change remained blocked")
        .unwrap()
        .unwrap();
    let set_result = tokio::time::timeout(std::time::Duration::from_secs(5), set_task)
        .await
        .expect("override set remained blocked")
        .unwrap();
    assert!(matches!(
        set_result,
        Err(SetAccountResourceOverrideError::Policy(
            OverridePolicyViolation::NotUserConfigurable
        ))
    ));
    assert_eq!(
        deps.account_resource_override_repo
            .get_active_value(
                account_id,
                AccountResourceOverrideDimension::MaxMemoryPerWorker,
                &SqlDateTime::now(),
            )
            .await
            .unwrap(),
        None
    );

    let mut expanded = deps
        .plan_repo
        .get_by_id(destination_plan_id)
        .await
        .unwrap()
        .unwrap();
    expanded.max_memory_per_worker_user_configurable = true;
    deps.plan_repo.create_or_update(expanded).await.unwrap();
    assert_eq!(
        deps.account_resource_override_repo
            .get_active_value(
                account_id,
                AccountResourceOverrideDimension::MaxMemoryPerWorker,
                &SqlDateTime::now(),
            )
            .await
            .unwrap(),
        None
    );
}

pub async fn test_atomic_user_override_set_racing_range_shrink(deps: &Deps) {
    let TestDb::Postgres(pool) = &deps.test_db else {
        panic!("this race depends on PostgreSQL row-lock semantics");
    };
    let pool = pool.clone();
    let account = deps.create_account().await;
    let account_id = account.revision.account_id;
    let plan_id = account.revision.plan_id;
    let mut plan = deps.plan_repo.get_by_id(plan_id).await.unwrap().unwrap();
    plan.max_memory_per_worker = 100.into();
    plan.max_memory_per_worker_ceiling = 300.into();
    plan.max_memory_per_worker_user_configurable = true;
    deps.plan_repo.create_or_update(plan.clone()).await.unwrap();

    let mut blocker = pool
        .with_rw("test", "block_range_shrink_plan_update")
        .begin()
        .await
        .unwrap();
    blocker
        .execute(
            sqlx::query("SELECT plan_id FROM plans WHERE plan_id = $1 FOR UPDATE").bind(plan_id),
        )
        .await
        .unwrap();

    let set_task = tokio::spawn({
        let override_repo = deps.account_resource_override_repo.clone();
        async move {
            override_repo
                .set_user_override(
                    account_id,
                    AccountResourceOverrideDimension::MaxMemoryPerWorker,
                    250,
                    account_id,
                )
                .await
        }
    });
    wait_for_postgres_lock(&pool, "FROM plans").await;

    plan.max_memory_per_worker_ceiling = 150.into();
    let policy_task = tokio::spawn({
        let plan_repo = DbPlanRepo::new(pool.clone());
        async move { plan_repo.create_or_update(plan).await }
    });
    wait_for_postgres_lock(&pool, "SELECT accounts.account_id").await;
    blocker.commit().await.unwrap();

    let set_policy = tokio::time::timeout(std::time::Duration::from_secs(5), set_task)
        .await
        .expect("override set remained blocked")
        .unwrap()
        .unwrap();
    assert_eq!(set_policy.ceiling, 300);
    tokio::time::timeout(std::time::Duration::from_secs(5), policy_task)
        .await
        .expect("policy update remained blocked")
        .unwrap()
        .unwrap();
    assert_eq!(
        deps.account_resource_override_repo
            .get_active_value(
                account_id,
                AccountResourceOverrideDimension::MaxMemoryPerWorker,
                &SqlDateTime::now(),
            )
            .await
            .unwrap()
            .map(|value| value.get()),
        Some(150)
    );

    let mut expanded = deps.plan_repo.get_by_id(plan_id).await.unwrap().unwrap();
    expanded.max_memory_per_worker_ceiling = 300.into();
    deps.plan_repo.create_or_update(expanded).await.unwrap();
    assert_eq!(
        deps.account_resource_override_repo
            .get_active_value(
                account_id,
                AccountResourceOverrideDimension::MaxMemoryPerWorker,
                &SqlDateTime::now(),
            )
            .await
            .unwrap()
            .map(|value| value.get()),
        Some(150)
    );
}

async fn create_disk_override_plan(deps: &Deps, account_id: Uuid, user_configurable: bool) -> Uuid {
    let destination_plan_id = new_repo_uuid();
    deps.plan_repo
        .create_or_update(PlanRecord {
            plan_id: destination_plan_id,
            name: format!("DISK_OVERRIDE_PLAN_{destination_plan_id}"),
            total_app_count: 3.into(),
            total_env_count: 10.into(),
            total_component_count: 15.into(),
            total_worker_connection_count: 25.into(),
            total_component_storage_bytes: 1000.into(),
            monthly_gas_limit: 2000.into(),
            monthly_component_upload_limit_bytes: 3000.into(),
            max_memory_per_worker: 1024.into(),
            max_memory_per_worker_ceiling: 2048.into(),
            max_memory_per_worker_user_configurable: user_configurable,
            monthly_compute_gcu: 1.into(),
            monthly_memory_gb_seconds: 1024.into(),
            monthly_durable_storage_gb_month: 2.into(),
            monthly_ephemeral_storage_gb_month: 3.into(),
            overage_eligible: false,
            max_table_elements_per_worker: 16384.into(),
            max_disk_space_per_worker_enabled: true,
            max_disk_space_per_worker: 1024.into(),
            max_disk_space_per_worker_ceiling: 2048.into(),
            max_disk_space_per_worker_user_configurable: user_configurable,
            per_invocation_http_call_limit: u64::MAX.into(),
            per_invocation_rpc_call_limit: u64::MAX.into(),
            monthly_http_call_limit: 5000.into(),
            monthly_rpc_call_limit: 5000.into(),
            max_concurrent_agents_per_executor: 1_000_000_000_000_000_000u64.into(),
            oplog_writes_per_second: 1_000_000_000_000_000_000u64.into(),
        })
        .await
        .unwrap();
    deps.account_resource_override_repo
        .upsert(AccountResourceOverrideRecord {
            account_id,
            dimension: AccountResourceOverrideDimension::MaxDiskSpacePerWorker,
            source: AccountResourceOverrideSource::SelfService,
            override_value: 512.into(),
            reason: AccountResourceOverrideReason::UserSelfServe,
            expires_at: None,
            created_by: account_id,
            created_at: SqlDateTime::now(),
        })
        .await
        .unwrap();
    deps.account_resource_override_repo
        .upsert(AccountResourceOverrideRecord {
            account_id,
            dimension: AccountResourceOverrideDimension::MaxMemoryPerWorker,
            source: AccountResourceOverrideSource::SelfService,
            override_value: 4096.into(),
            reason: AccountResourceOverrideReason::UserSelfServe,
            expires_at: None,
            created_by: account_id,
            created_at: SqlDateTime::now(),
        })
        .await
        .unwrap();

    destination_plan_id
}

pub async fn test_plan_change_clamps_disk_override(deps: &Deps) {
    let account = deps.create_account().await;
    let destination_plan_id =
        create_disk_override_plan(deps, account.revision.account_id, true).await;

    deps.account_service()
        .set_plan(
            AccountId(account.revision.account_id),
            AccountSetPlan {
                current_revision: AccountRevision::INITIAL,
                plan: PlanId(destination_plan_id),
            },
            &AuthCtx::System,
        )
        .await
        .unwrap();

    let storage_limit = deps
        .account_usage_repo
        .get(account.revision.account_id, &SqlDateTime::now())
        .await
        .unwrap()
        .unwrap()
        .storage_limit;
    assert_eq!(storage_limit.override_value, Some(1024));
    assert_eq!(storage_limit.effective_value, Some(1024));
    let usage = deps
        .account_usage_repo
        .get(account.revision.account_id, &SqlDateTime::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(usage.max_memory_per_worker.override_value, Some(2048));
    assert_eq!(usage.max_memory_per_worker.effective_value, 2048);
}

pub async fn test_plan_change_clears_forbidden_disk_override(deps: &Deps) {
    let account = deps.create_account().await;
    let destination_plan_id =
        create_disk_override_plan(deps, account.revision.account_id, false).await;

    deps.account_service()
        .set_plan(
            AccountId(account.revision.account_id),
            AccountSetPlan {
                current_revision: AccountRevision::INITIAL,
                plan: PlanId(destination_plan_id),
            },
            &AuthCtx::System,
        )
        .await
        .unwrap();

    let storage_limit = deps
        .account_usage_repo
        .get(account.revision.account_id, &SqlDateTime::now())
        .await
        .unwrap()
        .unwrap()
        .storage_limit;
    assert_eq!(storage_limit.override_value, None);
    assert_eq!(storage_limit.effective_value, Some(1024));
    let usage = deps
        .account_usage_repo
        .get(account.revision.account_id, &SqlDateTime::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(usage.max_memory_per_worker.override_value, None);
    assert_eq!(usage.max_memory_per_worker.effective_value, 1024);
}

pub async fn test_account_usage(deps: &Deps) {
    let user = deps.create_account().await;
    let now = SqlDateTime::now();

    let mut usage = deps
        .account_usage_repo
        .get(user.revision.account_id, &now)
        .await
        .unwrap()
        .unwrap();

    for usage_type in UsageType::iter() {
        let limit: u64 = match usage_type {
            UsageType::TotalAppCount => 3,
            UsageType::TotalEnvCount => 10,
            UsageType::TotalComponentCount => 15,
            UsageType::TotalWorkerConnectionCount => 25,
            UsageType::TotalComponentStorageBytes => 1000,
            UsageType::MonthlyGasLimit => 2000,
            UsageType::MonthlyComponentUploadLimitBytes => 3000,
            UsageType::MonthlyHttpCalls => 5000,
            UsageType::MonthlyRpcCalls => 5000,
            UsageType::MonthlyDurableAgentStorageByteSeconds
            | UsageType::MonthlyEphemeralStorageByteSeconds => u64::MAX,
            UsageType::MonthlyMemoryGbSeconds => 6000,
        };
        let plan_limit = usage.plan.limit(usage_type);
        assert!(plan_limit == limit);

        check!(usage.usage(usage_type) == 0, "{usage_type:?}");
        assert!(usage.add_change(usage_type, 1));
        check!(usage.change(usage_type) == 1, "{usage_type:?}");
    }

    let increased_usage = usage;

    {
        deps.account_usage_repo.add(&increased_usage).await.unwrap();
        let usage = deps
            .account_usage_repo
            .get(user.revision.account_id, &now)
            .await
            .unwrap()
            .unwrap();
        for usage_type in UsageType::iter() {
            if usage_type.tracking() == UsageTracking::Stats {
                check!(usage.usage(usage_type) == 1, "{usage_type:?}");
            } else {
                check!(usage.usage(usage_type) == 0, "{usage_type:?}");
            }
            check!(usage.change(usage_type) == 0, "{usage_type:?}");
        }
    }

    {
        deps.account_usage_repo.add(&increased_usage).await.unwrap();
        deps.account_usage_repo.add(&increased_usage).await.unwrap();
        let usage = deps
            .account_usage_repo
            .get(user.revision.account_id, &now)
            .await
            .unwrap()
            .unwrap();

        for usage_type in UsageType::iter() {
            if usage_type.tracking() == UsageTracking::Stats {
                check!(usage.usage(usage_type) == 3, "{usage_type:?}");
            } else {
                check!(usage.usage(usage_type) == 0, "{usage_type:?}");
            }
            check!(usage.change(usage_type) == 0, "{usage_type:?}");
        }
    }

    {
        let mut usage = deps
            .account_usage_repo
            .get(user.revision.account_id, &now)
            .await
            .unwrap()
            .unwrap();

        for usage_type in UsageType::iter() {
            let within_limit = matches!(
                usage_type,
                UsageType::MonthlyDurableAgentStorageByteSeconds
                    | UsageType::MonthlyEphemeralStorageByteSeconds
            );
            check!(usage.add_change(usage_type, 1000000) == within_limit);
        }
    }

    {
        let mut usage = deps
            .account_usage_repo
            .get(user.revision.account_id, &now)
            .await
            .unwrap()
            .unwrap();
        for usage_type in UsageType::iter() {
            usage.add_change(usage_type, -2);
        }
        deps.account_usage_repo.add(&usage).await.unwrap();

        let usage = deps
            .account_usage_repo
            .get(user.revision.account_id, &now)
            .await
            .unwrap()
            .unwrap();
        for usage_type in UsageType::iter() {
            if usage_type.tracking() == UsageTracking::Stats {
                check!(usage.usage(usage_type) == 1, "{usage_type:?}");
            }
        }
    }

    {
        let app = deps
            .application_repo
            .create(
                user.revision.account_id,
                ApplicationRevisionRecord {
                    application_id: new_repo_uuid(),
                    revision_id: 0,
                    name: "test-app".to_string(),
                    audit: DeletableRevisionAuditFields::new(user.revision.account_id),
                },
            )
            .await
            .unwrap();

        let env_revision = EnvironmentRevisionRecord {
            environment_id: new_repo_uuid(),
            revision_id: 0,
            name: "env".to_string(),
            hash: SqlBlake3Hash::empty(),
            audit: DeletableRevisionAuditFields::new(user.revision.account_id),
            compatibility_check: false,
            version_check: false,
            security_overrides: false,
        };
        let env = deps
            .environment_repo
            .create(
                app.revision.application_id,
                env_revision.clone(),
                test_environment_default_card_record(env_revision.environment_id),
            )
            .await
            .unwrap();
        let _component = deps
            .component_repo
            .create(
                env.revision.environment_id,
                "component",
                ComponentRevisionRecord {
                    component_id: Default::default(),
                    revision_id: 0,
                    hash: SqlBlake3Hash::empty(),
                    audit: DeletableRevisionAuditFields::new(user.revision.account_id),
                    size: 0.into(),
                    metadata: Blob::new(ComponentMetadata::from_parts(
                        KnownExports::default(),
                        vec![],
                        None,
                        None,
                        vec![],
                        BTreeMap::new(),
                    )),
                    object_store_key: "".to_string(),
                    binary_hash: SqlBlake3Hash::empty(),
                },
                Vec::new(),
            )
            .await
            .unwrap();

        let usage = deps
            .account_usage_repo
            .get(user.revision.account_id, &now)
            .await
            .unwrap()
            .unwrap();
        check!(usage.usage(UsageType::TotalAppCount) == 1);
        check!(usage.usage(UsageType::TotalEnvCount) == 1);
        check!(usage.usage(UsageType::TotalComponentCount) == 1);
    }
}

pub async fn test_account_usage_history(deps: &Deps) {
    use golem_service_base::model::auth::AuthCtx;

    let account = deps.create_account().await;
    let previous_month = SqlDateTime::new(Utc::now() - chrono::Duration::days(40));
    let older_month = SqlDateTime::new(Utc::now() - chrono::Duration::days(80));
    let mut usage = deps
        .account_usage_repo
        .get(account.revision.account_id, &previous_month)
        .await
        .unwrap()
        .unwrap();
    usage.add_change(UsageType::MonthlyDurableAgentStorageByteSeconds, 123);
    usage.add_change(UsageType::MonthlyEphemeralStorageByteSeconds, 456);
    usage.add_change(UsageType::MonthlyGasLimit, 789);
    usage.add_change(UsageType::MonthlyMemoryGbSeconds, 321);
    usage.metering = Some(
        golem_service_base::clients::registry::ResourceUsageMetering {
            compute: true,
            memory: false,
            filesystem: true,
        },
    );
    deps.account_usage_repo.add(&usage).await.unwrap();

    let mut older_usage = deps
        .account_usage_repo
        .get(account.revision.account_id, &older_month)
        .await
        .unwrap()
        .unwrap();
    older_usage.add_change(UsageType::MonthlyDurableAgentStorageByteSeconds, 999);
    deps.account_usage_repo.add(&older_usage).await.unwrap();

    let history = deps
        .account_usage_repo
        .get_usage_history(
            account.revision.account_id,
            golem_common::model::account_usage::AccountUsagePeriod {
                year: Utc::now().year(),
                month: Utc::now().month(),
            },
            1,
        )
        .await
        .unwrap();

    assert_eq!(history.len(), 1);
    assert_eq!(history[0].durable_storage_byte_seconds, 123);
    assert_eq!(history[0].ephemeral_storage_byte_seconds, 456);
    assert_eq!(history[0].compute_fuel, 789);
    assert_eq!(history[0].memory_gb_seconds, 321);
    assert_eq!(
        history[0].metering,
        Some(ResourceUsageMetering {
            compute: true,
            memory: false,
            filesystem: true,
        })
    );

    let service_history = deps
        .account_usage_service()
        .get_usage_history(AccountId(account.revision.account_id), 1, &AuthCtx::System)
        .await
        .unwrap();
    assert_eq!(service_history.len(), 1);
    assert_eq!(service_history[0].usage.memory_gb_seconds, 321);
    assert_eq!(
        service_history[0].usage.metering.memory,
        golem_common::model::account_usage::MeteringStatus::Disabled
    );
}

pub async fn test_monthly_usage_mode_transitions(deps: &Deps) {
    let mut plan = deps
        .plan_repo
        .get_by_id(deps.test_plan_id())
        .await
        .unwrap()
        .unwrap();
    plan.overage_eligible = true;
    deps.plan_repo.create_or_update(plan.clone()).await.unwrap();

    let account = deps.create_account().await;
    let account_id = account.revision.account_id;
    let initial = deps
        .account_usage_repo
        .get_monthly_usage_mode(account_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(initial.mode, MonthlyUsageMode::HardLimit);
    assert!(initial.overage_eligible);
    assert!(initial.latest_owner_transition.is_none());

    let mut usage = deps
        .account_usage_repo
        .get(account_id, &SqlDateTime::now())
        .await
        .unwrap()
        .unwrap();
    usage.add_change(UsageType::MonthlyGasLimit, 11);
    usage.add_change(UsageType::MonthlyMemoryGbSeconds, 13);
    usage.add_change(UsageType::MonthlyDurableAgentStorageByteSeconds, 17);
    usage.add_change(UsageType::MonthlyEphemeralStorageByteSeconds, 19);
    deps.account_usage_repo.add(&usage).await.unwrap();

    let enabled = deps
        .account_usage_repo
        .set_monthly_usage_mode(
            account_id,
            MonthlyUsageMode::AllowOverage,
            account_id,
            MonthlyUsageModeTransitionSource::Owner,
        )
        .await
        .unwrap();
    assert_eq!(enabled.previous_mode, MonthlyUsageMode::HardLimit);
    assert_eq!(enabled.new_mode, MonthlyUsageMode::AllowOverage);
    assert_eq!(enabled.revision, 1);
    assert_eq!(enabled.source, MonthlyUsageModeTransitionSource::Owner);
    assert_eq!(enabled.usage_baseline.compute_fuel, 11);
    assert_eq!(enabled.usage_baseline.memory_gb_seconds, 13);
    assert_eq!(enabled.usage_baseline.durable_storage_byte_seconds, 17);
    assert_eq!(enabled.usage_baseline.ephemeral_storage_byte_seconds, 19);

    assert!(matches!(
        deps.account_usage_repo
            .set_monthly_usage_mode(
                account_id,
                MonthlyUsageMode::AllowOverage,
                account_id,
                MonthlyUsageModeTransitionSource::Owner,
            )
            .await,
        Err(SetMonthlyUsageModeError::ModeUnchanged(
            MonthlyUsageMode::AllowOverage
        ))
    ));
    assert_eq!(
        deps.account_usage_repo
            .get_monthly_usage_mode_transitions(account_id)
            .await
            .unwrap()
            .len(),
        1
    );

    let mut additional_usage = deps
        .account_usage_repo
        .get(account_id, &SqlDateTime::now())
        .await
        .unwrap()
        .unwrap();
    additional_usage.add_change(UsageType::MonthlyGasLimit, 2);
    additional_usage.add_change(UsageType::MonthlyMemoryGbSeconds, 3);
    additional_usage.add_change(UsageType::MonthlyDurableAgentStorageByteSeconds, 5);
    additional_usage.add_change(UsageType::MonthlyEphemeralStorageByteSeconds, 7);
    deps.account_usage_repo
        .add(&additional_usage)
        .await
        .unwrap();

    let disabled = deps
        .account_usage_repo
        .set_monthly_usage_mode(
            account_id,
            MonthlyUsageMode::HardLimit,
            account_id,
            MonthlyUsageModeTransitionSource::Owner,
        )
        .await
        .unwrap();
    assert_eq!(disabled.usage_baseline.compute_fuel, 13);
    assert_eq!(disabled.revision, 2);
    assert_eq!(disabled.usage_baseline.memory_gb_seconds, 16);
    assert_eq!(disabled.usage_baseline.durable_storage_byte_seconds, 22);
    assert_eq!(disabled.usage_baseline.ephemeral_storage_byte_seconds, 26);

    deps.account_usage_repo
        .set_monthly_usage_mode(
            account_id,
            MonthlyUsageMode::AllowOverage,
            account_id,
            MonthlyUsageModeTransitionSource::Owner,
        )
        .await
        .unwrap();
    let administrator_disabled = deps
        .account_usage_repo
        .set_monthly_usage_mode(
            account_id,
            MonthlyUsageMode::HardLimit,
            AccountId::SYSTEM.0,
            MonthlyUsageModeTransitionSource::Administrator,
        )
        .await
        .unwrap();
    assert_eq!(
        administrator_disabled.source,
        MonthlyUsageModeTransitionSource::Administrator
    );
    deps.account_usage_repo
        .set_monthly_usage_mode(
            account_id,
            MonthlyUsageMode::AllowOverage,
            account_id,
            MonthlyUsageModeTransitionSource::Owner,
        )
        .await
        .unwrap();
    plan.overage_eligible = false;
    deps.plan_repo.create_or_update(plan).await.unwrap();

    let state = deps
        .account_usage_repo
        .get_monthly_usage_mode(account_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.mode, MonthlyUsageMode::HardLimit);
    assert!(!state.overage_eligible);
    assert!(matches!(
        deps.account_usage_repo
            .set_monthly_usage_mode(
                account_id,
                MonthlyUsageMode::AllowOverage,
                account_id,
                MonthlyUsageModeTransitionSource::Owner,
            )
            .await,
        Err(SetMonthlyUsageModeError::OverageNotEligible)
    ));
    assert_eq!(
        state.latest_owner_transition.unwrap().new_mode,
        MonthlyUsageMode::AllowOverage
    );
    let transitions = deps
        .account_usage_repo
        .get_monthly_usage_mode_transitions(account_id)
        .await
        .unwrap();
    assert_eq!(transitions.len(), 6);
    assert_eq!(
        transitions
            .iter()
            .map(|transition| transition.revision)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5, 6]
    );
    assert_eq!(
        transitions[3].source,
        MonthlyUsageModeTransitionSource::Administrator
    );
    assert_eq!(
        transitions.last().unwrap().source,
        MonthlyUsageModeTransitionSource::PlanEligibilityRemoved
    );
    assert_eq!(
        transitions.last().unwrap().new_mode,
        MonthlyUsageMode::HardLimit
    );

    let mut eligible_plan = deps
        .plan_repo
        .get_by_id(deps.test_plan_id())
        .await
        .unwrap()
        .unwrap();
    eligible_plan.overage_eligible = true;
    deps.plan_repo
        .create_or_update(eligible_plan.clone())
        .await
        .unwrap();
    let assigned_account = deps.create_account().await;
    deps.account_usage_repo
        .set_monthly_usage_mode(
            assigned_account.revision.account_id,
            MonthlyUsageMode::AllowOverage,
            assigned_account.revision.account_id,
            MonthlyUsageModeTransitionSource::Owner,
        )
        .await
        .unwrap();
    let ineligible_plan_id = new_repo_uuid();
    eligible_plan.plan_id = ineligible_plan_id;
    eligible_plan.name = format!("INELIGIBLE_PLAN_{ineligible_plan_id}");
    eligible_plan.overage_eligible = false;
    deps.plan_repo
        .create_or_update(eligible_plan)
        .await
        .unwrap();
    deps.account_service()
        .set_plan(
            AccountId(assigned_account.revision.account_id),
            AccountSetPlan {
                current_revision: AccountRevision::INITIAL,
                plan: PlanId(ineligible_plan_id),
            },
            &AuthCtx::System,
        )
        .await
        .unwrap();
    let assigned_state = deps
        .account_usage_repo
        .get_monthly_usage_mode(assigned_account.revision.account_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(assigned_state.mode, MonthlyUsageMode::HardLimit);
    assert!(!assigned_state.overage_eligible);
    let assigned_transitions = deps
        .account_usage_repo
        .get_monthly_usage_mode_transitions(assigned_account.revision.account_id)
        .await
        .unwrap();
    assert_eq!(assigned_transitions.len(), 2);
    assert_eq!(
        assigned_transitions.last().unwrap().source,
        MonthlyUsageModeTransitionSource::IneligiblePlanAssigned
    );
}

pub async fn test_plan_eligibility_downgrade_serializes_with_account_assignment(deps: &Deps) {
    let TestDb::Postgres(pool) = &deps.test_db else {
        panic!("this race depends on PostgreSQL row-lock semantics");
    };
    let pool = pool.clone();

    let mut source_plan = deps
        .plan_repo
        .get_by_id(deps.test_plan_id())
        .await
        .unwrap()
        .unwrap();
    source_plan.overage_eligible = true;
    deps.plan_repo
        .create_or_update(source_plan.clone())
        .await
        .unwrap();

    let destination_plan_id = new_repo_uuid();
    let mut destination_plan = source_plan;
    destination_plan.plan_id = destination_plan_id;
    destination_plan.name = format!("OVERAGE_PLAN_{destination_plan_id}");
    deps.plan_repo
        .create_or_update(destination_plan.clone())
        .await
        .unwrap();

    let account = deps.create_account().await;
    let account_id = account.revision.account_id;
    deps.account_usage_repo
        .set_monthly_usage_mode(
            account_id,
            MonthlyUsageMode::AllowOverage,
            account_id,
            MonthlyUsageModeTransitionSource::Owner,
        )
        .await
        .unwrap();

    let mut blocker = pool
        .with_rw("test", "block_plan_eligibility_downgrade")
        .begin()
        .await
        .unwrap();
    blocker
        .execute(
            sqlx::query("SELECT plan_id FROM plans WHERE plan_id = $1 FOR UPDATE")
                .bind(destination_plan_id),
        )
        .await
        .unwrap();

    let assignment_task = tokio::spawn({
        let account_service = deps.account_service();
        async move {
            account_service
                .set_plan(
                    AccountId(account_id),
                    AccountSetPlan {
                        current_revision: AccountRevision::INITIAL,
                        plan: PlanId(destination_plan_id),
                    },
                    &AuthCtx::System,
                )
                .await
        }
    });
    wait_for_postgres_lock(&pool, "FROM plans").await;

    destination_plan.overage_eligible = false;
    let downgrade_task = tokio::spawn({
        let plan_repo = DbPlanRepo::new(pool.clone());
        async move { plan_repo.create_or_update(destination_plan).await }
    });
    wait_for_postgres_locks(&pool, "plans", 2).await;

    let grant_task = tokio::spawn({
        let repo = deps.account_resource_override_repo.clone();
        async move {
            repo.set_admin_grant(
                account_id,
                AccountResourceOverrideDimension::MonthlyComputeGcu,
                3,
                AdminResourceGrantReason::Support,
                None,
                account_id,
            )
            .await
        }
    });
    wait_for_postgres_lock(&pool, "FROM accounts").await;
    blocker.commit().await.unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(5), assignment_task)
        .await
        .expect("account Plan assignment remained blocked")
        .unwrap()
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), downgrade_task)
        .await
        .expect("Plan eligibility downgrade remained blocked")
        .unwrap()
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), grant_task)
        .await
        .expect("admin grant remained blocked")
        .unwrap()
        .unwrap();

    let state = deps
        .account_usage_repo
        .get_monthly_usage_mode(account_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.mode, MonthlyUsageMode::HardLimit);
    assert!(!state.overage_eligible);
    let transitions = deps
        .account_usage_repo
        .get_monthly_usage_mode_transitions(account_id)
        .await
        .unwrap();
    assert_eq!(transitions.len(), 2);
    assert_eq!(
        transitions.last().unwrap().source,
        MonthlyUsageModeTransitionSource::PlanEligibilityRemoved
    );

    let mut destination_plan = deps
        .plan_repo
        .get_by_id(destination_plan_id)
        .await
        .unwrap()
        .unwrap();
    destination_plan.overage_eligible = true;
    deps.plan_repo
        .create_or_update(destination_plan)
        .await
        .unwrap();
    assert_eq!(
        deps.account_usage_repo
            .get_monthly_usage_mode(account_id)
            .await
            .unwrap()
            .unwrap()
            .mode,
        MonthlyUsageMode::HardLimit
    );
}

async fn record_compute_usage_at_revision(deps: &Deps, account_id: Uuid, revision: u64, fuel: i64) {
    let mut usage = deps
        .account_usage_repo
        .get(account_id, &SqlDateTime::now())
        .await
        .unwrap()
        .unwrap();
    usage.monthly_usage_attribution = Some(MonthlyUsageAttribution {
        revision,
        memory_byte_nanoseconds_remainder: 0,
        durable_storage_byte_nanoseconds_remainder: 0,
        ephemeral_storage_byte_nanoseconds_remainder: 0,
    });
    usage.add_change(UsageType::MonthlyGasLimit, fuel);
    deps.account_usage_repo.add(&usage).await.unwrap();
}

async fn compute_usage_by_revision(deps: &Deps, account_id: Uuid) -> Vec<(i64, i64)> {
    match &deps.test_db {
        TestDb::Postgres(pool) => pool
            .with_ro("test", "compute_usage_by_revision")
            .fetch_all_as(
                sqlx::query_as(
                    "SELECT CAST(revision AS BIGINT), CAST(SUM(compute_fuel_delta) AS BIGINT) FROM account_monthly_usage_mode_attribution WHERE account_id = $1 GROUP BY revision ORDER BY revision",
                )
                .bind(account_id),
            )
            .await
            .unwrap(),
        TestDb::Sqlite(pool) => pool
            .with_ro("test", "compute_usage_by_revision")
            .fetch_all_as(
                sqlx::query_as(
                    "SELECT CAST(revision AS BIGINT), CAST(SUM(compute_fuel_delta) AS BIGINT) FROM account_monthly_usage_mode_attribution WHERE account_id = $1 GROUP BY revision ORDER BY revision",
                )
                .bind(account_id),
            )
            .await
            .unwrap(),
    }
}

async fn fractional_usage_by_revision(deps: &Deps, account_id: Uuid) -> Vec<(i64, i64, i64, i64)> {
    let query = "SELECT CAST(revision AS BIGINT), CAST(SUM(memory_byte_nanoseconds_remainder) AS BIGINT), CAST(SUM(durable_storage_byte_nanoseconds_remainder) AS BIGINT), CAST(SUM(ephemeral_storage_byte_nanoseconds_remainder) AS BIGINT) FROM account_monthly_usage_mode_attribution WHERE account_id = $1 GROUP BY revision ORDER BY revision";
    match &deps.test_db {
        TestDb::Postgres(pool) => pool
            .with_ro("test", "fractional_usage_by_revision")
            .fetch_all_as(sqlx::query_as(query).bind(account_id))
            .await
            .unwrap(),
        TestDb::Sqlite(pool) => pool
            .with_ro("test", "fractional_usage_by_revision")
            .fetch_all_as(sqlx::query_as(query).bind(account_id))
            .await
            .unwrap(),
    }
}

async fn direct_consent_insert_is_rejected(
    deps: &Deps,
    account_id: Uuid,
    revision: i64,
    actor_account_id: Uuid,
    source: &str,
) -> bool {
    let sql = "INSERT INTO account_monthly_usage_mode_transitions (transition_id, account_id, revision, actor_account_id, source, changed_at, previous_mode, new_mode, period_year, period_month, compute_fuel, memory_gb_seconds, durable_storage_byte_seconds, ephemeral_storage_byte_seconds) VALUES ($1, $2, $3, $4, $5, CURRENT_TIMESTAMP, 'hard_limit', 'allow_overage', 2026, 1, 0, 0, 0, 0)";
    match &deps.test_db {
        TestDb::Postgres(pool) => pool
            .with_rw("test", "direct_invalid_consent_insert")
            .execute(
                sqlx::query(sql)
                    .bind(Uuid::new_v4())
                    .bind(account_id)
                    .bind(revision)
                    .bind(actor_account_id)
                    .bind(source),
            )
            .await
            .is_err(),
        TestDb::Sqlite(pool) => pool
            .with_rw("test", "direct_invalid_consent_insert")
            .execute(
                sqlx::query(sql)
                    .bind(Uuid::new_v4())
                    .bind(account_id)
                    .bind(revision)
                    .bind(actor_account_id)
                    .bind(source),
            )
            .await
            .is_err(),
    }
}

pub async fn test_monthly_usage_mode_consent_invariants(deps: &Deps) {
    let mut plan = deps
        .plan_repo
        .get_by_id(deps.test_plan_id())
        .await
        .unwrap()
        .unwrap();
    plan.overage_eligible = true;
    deps.plan_repo.create_or_update(plan).await.unwrap();

    let account_id = deps.create_account().await.revision.account_id;
    let other_account_id = deps.create_account().await.revision.account_id;
    assert!(matches!(
        deps.account_usage_repo
            .set_monthly_usage_mode(
                account_id,
                MonthlyUsageMode::AllowOverage,
                account_id,
                MonthlyUsageModeTransitionSource::Administrator,
            )
            .await,
        Err(SetMonthlyUsageModeError::InvalidConsentSource)
    ));
    assert!(matches!(
        deps.account_usage_repo
            .set_monthly_usage_mode(
                account_id,
                MonthlyUsageMode::AllowOverage,
                other_account_id,
                MonthlyUsageModeTransitionSource::Owner,
            )
            .await,
        Err(SetMonthlyUsageModeError::InvalidConsentActor)
    ));

    assert!(
        direct_consent_insert_is_rejected(deps, account_id, 100, account_id, "administrator",)
            .await
    );
    assert!(
        direct_consent_insert_is_rejected(deps, account_id, 101, other_account_id, "owner").await
    );
}

pub async fn test_monthly_usage_attribution_uses_accrual_revision(deps: &Deps) {
    use golem_registry_service::services::account_usage::ResourceUsageUpdate;
    use golem_service_base::clients::registry::ResourceUsageMetering;
    use golem_service_base::model::auth::AuthCtx;

    let mut plan = deps
        .plan_repo
        .get_by_id(deps.test_plan_id())
        .await
        .unwrap()
        .unwrap();
    plan.overage_eligible = true;
    deps.plan_repo.create_or_update(plan).await.unwrap();

    let account_id = deps.create_account().await.revision.account_id;
    let mut zero_usage = deps
        .account_usage_repo
        .get(account_id, &SqlDateTime::now())
        .await
        .unwrap()
        .unwrap();
    zero_usage.add_change(UsageType::MonthlyGasLimit, 0);
    deps.account_usage_repo.add(&zero_usage).await.unwrap();
    assert!(
        deps.account_usage_repo
            .get_usage_report(account_id, AccountUsagePeriod::current())
            .await
            .unwrap()
            .as_of
            .is_none()
    );

    record_compute_usage_at_revision(deps, account_id, 0, 10).await;

    deps.account_usage_repo
        .set_monthly_usage_mode(
            account_id,
            MonthlyUsageMode::AllowOverage,
            account_id,
            MonthlyUsageModeTransitionSource::Owner,
        )
        .await
        .unwrap();
    record_compute_usage_at_revision(deps, account_id, 0, 20).await;
    record_compute_usage_at_revision(deps, account_id, 1, 30).await;

    deps.account_usage_repo
        .set_monthly_usage_mode(
            account_id,
            MonthlyUsageMode::HardLimit,
            account_id,
            MonthlyUsageModeTransitionSource::Owner,
        )
        .await
        .unwrap();
    record_compute_usage_at_revision(deps, account_id, 1, 40).await;
    record_compute_usage_at_revision(deps, account_id, 2, 50).await;

    let mut delayed_update = HashMap::new();
    delayed_update.insert(
        AccountId(account_id),
        ResourceUsageUpdate {
            monthly_usage_mode_revision: 1,
            memory_byte_nanoseconds_remainder: 0,
            durable_storage_byte_nanoseconds_remainder: 0,
            ephemeral_storage_byte_nanoseconds_remainder: 0,
            fuel_delta: 60,
            http_call_count_delta: 0,
            rpc_call_count_delta: 0,
            durable_storage_byte_seconds_delta: 0,
            ephemeral_storage_byte_seconds_delta: 0,
            memory_gb_seconds_delta: 0,
            metering: ResourceUsageMetering::all_enabled(),
        },
    );
    let response = deps
        .account_usage_service()
        .update_resource_usage(delayed_update, &AuthCtx::System)
        .await
        .unwrap();
    let limits = response.0.get(&AccountId(account_id)).unwrap();
    assert_eq!(limits.monthly_usage_mode_revision, 2);

    let mut remainder_update = HashMap::new();
    remainder_update.insert(
        AccountId(account_id),
        ResourceUsageUpdate {
            monthly_usage_mode_revision: 1,
            memory_byte_nanoseconds_remainder: 11,
            durable_storage_byte_nanoseconds_remainder: 22,
            ephemeral_storage_byte_nanoseconds_remainder: 33,
            fuel_delta: 0,
            http_call_count_delta: 0,
            rpc_call_count_delta: 0,
            durable_storage_byte_seconds_delta: 0,
            ephemeral_storage_byte_seconds_delta: 0,
            memory_gb_seconds_delta: 0,
            metering: ResourceUsageMetering::all_enabled(),
        },
    );
    let response = deps
        .account_usage_service()
        .update_resource_usage(remainder_update, &AuthCtx::System)
        .await
        .unwrap();
    let limits = response.0.get(&AccountId(account_id)).unwrap();
    assert_eq!(limits.monthly_usage_mode_revision, 2);

    assert_eq!(
        compute_usage_by_revision(deps, account_id).await,
        vec![(0, 30), (1, 130), (2, 50)]
    );
    assert_eq!(
        fractional_usage_by_revision(deps, account_id).await,
        vec![(0, 0, 0, 0), (1, 11, 22, 33), (2, 0, 0, 0)]
    );

    let mut future_usage = deps
        .account_usage_repo
        .get(account_id, &SqlDateTime::now())
        .await
        .unwrap()
        .unwrap();
    future_usage.monthly_usage_attribution = Some(MonthlyUsageAttribution {
        revision: 3,
        memory_byte_nanoseconds_remainder: 0,
        durable_storage_byte_nanoseconds_remainder: 0,
        ephemeral_storage_byte_nanoseconds_remainder: 0,
    });
    future_usage.add_change(UsageType::MonthlyGasLimit, 60);
    assert!(deps.account_usage_repo.add(&future_usage).await.is_err());
}

pub async fn test_monthly_usage_mode_baseline_serializes_with_usage_updates(deps: &Deps) {
    let TestDb::Postgres(pool) = &deps.test_db else {
        panic!("this race depends on PostgreSQL row-lock semantics");
    };
    let pool = pool.clone();
    let mut plan = deps
        .plan_repo
        .get_by_id(deps.test_plan_id())
        .await
        .unwrap()
        .unwrap();
    plan.overage_eligible = true;
    deps.plan_repo.create_or_update(plan).await.unwrap();

    let account = deps.create_account().await;
    let account_id = account.revision.account_id;
    let mut usage = deps
        .account_usage_repo
        .get(account_id, &SqlDateTime::now())
        .await
        .unwrap()
        .unwrap();
    usage.add_change(UsageType::MonthlyGasLimit, 11);

    let mut blocker = pool
        .with_rw("test", "block_monthly_usage_mode_transition")
        .begin()
        .await
        .unwrap();
    blocker
        .execute(
            sqlx::query("SELECT account_id FROM accounts WHERE account_id = $1 FOR UPDATE")
                .bind(account_id),
        )
        .await
        .unwrap();

    let transition_task = tokio::spawn({
        let account_usage_repo = deps.account_usage_repo.clone();
        async move {
            account_usage_repo
                .set_monthly_usage_mode(
                    account_id,
                    MonthlyUsageMode::AllowOverage,
                    account_id,
                    MonthlyUsageModeTransitionSource::Owner,
                )
                .await
        }
    });
    wait_for_postgres_lock(&pool, "SELECT account_id").await;

    let usage_task = tokio::spawn({
        let account_usage_repo = deps.account_usage_repo.clone();
        async move { account_usage_repo.add(&usage).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(!usage_task.is_finished());
    blocker.commit().await.unwrap();

    let transition = tokio::time::timeout(std::time::Duration::from_secs(5), transition_task)
        .await
        .expect("monthly usage mode transition remained blocked")
        .unwrap()
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), usage_task)
        .await
        .expect("usage update remained blocked")
        .unwrap()
        .unwrap();
    assert_eq!(transition.usage_baseline.compute_fuel, 0);
    assert_eq!(
        deps.account_usage_repo
            .get_usage_report(account_id, AccountUsagePeriod::current())
            .await
            .unwrap()
            .compute_fuel,
        11
    );
}

pub async fn test_resource_usage_response_uses_revision_observed_under_account_lock(deps: &Deps) {
    let TestDb::Postgres(pool) = &deps.test_db else {
        panic!("this race depends on PostgreSQL row-lock semantics");
    };
    let pool = pool.clone();
    let mut plan = deps
        .plan_repo
        .get_by_id(deps.test_plan_id())
        .await
        .unwrap()
        .unwrap();
    plan.overage_eligible = true;
    deps.plan_repo.create_or_update(plan).await.unwrap();

    let account = deps.create_account().await;
    let account_id = account.revision.account_id;
    let mut blocker = pool
        .with_rw("test", "block_resource_usage_and_mode_transition")
        .begin()
        .await
        .unwrap();
    blocker
        .execute(
            sqlx::query("SELECT account_id FROM accounts WHERE account_id = $1 FOR UPDATE")
                .bind(account_id),
        )
        .await
        .unwrap();

    let transition_task = tokio::spawn({
        let account_usage_repo = deps.account_usage_repo.clone();
        async move {
            account_usage_repo
                .set_monthly_usage_mode(
                    account_id,
                    MonthlyUsageMode::AllowOverage,
                    account_id,
                    MonthlyUsageModeTransitionSource::Owner,
                )
                .await
        }
    });
    wait_for_postgres_locks(&pool, "SELECT account_id", 1).await;

    let update_task = tokio::spawn({
        let account_usage_service = deps.account_usage_service();
        async move {
            account_usage_service
                .update_resource_usage(
                    HashMap::from([(
                        AccountId(account_id),
                        ResourceUsageUpdate {
                            monthly_usage_mode_revision: 0,
                            memory_byte_nanoseconds_remainder: 0,
                            durable_storage_byte_nanoseconds_remainder: 0,
                            ephemeral_storage_byte_nanoseconds_remainder: 0,
                            fuel_delta: 11,
                            http_call_count_delta: 0,
                            rpc_call_count_delta: 0,
                            durable_storage_byte_seconds_delta: 0,
                            ephemeral_storage_byte_seconds_delta: 0,
                            memory_gb_seconds_delta: 0,
                            metering: ResourceUsageMetering::all_enabled(),
                        },
                    )]),
                    &AuthCtx::System,
                )
                .await
        }
    });
    wait_for_postgres_locks(&pool, "SELECT account_id", 2).await;
    blocker.commit().await.unwrap();

    let transition = tokio::time::timeout(std::time::Duration::from_secs(5), transition_task)
        .await
        .expect("monthly usage mode transition remained blocked")
        .unwrap()
        .unwrap();
    let response = tokio::time::timeout(std::time::Duration::from_secs(5), update_task)
        .await
        .expect("resource usage update remained blocked")
        .unwrap()
        .unwrap();

    assert_eq!(transition.revision, 1);
    assert_eq!(
        response
            .0
            .get(&AccountId(account_id))
            .unwrap()
            .monthly_usage_mode_revision,
        1
    );
    assert_eq!(
        compute_usage_by_revision(deps, account_id).await,
        vec![(0, 11)]
    );
}

fn compare_created_to_requested_account(
    requested: &AccountRevisionRecord,
    created: &AccountExtRevisionRecord,
) {
    assert!(created.revision.account_id == requested.account_id);
    assert!(created.revision.name == requested.name);
    assert!(created.revision.email == requested.email);
    assert!(created.revision.roles == requested.roles)
}

fn test_account_root_card(account_id: Uuid) -> CardRecord {
    CardRecord::creation(
        CardId(new_repo_uuid()),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
        true,
        Some(CardManagedBy::AccountRoot(CardManagedByAccountRoot {
            account_id: golem_common::model::account::AccountId(account_id),
        })),
    )
}

// resolve_agent_type_by_names tests ---------------------------------------------------------------

fn make_test_agent_type(name: &str) -> AgentTypeSchema {
    AgentTypeSchema {
        type_name: AgentTypeName(name.to_string()),
        description: format!("Test agent {name}"),
        source_language: String::new(),
        schema: SchemaGraph::empty(),
        constructor: AgentConstructorSchema {
            name: None,
            description: "constructor".to_string(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![]),
        },
        methods: vec![],
        dependencies: vec![],
        mode: AgentMode::Durable,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: vec![],
    }
}

fn make_test_tool(name: &str, version: &str) -> Tool {
    Tool {
        version: version.to_string(),
        commands: CommandTree {
            nodes: vec![CommandNode {
                name: name.to_string(),
                aliases: Vec::new(),
                doc: Doc::default(),
                globals: Globals::default(),
                subcommands: Vec::new(),
                body: None,
            }],
        },
        schema: SchemaGraph::empty(),
    }
}

pub async fn test_deployment_tool_snapshot_and_rollback(deps: &Deps) {
    let owner = deps.create_account().await;
    let owner_account_id = owner.revision.account_id;
    let owner_account_email = owner.revision.email.clone();
    let app = deps.create_application(owner_account_id).await;
    let env = deps.create_env(app.revision.application_id).await;
    let environment_id = env.revision.environment_id;
    let component_name = format!("tool-component-{}", new_repo_uuid());
    let initial_component_revision = ComponentRevisionRecord {
        component_id: new_repo_uuid(),
        revision_id: 0,
        hash: SqlBlake3Hash::empty(),
        audit: DeletableRevisionAuditFields::new(owner_account_id),
        size: 0.into(),
        metadata: Blob::new(ComponentMetadata::from_parts(
            KnownExports::default(),
            Vec::new(),
            None,
            None,
            Vec::new(),
            BTreeMap::new(),
        )),
        object_store_key: String::new(),
        binary_hash: SqlBlake3Hash::empty(),
    };
    let component = deps
        .component_repo
        .create(
            environment_id,
            &component_name,
            initial_component_revision.clone(),
            Vec::new(),
        )
        .await
        .unwrap();
    let component_id = component.revision.component_id;
    let component_revision_id = component.revision.revision_id;
    let agent_type_name = format!("Agent{}", new_repo_uuid().simple());

    let deployment_creation = |deployment_revision_id: i64,
                               component_revision_id: i64,
                               version: &str,
                               tools: Vec<Tool>| {
        let deployment_revision = DeploymentRevision::try_from(deployment_revision_id).unwrap();
        let source = ToolSource::Component {
            component_id: ComponentId(component_id),
            component_revision: ComponentRevision::try_from(component_revision_id).unwrap(),
            component_name: golem_common::model::component::ComponentName(component_name.clone()),
        };
        let registered_tools = tools
            .into_iter()
            .map(|definition| {
                DeploymentRegisteredToolRecord::from_model(
                    EnvironmentId(environment_id),
                    RegisteredTool {
                        deployment_revision,
                        definition,
                        provision: ToolProvisionConfig::default(),
                        source: source.clone(),
                        owner_account_id: AccountId(owner_account_id),
                        owner_account_email: golem_common::model::account::AccountEmail::new(
                            owner_account_email.clone(),
                        ),
                        metadata_version: TOOL_METADATA_WIT_VERSION.to_string(),
                    },
                )
            })
            .collect::<Vec<_>>();
        let alpha_name = ToolName::try_from("alpha").unwrap();
        let agent_tool_bindings = registered_tools
            .iter()
            .find(|tool| tool.tool_name == alpha_name.as_str())
            .map(|tool| {
                DeploymentAgentToolBindingRecord::from_model(
                    EnvironmentId(environment_id),
                    CompiledToolBinding {
                        deployment_revision,
                        agent_type_name: AgentTypeName(agent_type_name.clone()),
                        tool_name: alpha_name,
                        version: tool.tool_definition.value().version.clone(),
                        metadata_version: tool.metadata_version.clone(),
                        account_id: AccountId(owner_account_id),
                        account_email: golem_common::model::account::AccountEmail::new(
                            owner_account_email.clone(),
                        ),
                        parameters: NormalizedJsonValue::new(serde_json::json!({
                            "revision": deployment_revision_id
                        })),
                        secret_keys_readable: SecretKeyScope::All,
                        secret_keys_revealable: SecretKeyScope::All,
                        filesystem_access: golem_common::model::tool::ToolFilesystemAccess::Unset,
                        source,
                    },
                )
            })
            .into_iter()
            .collect();

        DeploymentRevisionCreationRecord {
            environment_id,
            deployment_revision_id,
            version: version.to_string(),
            hash: SqlBlake3Hash::empty(),
            components: vec![DeploymentComponentRevisionRecord {
                environment_id,
                deployment_revision_id,
                component_id,
                component_revision_id,
            }],
            http_api_deployments: Vec::new(),
            mcp_deployments: Vec::new(),
            compiled_routes: Vec::new(),
            compiled_mcp: Vec::new(),
            registered_agent_types: vec![DeploymentRegisteredAgentTypeRecord {
                environment_id,
                deployment_revision_id,
                agent_type_name: agent_type_name.clone(),
                component_id,
                component_revision_id,
                component_name: component_name.clone(),
                owner_account_id,
                owner_account_email: owner_account_email.clone(),
                webhook_prefix_authority_and_path: None,
                agent_type: Blob::new(make_test_agent_type(&agent_type_name)),
                canonical_agent_type_name: agent_type_name.to_kebab_case(),
            }],
            registered_tools,
            agent_tool_bindings,
            created_agent_secrets: Vec::new(),
            updated_agent_secrets: Vec::new(),
            replaced_agent_secrets: Vec::new(),
            created_resource_definitions: Vec::new(),
            created_retry_policies: Vec::new(),
            user_account_id: owner_account_id,
        }
    };

    deps.full_deployment_repo
        .deploy(
            deployment_creation(
                1,
                component_revision_id,
                "1.0.0",
                vec![
                    make_test_tool("zeta", "1.0.0"),
                    make_test_tool("alpha", "1.0.0"),
                ],
            ),
            false,
        )
        .await
        .unwrap()
        .signal_new_events_available(&deps.test_registry_change_notifier());
    deps.full_deployment_repo
        .deploy(
            deployment_creation(
                2,
                component_revision_id,
                "2.0.0",
                vec![make_test_tool("alpha", "2.0.0")],
            ),
            false,
        )
        .await
        .unwrap()
        .signal_new_events_available(&deps.test_registry_change_notifier());

    let first_revision = deps
        .full_deployment_repo
        .list_deployment_registered_tools(environment_id, 1)
        .await
        .unwrap();
    assert_eq!(
        first_revision
            .iter()
            .map(|tool| tool.tool_name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha", "zeta"]
    );
    let exact_alpha: RegisteredTool = deps
        .full_deployment_repo
        .get_deployment_registered_tool(environment_id, 1, "alpha")
        .await
        .unwrap()
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(exact_alpha.deployment_revision.get(), 1);
    assert_eq!(exact_alpha.definition.version, "1.0.0");
    assert_eq!(exact_alpha.metadata_version, TOOL_METADATA_WIT_VERSION);

    let current: golem_common::model::tool::ToolDeploymentState = deps
        .full_deployment_repo
        .get_current_tool_deployment_state(environment_id)
        .await
        .unwrap()
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(current.deployment_revision.get(), 2);
    assert_eq!(
        current.registered_tools[&ToolName::try_from("alpha").unwrap()]
            .definition
            .version,
        "2.0.0"
    );
    assert_eq!(
        current.agent_tool_bindings[&AgentTypeName(agent_type_name.clone())]
            [&ToolName::try_from("alpha").unwrap()]
            .metadata_version,
        TOOL_METADATA_WIT_VERSION
    );
    assert_eq!(
        current.agent_tool_bindings[&AgentTypeName(agent_type_name.clone())]
            [&ToolName::try_from("alpha").unwrap()]
            .parameters
            .0,
        serde_json::json!({ "revision": 2 })
    );

    let updated_component_revision_id = component_revision_id + 1;
    deps.component_repo
        .update(
            ComponentRevisionRecord {
                revision_id: updated_component_revision_id,
                ..initial_component_revision
            }
            .with_updated_hash()
            .unwrap(),
            Vec::new(),
        )
        .await
        .unwrap();
    deps.full_deployment_repo
        .deploy(
            deployment_creation(
                3,
                updated_component_revision_id,
                "3.0.0",
                vec![make_test_tool("alpha", "3.0.0")],
            ),
            false,
        )
        .await
        .unwrap()
        .signal_new_events_available(&deps.test_registry_change_notifier());

    let latest_for_component: golem_common::model::tool::ToolDeploymentState = deps
        .full_deployment_repo
        .get_latest_tool_deployment_state_by_component_revision(
            &environment_id,
            &component_id,
            component_revision_id,
        )
        .await
        .unwrap()
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(latest_for_component.deployment_revision.get(), 2);
    let latest_for_updated_component: golem_common::model::tool::ToolDeploymentState = deps
        .full_deployment_repo
        .get_latest_tool_deployment_state_by_component_revision(
            &environment_id,
            &component_id,
            updated_component_revision_id,
        )
        .await
        .unwrap()
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(latest_for_updated_component.deployment_revision.get(), 3);

    deps.full_deployment_repo
        .set_current_deployment(owner_account_id, environment_id, 1)
        .await
        .unwrap()
        .signal_new_events_available(&deps.test_registry_change_notifier());
    let rolled_back: golem_common::model::tool::ToolDeploymentState = deps
        .full_deployment_repo
        .get_current_tool_deployment_state(environment_id)
        .await
        .unwrap()
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(rolled_back.deployment_revision.get(), 1);
    assert_eq!(
        rolled_back.registered_tools[&ToolName::try_from("alpha").unwrap()]
            .definition
            .version,
        "1.0.0"
    );
    assert_eq!(rolled_back.registered_tools.len(), 2);
}

struct ResolveTestEnv {
    owner_account_id: Uuid,
    app_name: String,
    env_name: String,
    environment_id: Uuid,
    deployment_revision_id: i64,
    agent_type_name: String,
}

/// Sets up: owner account → application → environment → deployment with one agent type.
/// Returns the environment context for use in resolve_agent_type_by_names tests.
async fn setup_resolve_env(deps: &Deps) -> ResolveTestEnv {
    let email = format!("resolve-test-{}@golem.test", new_repo_uuid());
    let owner = deps.create_account_with_email(&email).await;
    let owner_account_id = owner.revision.account_id;
    let app_name = format!("resolve-app-{}", new_repo_uuid());
    let env_name = format!("resolve-env-{}", new_repo_uuid());

    let app = deps
        .application_repo
        .create(
            owner_account_id,
            ApplicationRevisionRecord {
                application_id: new_repo_uuid(),
                revision_id: 0,
                name: app_name.clone(),
                audit: DeletableRevisionAuditFields::new(owner_account_id),
            },
        )
        .await
        .unwrap();

    let env_revision = EnvironmentRevisionRecord {
        environment_id: new_repo_uuid(),
        revision_id: 0,
        name: env_name.clone(),
        audit: DeletableRevisionAuditFields::new(owner_account_id),
        compatibility_check: false,
        version_check: false,
        security_overrides: false,
        hash: SqlBlake3Hash::empty(),
    };
    let env = deps
        .environment_repo
        .create(
            app.revision.application_id,
            env_revision.clone(),
            test_environment_default_card_record(env_revision.environment_id),
        )
        .await
        .unwrap();

    let environment_id = env.revision.environment_id;

    // Create a component (required by FK on deployment_registered_agent_types)
    let component_name = format!("test-component-{}", new_repo_uuid());
    let component = deps
        .component_repo
        .create(
            environment_id,
            &component_name,
            ComponentRevisionRecord {
                component_id: new_repo_uuid(),
                revision_id: 0,
                hash: SqlBlake3Hash::empty(),
                audit: DeletableRevisionAuditFields::new(owner_account_id),
                size: 0.into(),
                metadata: Blob::new(ComponentMetadata::from_parts(
                    KnownExports::default(),
                    vec![],
                    None,
                    None,
                    vec![],
                    BTreeMap::new(),
                )),
                object_store_key: "".to_string(),
                binary_hash: SqlBlake3Hash::empty(),
            },
            Vec::new(),
        )
        .await
        .unwrap();

    let component_id = component.revision.component_id;
    let component_revision_id = component.revision.revision_id;

    let agent_type_name = format!("TestAgent{}", new_repo_uuid().simple());
    let agent_type = make_test_agent_type(&agent_type_name);
    let deployment_revision_id: i64 = 1;

    let agent_type_record = DeploymentRegisteredAgentTypeRecord {
        environment_id,
        deployment_revision_id,
        agent_type_name: agent_type_name.clone(),
        component_id,
        component_revision_id,
        component_name,
        owner_account_id,
        owner_account_email: email.clone(),
        webhook_prefix_authority_and_path: None,
        agent_type: Blob::new(agent_type),
        canonical_agent_type_name: agent_type_name.to_kebab_case(),
    };

    let deployment_creation = DeploymentRevisionCreationRecord {
        environment_id,
        deployment_revision_id,
        version: "1.0.0".to_string(),
        hash: SqlBlake3Hash::empty(),
        components: vec![],
        http_api_deployments: vec![],
        mcp_deployments: vec![],
        compiled_routes: vec![],
        compiled_mcp: vec![],
        registered_agent_types: vec![agent_type_record],
        registered_tools: vec![],
        agent_tool_bindings: vec![],
        created_agent_secrets: vec![],
        updated_agent_secrets: vec![],
        replaced_agent_secrets: vec![],
        created_resource_definitions: vec![],
        created_retry_policies: vec![],
        user_account_id: owner_account_id,
    };

    deps.full_deployment_repo
        .deploy(deployment_creation, false)
        .await
        .unwrap()
        .signal_new_events_available(&deps.test_registry_change_notifier());

    ResolveTestEnv {
        owner_account_id,
        app_name,
        env_name,
        environment_id,
        deployment_revision_id,
        agent_type_name,
    }
}

/// Caller owns env → works (no email)
pub async fn test_resolve_agent_type_owner_no_email(deps: &Deps) {
    let env = setup_resolve_env(deps).await;

    let result = deps
        .full_deployment_repo
        .resolve_agent_type_by_names(
            env.owner_account_id,
            &env.app_name,
            &env.env_name,
            &env.agent_type_name,
            None, // latest deployment
            None, // no owner email → use caller's own account
        )
        .await
        .unwrap();

    let_assert!(Some(record) = result);
    check!(record.agent_type_name == env.agent_type_name);
    check!(record.environment_id == env.environment_id);
    check!(record.deployment_revision_id == env.deployment_revision_id);
    check!(record.owner_account_id == env.owner_account_id);
}

/// Env exists but no current deployment (latest) → None
pub async fn test_resolve_agent_type_no_deployment_returns_none(deps: &Deps) {
    let email = format!("no-deploy-{}@golem.test", new_repo_uuid());
    let owner = deps.create_account_with_email(&email).await;
    let owner_account_id = owner.revision.account_id;
    let app_name = format!("no-deploy-app-{}", new_repo_uuid());
    let env_name = format!("no-deploy-env-{}", new_repo_uuid());

    let app = deps
        .application_repo
        .create(
            owner_account_id,
            ApplicationRevisionRecord {
                application_id: new_repo_uuid(),
                revision_id: 0,
                name: app_name.clone(),
                audit: DeletableRevisionAuditFields::new(owner_account_id),
            },
        )
        .await
        .unwrap();

    let env_revision = EnvironmentRevisionRecord {
        environment_id: new_repo_uuid(),
        revision_id: 0,
        name: env_name.clone(),
        audit: DeletableRevisionAuditFields::new(owner_account_id),
        compatibility_check: false,
        version_check: false,
        security_overrides: false,
        hash: SqlBlake3Hash::empty(),
    };
    deps.environment_repo
        .create(
            app.revision.application_id,
            env_revision.clone(),
            test_environment_default_card_record(env_revision.environment_id),
        )
        .await
        .unwrap();

    // No deployment created — resolve latest should return None
    let result = deps
        .full_deployment_repo
        .resolve_agent_type_by_names(
            owner_account_id,
            &app_name,
            &env_name,
            "SomeAgent",
            None, // latest
            None,
        )
        .await
        .unwrap();

    assert!(result.is_none());
}

/// Specific deployment revision not present → None
pub async fn test_resolve_agent_type_nonexistent_revision_returns_none(deps: &Deps) {
    let env = setup_resolve_env(deps).await;

    let result = deps
        .full_deployment_repo
        .resolve_agent_type_by_names(
            env.owner_account_id,
            &env.app_name,
            &env.env_name,
            &env.agent_type_name,
            Some(9999), // non-existent revision
            None,
        )
        .await
        .unwrap();

    assert!(result.is_none());
}

/// Email doesn't exist → None (no existence leak)
pub async fn test_resolve_agent_type_unknown_email_returns_none(deps: &Deps) {
    let env = setup_resolve_env(deps).await;

    let grantee = deps.create_account().await;

    let result = deps
        .full_deployment_repo
        .resolve_agent_type_by_names(
            grantee.revision.account_id,
            &env.app_name,
            &env.env_name,
            &env.agent_type_name,
            None,
            Some("nonexistent-user@nowhere.example"),
        )
        .await
        .unwrap();

    assert!(result.is_none());
}

pub async fn test_mcp_deployment_create_and_update(deps: &Deps) {
    let user = deps.create_account().await;
    let app = deps.create_application(user.revision.account_id).await;
    let env = deps.create_env(app.revision.application_id).await;

    let deployment_id = new_repo_uuid();
    let domain = "test-mcp.com";
    let revision_0 = McpDeploymentRevisionRecord {
        mcp_deployment_id: deployment_id,
        revision_id: 0,
        hash: SqlBlake3Hash::empty(),
        data: Blob::new(McpDeploymentData {
            agents: Default::default(),
        }),
        audit: DeletableRevisionAuditFields::new(user.revision.account_id),
    };

    let _created_deployment = deps
        .mcp_deployment_repo
        .create(env.revision.environment_id, domain, revision_0.clone())
        .await
        .unwrap();

    let fetched_deployment = deps
        .mcp_deployment_repo
        .get_staged_by_id(deployment_id)
        .await
        .unwrap();
    let_assert!(Some(fetched_deployment) = fetched_deployment);
    assert!(fetched_deployment.deployment.revision.revision_id == revision_0.revision_id);
    assert!(fetched_deployment.deployment.domain == domain);

    let fetched_by_domain = deps
        .mcp_deployment_repo
        .get_staged_by_domain(env.revision.environment_id, domain)
        .await
        .unwrap();
    let_assert!(Some(fetched_by_domain) = fetched_by_domain);
    assert!(fetched_by_domain.revision.revision_id == revision_0.revision_id);
    assert!(fetched_by_domain.domain == domain);

    // Update the deployment (domain stays the same, only data changes)
    let revision_1 = McpDeploymentRevisionRecord {
        mcp_deployment_id: deployment_id,
        revision_id: 1,
        hash: SqlBlake3Hash::empty(),
        data: Blob::new(McpDeploymentData {
            agents: Default::default(),
        }),
        audit: DeletableRevisionAuditFields::new(user.revision.account_id),
    };

    let updated_deployment = deps
        .mcp_deployment_repo
        .update(revision_1.clone())
        .await
        .unwrap();

    assert!(updated_deployment.revision.revision_id == revision_1.revision_id);
    assert!(updated_deployment.domain == domain);

    // Domain should still be found
    let domain_query = deps
        .mcp_deployment_repo
        .get_staged_by_domain(env.revision.environment_id, domain)
        .await
        .unwrap();
    let_assert!(Some(domain_query) = domain_query);
    assert!(domain_query.revision.revision_id == revision_1.revision_id);
    assert!(domain_query.domain == domain);
}

pub async fn test_mcp_deployment_list_and_delete(deps: &Deps) {
    let user = deps.create_account().await;
    let app = deps.create_application(user.revision.account_id).await;
    let env = deps.create_env(app.revision.application_id).await;

    let deployment_id = new_repo_uuid();
    let domain = "test-mcp-1.com";
    let revision_0 = McpDeploymentRevisionRecord {
        mcp_deployment_id: deployment_id,
        revision_id: 0,
        hash: SqlBlake3Hash::empty(),
        data: Blob::new(McpDeploymentData {
            agents: Default::default(),
        }),
        audit: DeletableRevisionAuditFields::new(user.revision.account_id),
    };

    let _created_deployment = deps
        .mcp_deployment_repo
        .create(env.revision.environment_id, domain, revision_0.clone())
        .await
        .unwrap();

    let deployments = deps
        .mcp_deployment_repo
        .list_staged(env.revision.environment_id)
        .await
        .unwrap();

    assert!(deployments.len() == 1);

    // Update the deployment
    let revision_1 = McpDeploymentRevisionRecord {
        mcp_deployment_id: deployment_id,
        revision_id: 1,
        hash: SqlBlake3Hash::empty(),
        data: Blob::new(McpDeploymentData {
            agents: Default::default(),
        }),
        audit: DeletableRevisionAuditFields::new(user.revision.account_id),
    };

    let _updated_deployment = deps
        .mcp_deployment_repo
        .update(revision_1.clone())
        .await
        .unwrap();

    let deployments = deps
        .mcp_deployment_repo
        .list_staged(env.revision.environment_id)
        .await
        .unwrap();

    assert!(deployments.len() == 1);

    // Create another deployment
    let other_deployment_id = new_repo_uuid();
    let other_domain = "test-mcp-2.com";
    let other_revision_0 = McpDeploymentRevisionRecord {
        mcp_deployment_id: other_deployment_id,
        revision_id: 0,
        hash: SqlBlake3Hash::empty(),
        data: Blob::new(McpDeploymentData {
            agents: Default::default(),
        }),
        audit: DeletableRevisionAuditFields::new(user.revision.account_id),
    };

    let _created_other_deployment = deps
        .mcp_deployment_repo
        .create(
            env.revision.environment_id,
            other_domain,
            other_revision_0.clone(),
        )
        .await
        .unwrap();

    let deployments = deps
        .mcp_deployment_repo
        .list_staged(env.revision.environment_id)
        .await
        .unwrap();

    assert!(deployments.len() == 2);

    let delete_with_old_revision = deps
        .mcp_deployment_repo
        .delete(user.revision.account_id, deployment_id, 1)
        .await;

    let_assert!(Err(McpDeploymentRepoError::ConcurrentModification) = delete_with_old_revision);

    deps.mcp_deployment_repo
        .delete(user.revision.account_id, deployment_id, 2)
        .await
        .unwrap();

    let deployments = deps
        .mcp_deployment_repo
        .list_staged(env.revision.environment_id)
        .await
        .unwrap();

    assert!(deployments.len() == 1);
}

pub async fn test_registry_change_record_and_query(deps: &Deps) {
    let env_id1 = new_repo_uuid();
    let env_id2 = new_repo_uuid();

    // Capture baseline cursor (other tests may have inserted events)
    let baseline = deps
        .registry_change_repo
        .get_latest_event_id()
        .await
        .unwrap()
        .unwrap_or(ChangeEventId(0));

    // Record first event
    let id1 = deps
        .record_registry_change_event(NewRegistryChangeEvent::deployment_changed(env_id1, 1, 1))
        .await;
    assert!(id1 > baseline);

    // Record second event
    let id2 = deps
        .record_registry_change_event(NewRegistryChangeEvent::deployment_changed(env_id2, 2, 2))
        .await;
    assert!(id2 > id1);

    // Record third event for same environment
    let id3 = deps
        .record_registry_change_event(NewRegistryChangeEvent::deployment_changed(env_id1, 3, 3))
        .await;
    assert!(id3 > id2);

    // get_latest_event_id returns latest
    let latest = deps
        .registry_change_repo
        .get_latest_event_id()
        .await
        .unwrap();
    assert!(latest == Some(id3));

    // get_events_since with baseline cursor returns our 3 events
    let events = deps
        .registry_change_repo
        .get_events_since(baseline)
        .await
        .unwrap();
    assert!(events.len() >= 3);
    // Find our events by id
    let our_events: Vec<_> = events.iter().filter(|e| e.event_id() >= id1).collect();
    assert!(our_events.len() == 3);
    assert!(our_events[0].event_id() == id1);
    assert!(matches!(
        &our_events[0],
        RegistryChangeEvent::DeploymentChanged { environment_id, deployment_revision_id: 1, .. }
        if *environment_id == env_id1
    ));
    assert!(our_events[1].event_id() == id2);
    assert!(matches!(
        &our_events[1],
        RegistryChangeEvent::DeploymentChanged { environment_id, deployment_revision_id: 2, .. }
        if *environment_id == env_id2
    ));
    assert!(our_events[2].event_id() == id3);

    // get_events_since with cursor at id2 returns only id3
    let events = deps
        .registry_change_repo
        .get_events_since(id2)
        .await
        .unwrap();
    assert!(events.len() == 1);
    assert!(events[0].event_id() == id3);

    // get_events_since with cursor at latest returns empty
    let events = deps
        .registry_change_repo
        .get_events_since(id3)
        .await
        .unwrap();
    assert!(events.is_empty());
}

pub async fn test_registry_change_replay_and_broadcast(deps: &Deps) {
    let env_id = new_repo_uuid();
    let notifier = SqliteRegistryChangeNotifier::new(64, deps.registry_change_repo_for_notifier());
    let mut join_set = tokio::task::JoinSet::new();
    notifier.start_background_tasks(&mut join_set);

    // Insert some events in the DB (simulating past deployments)
    let id1 = deps
        .record_registry_change_event(NewRegistryChangeEvent::deployment_changed(env_id, 10, 10))
        .await;
    let id2 = deps
        .record_registry_change_event(NewRegistryChangeEvent::deployment_changed(env_id, 20, 20))
        .await;

    // Simulate replay from cursor: get_events_since(id1 - 1) should include id1 and id2
    let replayed = deps
        .registry_change_repo
        .get_events_since(ChangeEventId(id1.0 - 1))
        .await
        .unwrap();
    let our_events: Vec<_> = replayed
        .iter()
        .filter(|e| matches!(e, RegistryChangeEvent::DeploymentChanged { environment_id, .. } if *environment_id == env_id))
        .collect();
    assert!(our_events.len() == 2);
    assert!(matches!(
        &our_events[0],
        RegistryChangeEvent::DeploymentChanged {
            deployment_revision_id: 10,
            ..
        }
    ));
    assert!(matches!(
        &our_events[1],
        RegistryChangeEvent::DeploymentChanged {
            deployment_revision_id: 20,
            ..
        }
    ));

    // Subscribe to broadcast for live events
    let mut rx = notifier.subscribe();

    // Simulate a new deployment: record in DB and notify
    let id3 = deps
        .record_registry_change_event(NewRegistryChangeEvent::deployment_changed(env_id, 30, 30))
        .await;
    notifier.signal_new_events_available();

    // Verify we eventually receive the new live event.
    // Depending on local notifier cursor state, older events can still be emitted first.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    let received = loop {
        let now = tokio::time::Instant::now();
        assert!(now < deadline, "timed out waiting for event id {}", id3.0);

        let next = tokio::time::timeout(deadline - now, rx.recv())
            .await
            .expect("timeout while waiting for broadcast event")
            .expect("broadcast channel closed");

        if next.event_id() == id3 {
            break next;
        }
    };
    assert!(matches!(
        received,
        RegistryChangeEvent::DeploymentChanged { environment_id, deployment_revision_id: 30, .. }
        if environment_id == env_id
    ));

    // Verify cursor-based replay from id2 only returns id3
    let replayed_from_id2 = deps
        .registry_change_repo
        .get_events_since(id2)
        .await
        .unwrap();
    let our_events: Vec<_> = replayed_from_id2
        .iter()
        .filter(|e| matches!(e, RegistryChangeEvent::DeploymentChanged { environment_id, .. } if *environment_id == env_id))
        .collect();
    assert!(our_events.len() == 1);
    assert!(our_events[0].event_id() == id3);
    join_set.abort_all();
}

pub async fn test_registry_change_cursor_expired_detection(deps: &Deps) {
    let env_id = new_repo_uuid();

    // Insert events
    let id1 = deps
        .record_registry_change_event(NewRegistryChangeEvent::deployment_changed(env_id, 10, 10))
        .await;
    let id2 = deps
        .record_registry_change_event(NewRegistryChangeEvent::deployment_changed(env_id, 20, 20))
        .await;

    // Normal case: events exist after cursor, so no cursor_expired
    let events = deps
        .registry_change_repo
        .get_events_since(ChangeEventId(id1.0 - 1))
        .await
        .unwrap();
    let our_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, RegistryChangeEvent::DeploymentChanged { environment_id, .. } if *environment_id == env_id))
        .collect();
    assert!(!our_events.is_empty());

    // After latest cursor: get_events_since(id2) returns empty for our env
    let events_after_latest = deps
        .registry_change_repo
        .get_events_since(id2)
        .await
        .unwrap();
    let our_events: Vec<_> = events_after_latest
        .iter()
        .filter(|e| matches!(e, RegistryChangeEvent::DeploymentChanged { environment_id, .. } if *environment_id == env_id))
        .collect();
    assert!(our_events.is_empty());

    let latest = deps
        .registry_change_repo
        .get_latest_event_id()
        .await
        .unwrap();
    assert!(latest.is_some());
    let latest_id = latest.unwrap();
    // The cursor_expired check in gRPC handler is: latest_id > last_seen_i64 + 1
    // With last_seen = id2 and latest >= id2, client is up to date -> not expired
    assert!(latest_id >= id2);
    // latest_id should NOT be > id2 + 1 unless other tests inserted events,
    // but the key invariant is: when events exist for replay, cursor is NOT expired.

    // Verify the cursor_expired detection logic with a hypothetical old cursor:
    // If events exist between old_cursor and latest, replay works -> not expired
    if id1.0 > 2 {
        let old_cursor = ChangeEventId(id1.0 - 2);
        let events = deps
            .registry_change_repo
            .get_events_since(old_cursor)
            .await
            .unwrap();
        assert!(!events.is_empty());
    }
}

pub async fn test_registry_change_cleanup(deps: &Deps) {
    let env_id = new_repo_uuid();

    // Record an event
    let id = deps
        .record_registry_change_event(NewRegistryChangeEvent::deployment_changed(env_id, 1, 1))
        .await;

    // Cleanup with large retention should not delete the freshly created event
    let deleted = deps
        .registry_change_repo
        .cleanup_old_events(3600)
        .await
        .unwrap();
    assert!(deleted == 0);

    // Verify our specific event still exists by querying events since id-1
    let events = deps
        .registry_change_repo
        .get_events_since(ChangeEventId(id.0 - 1))
        .await
        .unwrap();
    assert!(
        events.iter().any(|e| e.event_id() == id),
        "our event should still exist after cleanup with large retention"
    );
}

pub async fn test_registry_change_mixed_event_types(deps: &Deps) {
    let env_id = new_repo_uuid();
    let account_id = new_repo_uuid();
    let grantee_id = new_repo_uuid();

    let baseline = deps
        .registry_change_repo
        .get_latest_event_id()
        .await
        .unwrap()
        .unwrap_or(ChangeEventId(0));

    // Record deployment changed event
    let id1 = deps
        .record_registry_change_event(NewRegistryChangeEvent::deployment_changed(env_id, 1, 1))
        .await;

    // Record account tokens invalidated event
    let _id2 = deps
        .record_registry_change_event(NewRegistryChangeEvent::account_tokens_invalidated(
            account_id,
        ))
        .await;

    // Record environment permissions changed event
    let _id3 = deps
        .record_registry_change_event(NewRegistryChangeEvent::environment_permissions_changed(
            env_id, grantee_id,
        ))
        .await;

    // Fetch all events since baseline
    let events = deps
        .registry_change_repo
        .get_events_since(baseline)
        .await
        .unwrap();
    let our_events: Vec<_> = events.iter().filter(|e| e.event_id() >= id1).collect();
    assert_eq!(our_events.len(), 3);

    // Verify deployment event
    assert!(matches!(
        &our_events[0],
        RegistryChangeEvent::DeploymentChanged { environment_id, deployment_revision_id: 1, .. }
        if *environment_id == env_id
    ));

    // Verify account tokens event
    assert!(matches!(
        &our_events[1],
        RegistryChangeEvent::AccountTokensInvalidated { account_id: aid, .. }
        if *aid == account_id
    ));

    // Verify permissions event
    assert!(matches!(
        &our_events[2],
        RegistryChangeEvent::EnvironmentPermissionsChanged { environment_id, grantee_account_id, .. }
        if *environment_id == env_id && *grantee_account_id == grantee_id
    ));
}

pub async fn test_update_http_call_counts(deps: &Deps) {
    use golem_common::model::account::AccountId;
    use golem_registry_service::services::account_usage::ResourceUsageUpdate;
    use golem_service_base::model::auth::AuthCtx;
    use std::collections::HashMap;

    let user = deps.create_account().await;
    let account_id = AccountId(user.revision.account_id);
    let svc = deps.account_usage_service();

    // Empty updates → no accounts in the response.
    let initial = svc
        .update_resource_usage(
            HashMap::<AccountId, ResourceUsageUpdate>::new(),
            &AuthCtx::System,
        )
        .await
        .unwrap();
    assert!(initial.0.is_empty());

    // Recording 10 HTTP calls reduces available by 10.
    let mut updates = HashMap::new();
    updates.insert(
        account_id,
        ResourceUsageUpdate {
            monthly_usage_mode_revision: 0,
            memory_byte_nanoseconds_remainder: 0,
            durable_storage_byte_nanoseconds_remainder: 0,
            ephemeral_storage_byte_nanoseconds_remainder: 0,
            fuel_delta: 0,
            http_call_count_delta: 10,
            rpc_call_count_delta: 0,
            durable_storage_byte_seconds_delta: 0,
            ephemeral_storage_byte_seconds_delta: 0,
            memory_gb_seconds_delta: 0,
            metering: golem_service_base::clients::registry::ResourceUsageMetering::all_enabled(),
        },
    );
    let result = svc
        .update_resource_usage(updates, &AuthCtx::System)
        .await
        .unwrap();
    let limits = result
        .0
        .get(&account_id)
        .expect("account must be in response");
    check!(
        limits.available_http_calls == 4990,
        "expected 4990, got {}",
        limits.available_http_calls
    );
    // RPC should be untouched.
    check!(
        limits.available_rpc_calls == 5000,
        "expected RPC untouched at 5000, got {}",
        limits.available_rpc_calls
    );

    // Exactly reaching the limit returns 0.
    let mut updates = HashMap::new();
    updates.insert(
        account_id,
        ResourceUsageUpdate {
            monthly_usage_mode_revision: 0,
            memory_byte_nanoseconds_remainder: 0,
            durable_storage_byte_nanoseconds_remainder: 0,
            ephemeral_storage_byte_nanoseconds_remainder: 0,
            fuel_delta: 0,
            http_call_count_delta: 4990,
            rpc_call_count_delta: 0,
            durable_storage_byte_seconds_delta: 0,
            ephemeral_storage_byte_seconds_delta: 0,
            memory_gb_seconds_delta: 0,
            metering: golem_service_base::clients::registry::ResourceUsageMetering::all_enabled(),
        },
    );
    let result = svc
        .update_resource_usage(updates, &AuthCtx::System)
        .await
        .unwrap();
    let limits = result.0.get(&account_id).unwrap();
    check!(
        limits.available_http_calls == 0,
        "expected 0 at limit, got {}",
        limits.available_http_calls
    );

    // Slightly exceeding the limit is allowed (optimistic) — saturates at 0, not an error.
    let mut updates = HashMap::new();
    updates.insert(
        account_id,
        ResourceUsageUpdate {
            monthly_usage_mode_revision: 0,
            memory_byte_nanoseconds_remainder: 0,
            durable_storage_byte_nanoseconds_remainder: 0,
            ephemeral_storage_byte_nanoseconds_remainder: 0,
            fuel_delta: 0,
            http_call_count_delta: 1,
            rpc_call_count_delta: 0,
            durable_storage_byte_seconds_delta: 0,
            ephemeral_storage_byte_seconds_delta: 0,
            memory_gb_seconds_delta: 0,
            metering: golem_service_base::clients::registry::ResourceUsageMetering::all_enabled(),
        },
    );
    let result = svc
        .update_resource_usage(updates, &AuthCtx::System)
        .await
        .unwrap();
    let limits = result.0.get(&account_id).unwrap();
    check!(
        limits.available_http_calls == 0,
        "available_http_calls should saturate at 0, got {}",
        limits.available_http_calls
    );

    let mut updates = HashMap::new();
    updates.insert(
        account_id,
        ResourceUsageUpdate {
            monthly_usage_mode_revision: 0,
            memory_byte_nanoseconds_remainder: 0,
            durable_storage_byte_nanoseconds_remainder: 0,
            ephemeral_storage_byte_nanoseconds_remainder: 0,
            fuel_delta: 0,
            http_call_count_delta: 0,
            rpc_call_count_delta: 0,
            durable_storage_byte_seconds_delta: 123,
            ephemeral_storage_byte_seconds_delta: 456,
            memory_gb_seconds_delta: 12,
            metering: golem_service_base::clients::registry::ResourceUsageMetering::all_enabled(),
        },
    );
    svc.update_resource_usage(updates, &AuthCtx::System)
        .await
        .unwrap();

    let mut updates = HashMap::new();
    updates.insert(
        account_id,
        ResourceUsageUpdate {
            monthly_usage_mode_revision: 0,
            memory_byte_nanoseconds_remainder: 0,
            durable_storage_byte_nanoseconds_remainder: 0,
            ephemeral_storage_byte_nanoseconds_remainder: 0,
            fuel_delta: 0,
            http_call_count_delta: 0,
            rpc_call_count_delta: 0,
            durable_storage_byte_seconds_delta: 10,
            ephemeral_storage_byte_seconds_delta: 20,
            memory_gb_seconds_delta: 3,
            metering: golem_service_base::clients::registry::ResourceUsageMetering::all_enabled(),
        },
    );
    svc.update_resource_usage(updates, &AuthCtx::System)
        .await
        .unwrap();

    let usage = deps
        .account_usage_repo
        .get(account_id.0, &golem_service_base::repo::SqlDateTime::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        usage.usage(
            golem_registry_service::repo::model::account_usage::UsageType::MonthlyDurableAgentStorageByteSeconds
        ),
        133
    );
    assert_eq!(
        usage.usage(
            golem_registry_service::repo::model::account_usage::UsageType::MonthlyEphemeralStorageByteSeconds
        ),
        476
    );
    assert_eq!(usage.usage(UsageType::MonthlyMemoryGbSeconds), 15);
}

pub async fn test_update_rpc_call_counts(deps: &Deps) {
    use golem_common::model::account::AccountId;
    use golem_registry_service::services::account_usage::ResourceUsageUpdate;
    use golem_service_base::model::auth::AuthCtx;
    use std::collections::HashMap;

    let user = deps.create_account().await;
    let account_id = AccountId(user.revision.account_id);
    let svc = deps.account_usage_service();

    // Empty updates → no accounts in the response.
    let initial = svc
        .update_resource_usage(
            HashMap::<AccountId, ResourceUsageUpdate>::new(),
            &AuthCtx::System,
        )
        .await
        .unwrap();
    assert!(initial.0.is_empty());

    // Recording 100 RPC calls reduces available by 100.
    let mut updates = HashMap::new();
    updates.insert(
        account_id,
        ResourceUsageUpdate {
            monthly_usage_mode_revision: 0,
            memory_byte_nanoseconds_remainder: 0,
            durable_storage_byte_nanoseconds_remainder: 0,
            ephemeral_storage_byte_nanoseconds_remainder: 0,
            fuel_delta: 0,
            http_call_count_delta: 0,
            rpc_call_count_delta: 100,
            durable_storage_byte_seconds_delta: 0,
            ephemeral_storage_byte_seconds_delta: 0,
            memory_gb_seconds_delta: 0,
            metering: golem_service_base::clients::registry::ResourceUsageMetering::all_enabled(),
        },
    );
    let result = svc
        .update_resource_usage(updates, &AuthCtx::System)
        .await
        .unwrap();
    let limits = result
        .0
        .get(&account_id)
        .expect("account must be in response");
    check!(
        limits.available_rpc_calls == 4900,
        "expected 4900, got {}",
        limits.available_rpc_calls
    );
    // HTTP should be untouched.
    check!(
        limits.available_http_calls == 5000,
        "expected HTTP untouched at 5000, got {}",
        limits.available_http_calls
    );

    // Exactly reaching the limit returns 0.
    let mut updates = HashMap::new();
    updates.insert(
        account_id,
        ResourceUsageUpdate {
            monthly_usage_mode_revision: 0,
            memory_byte_nanoseconds_remainder: 0,
            durable_storage_byte_nanoseconds_remainder: 0,
            ephemeral_storage_byte_nanoseconds_remainder: 0,
            fuel_delta: 0,
            http_call_count_delta: 0,
            rpc_call_count_delta: 4900,
            durable_storage_byte_seconds_delta: 0,
            ephemeral_storage_byte_seconds_delta: 0,
            memory_gb_seconds_delta: 0,
            metering: golem_service_base::clients::registry::ResourceUsageMetering::all_enabled(),
        },
    );
    let result = svc
        .update_resource_usage(updates, &AuthCtx::System)
        .await
        .unwrap();
    let limits = result.0.get(&account_id).unwrap();
    check!(
        limits.available_rpc_calls == 0,
        "expected 0 at limit, got {}",
        limits.available_rpc_calls
    );

    // Slightly exceeding the limit is allowed (optimistic) — saturates at 0, not an error.
    let mut updates = HashMap::new();
    updates.insert(
        account_id,
        ResourceUsageUpdate {
            monthly_usage_mode_revision: 0,
            memory_byte_nanoseconds_remainder: 0,
            durable_storage_byte_nanoseconds_remainder: 0,
            ephemeral_storage_byte_nanoseconds_remainder: 0,
            fuel_delta: 0,
            http_call_count_delta: 0,
            rpc_call_count_delta: 1,
            durable_storage_byte_seconds_delta: 0,
            ephemeral_storage_byte_seconds_delta: 0,
            memory_gb_seconds_delta: 0,
            metering: golem_service_base::clients::registry::ResourceUsageMetering::all_enabled(),
        },
    );
    let result = svc
        .update_resource_usage(updates, &AuthCtx::System)
        .await
        .unwrap();
    let limits = result.0.get(&account_id).unwrap();
    check!(
        limits.available_rpc_calls == 0,
        "available_rpc_calls should saturate at 0, got {}",
        limits.available_rpc_calls
    );
}

pub async fn test_update_call_counts_batch(deps: &Deps) {
    use golem_common::model::account::AccountId;
    use golem_registry_service::services::account_usage::ResourceUsageUpdate;
    use golem_service_base::model::auth::AuthCtx;
    use std::collections::HashMap;

    let svc = deps.account_usage_service();
    let a1 = AccountId(deps.create_account().await.revision.account_id);
    let a2 = AccountId(deps.create_account().await.revision.account_id);
    let a3 = AccountId(deps.create_account().await.revision.account_id);
    let a4 = AccountId(deps.create_account().await.revision.account_id);

    // HTTP batch — multiple accounts in one call.
    let mut http_updates = HashMap::new();
    http_updates.insert(
        a1,
        ResourceUsageUpdate {
            monthly_usage_mode_revision: 0,
            memory_byte_nanoseconds_remainder: 0,
            durable_storage_byte_nanoseconds_remainder: 0,
            ephemeral_storage_byte_nanoseconds_remainder: 0,
            fuel_delta: 0,
            http_call_count_delta: 50,
            rpc_call_count_delta: 0,
            durable_storage_byte_seconds_delta: 0,
            ephemeral_storage_byte_seconds_delta: 0,
            memory_gb_seconds_delta: 0,
            metering: golem_service_base::clients::registry::ResourceUsageMetering::all_enabled(),
        },
    );
    http_updates.insert(
        a2,
        ResourceUsageUpdate {
            monthly_usage_mode_revision: 0,
            memory_byte_nanoseconds_remainder: 0,
            durable_storage_byte_nanoseconds_remainder: 0,
            ephemeral_storage_byte_nanoseconds_remainder: 0,
            fuel_delta: 0,
            http_call_count_delta: 200,
            rpc_call_count_delta: 0,
            durable_storage_byte_seconds_delta: 0,
            ephemeral_storage_byte_seconds_delta: 0,
            memory_gb_seconds_delta: 0,
            metering: golem_service_base::clients::registry::ResourceUsageMetering::all_enabled(),
        },
    );
    let result = svc
        .update_resource_usage(http_updates, &AuthCtx::System)
        .await
        .unwrap();
    check!(
        result.0.get(&a1).unwrap().available_http_calls == 4950,
        "a1: expected 4950"
    );
    check!(
        result.0.get(&a2).unwrap().available_http_calls == 4800,
        "a2: expected 4800"
    );
    // Accounts not in the batch are absent from the response.
    assert!(
        !result.0.contains_key(&a3),
        "a3 should not appear in HTTP response"
    );

    // RPC batch — different accounts, same call.
    let mut rpc_updates = HashMap::new();
    rpc_updates.insert(
        a3,
        ResourceUsageUpdate {
            monthly_usage_mode_revision: 0,
            memory_byte_nanoseconds_remainder: 0,
            durable_storage_byte_nanoseconds_remainder: 0,
            ephemeral_storage_byte_nanoseconds_remainder: 0,
            fuel_delta: 0,
            http_call_count_delta: 0,
            rpc_call_count_delta: 300,
            durable_storage_byte_seconds_delta: 0,
            ephemeral_storage_byte_seconds_delta: 0,
            memory_gb_seconds_delta: 0,
            metering: golem_service_base::clients::registry::ResourceUsageMetering::all_enabled(),
        },
    );
    rpc_updates.insert(
        a4,
        ResourceUsageUpdate {
            monthly_usage_mode_revision: 0,
            memory_byte_nanoseconds_remainder: 0,
            durable_storage_byte_nanoseconds_remainder: 0,
            ephemeral_storage_byte_nanoseconds_remainder: 0,
            fuel_delta: 0,
            http_call_count_delta: 0,
            rpc_call_count_delta: 1000,
            durable_storage_byte_seconds_delta: 0,
            ephemeral_storage_byte_seconds_delta: 0,
            memory_gb_seconds_delta: 0,
            metering: golem_service_base::clients::registry::ResourceUsageMetering::all_enabled(),
        },
    );
    let result = svc
        .update_resource_usage(rpc_updates, &AuthCtx::System)
        .await
        .unwrap();
    check!(
        result.0.get(&a3).unwrap().available_rpc_calls == 4700,
        "a3: expected 4700"
    );
    check!(
        result.0.get(&a4).unwrap().available_rpc_calls == 4000,
        "a4: expected 4000"
    );
    // HTTP budgets are untouched for accounts that only had RPC calls recorded.
    check!(
        result.0.get(&a3).unwrap().available_http_calls == 5000,
        "a3: HTTP should be untouched"
    );
    check!(
        result.0.get(&a4).unwrap().available_http_calls == 5000,
        "a4: HTTP should be untouched"
    );
    // a1/a2 (HTTP-only) are absent from the RPC response.
    assert!(
        !result.0.contains_key(&a1),
        "a1 should not appear in RPC response"
    );
    assert!(
        !result.0.contains_key(&a2),
        "a2 should not appear in RPC response"
    );
}
