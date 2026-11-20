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

use chrono::Utc;
use golem_common::config::{DbConfig, DbSqliteConfig};
use golem_common::model::card::{Card, CardId, CardManagedByRuntimeDerived, StoredCard};
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::component_metadata::{ComponentMetadata, KnownExports};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentId, InvocationId, OplogIndex};
use golem_registry_service::RegistryService as RegistryServer;
use golem_registry_service::config::{
    ComponentCompilationConfig, LoginConfig, RegistryServiceConfig,
};
use golem_registry_service::repo::application::{ApplicationRepo, DbApplicationRepo};
use golem_registry_service::repo::card::{CardRepo, DbCardRepo};
use golem_registry_service::repo::component::{ComponentRepo, DbComponentRepo};
use golem_registry_service::repo::environment::{DbEnvironmentRepo, EnvironmentRepo};
use golem_registry_service::repo::model::application::ApplicationRevisionRecord;
use golem_registry_service::repo::model::audit::DeletableRevisionAuditFields;
use golem_registry_service::repo::model::component::ComponentRevisionRecord;
use golem_registry_service::repo::model::environment::EnvironmentRevisionRecord;
use golem_registry_service::repo::model::hash::SqlBlake3Hash;
use golem_registry_service::repo::registry_change::{
    ChangeEventId, DbRegistryChangeRepo, RegistryChangeEvent, RegistryChangeRepo,
};
use golem_schema::Empty;
use golem_service_base::clients::registry::{
    GrpcRegistryService, GrpcRegistryServiceConfig, RegistryService, RegistryServiceError,
};
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::repo::Blob;
use tempfile::TempDir;
use test_r::{test, timeout};
use tokio::task::JoinSet;

fn runtime_card(card_id: CardId, parent_ids: Vec<CardId>) -> StoredCard {
    StoredCard::Concrete(Card {
        card_id,
        parent_ids,
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: Utc::now(),
        expires_at: None,
        system_card: false,
        managed_by: None,
    })
}

fn runtime_card_provenance(oplog_index: u64) -> CardManagedByRuntimeDerived {
    CardManagedByRuntimeDerived {
        environment_id: EnvironmentId::new(),
        agent_id: AgentId::from_parts(7, 2),
        invocation_key: InvocationId(uuid::Uuid::from_u128(u128::from(oplog_index))),
        oplog_index: OplogIndex::from_u64(oplog_index),
    }
}

async fn start_registry() -> (
    TempDir,
    DbSqliteConfig,
    JoinSet<Result<(), anyhow::Error>>,
    u16,
) {
    start_registry_with_issuer(None).await
}

async fn start_registry_with_issuer(
    golem_issuer: Option<golem_common::model::golem_issuer::GolemIssuer>,
) -> (
    TempDir,
    DbSqliteConfig,
    JoinSet<Result<(), anyhow::Error>>,
    u16,
) {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_config = DbSqliteConfig {
        database: temp_dir
            .path()
            .join("registry.db")
            .to_string_lossy()
            .into_owned(),
        max_connections: 4,
        foreign_keys: true,
    };
    let config = RegistryServiceConfig {
        golem_issuer,
        grpc: golem_registry_service::config::GrpcApiConfig {
            port: 0,
            ..Default::default()
        },
        db: DbConfig::Sqlite(db_config.clone()),
        login: LoginConfig::Disabled(Empty {}),
        blob_storage: BlobStorageConfig::default_in_memory(),
        component_compilation: ComponentCompilationConfig::Disabled(Empty {}),
        ..Default::default()
    };
    let mut join_set = JoinSet::new();
    let details = RegistryServer::new(config, prometheus::Registry::new())
        .start_for_single_executable(&mut join_set)
        .await
        .unwrap();

    (temp_dir, db_config, join_set, details.grpc_port)
}

fn registry_client(port: u16) -> GrpcRegistryService {
    GrpcRegistryService::new(&GrpcRegistryServiceConfig {
        host: "127.0.0.1".to_string(),
        port,
        ..Default::default()
    })
}

#[test]
#[timeout("120s")]
async fn every_client_gets_the_one_generated_golem_issuer_of_the_registry() {
    let (_temp_dir, _db_config, _join_set, port) = start_registry().await;
    let first = registry_client(port).get_golem_issuer().await.unwrap();
    assert!(first.is_generated());
    assert_eq!(
        registry_client(port).get_golem_issuer().await.unwrap(),
        first
    );
}

#[test]
#[timeout("120s")]
async fn a_configured_golem_issuer_is_what_the_registry_gives() {
    let configured: golem_common::model::golem_issuer::GolemIssuer =
        "https://id.acme.example".parse().unwrap();
    let (_temp_dir, _db_config, _join_set, port) =
        start_registry_with_issuer(Some(configured.clone())).await;
    assert_eq!(
        registry_client(port).get_golem_issuer().await.unwrap(),
        configured
    );
}

#[test]
#[timeout("120s")]
async fn component_grpc_metadata_retains_deleted_revisions_and_tombstones() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_config = DbSqliteConfig {
        database: temp_dir
            .path()
            .join("registry.db")
            .to_string_lossy()
            .into_owned(),
        max_connections: 4,
        foreign_keys: true,
    };
    let config = RegistryServiceConfig {
        db: DbConfig::Sqlite(db_config.clone()),
        login: LoginConfig::Disabled(Empty {}),
        blob_storage: BlobStorageConfig::default_in_memory(),
        component_compilation: ComponentCompilationConfig::Disabled(Empty {}),
        ..Default::default()
    };
    let mut join_set = JoinSet::new();
    let services = golem_registry_service::bootstrap::Services::new_without_component_builtins(
        &config,
        &mut join_set,
    )
    .await
    .unwrap();
    let port = golem_registry_service::grpc::start_grpc_server(
        "127.0.0.1:0".parse().unwrap(),
        &config.grpc,
        &services,
        &mut join_set,
    )
    .await
    .unwrap();
    let client = registry_client(port);
    let pool = SqlitePool::configured(&db_config).await.unwrap();
    let owner = config.initial_accounts["root"].id.0;
    let application_id = uuid::Uuid::new_v4();
    DbApplicationRepo::new(pool.clone())
        .create(
            owner,
            ApplicationRevisionRecord {
                application_id,
                revision_id: 0,
                name: "historical-read".to_string(),
                audit: DeletableRevisionAuditFields::new(owner),
            },
        )
        .await
        .unwrap();
    let environment_id = EnvironmentId::new();
    DbEnvironmentRepo::new(pool.clone())
        .create(
            application_id,
            EnvironmentRevisionRecord {
                environment_id: environment_id.0,
                revision_id: 0,
                name: "test".to_string(),
                audit: DeletableRevisionAuditFields::new(owner),
                compatibility_check: true,
                tool_compatibility_mode: "structural-subtype".to_string(),
                version_check: true,
                security_overrides: true,
                hash: SqlBlake3Hash::empty(),
            },
            crate::repo::test_environment_default_card_record(environment_id.0),
        )
        .await
        .unwrap();
    let component_id = ComponentId::new();
    let component_repo = DbComponentRepo::new(pool);
    component_repo
        .create(
            environment_id.0,
            "component",
            ComponentRevisionRecord {
                component_id: component_id.0,
                revision_id: 0,
                hash: SqlBlake3Hash::empty(),
                audit: DeletableRevisionAuditFields::new(owner),
                size: 17.into(),
                metadata: Blob::new(ComponentMetadata::from_parts(
                    KnownExports::default(),
                    vec![],
                    vec![],
                    Default::default(),
                )),
                object_store_key: "missing-binary".to_string(),
                binary_hash: SqlBlake3Hash::empty(),
                package_metadata: None,
            },
            vec![],
        )
        .await
        .unwrap();
    let original = client
        .get_component_metadata(component_id, ComponentRevision::INITIAL)
        .await
        .unwrap();
    let tombstone = ComponentRevision::INITIAL.next().unwrap();
    let _deleted = component_repo
        .delete(owner, component_id.0, tombstone.into())
        .await
        .unwrap();

    for revision in [ComponentRevision::INITIAL, tombstone] {
        let retained = client
            .get_component_metadata(component_id, revision)
            .await
            .unwrap();
        assert_eq!(retained.id, component_id);
        assert_eq!(retained.revision, revision);
        if revision == ComponentRevision::INITIAL {
            assert_eq!(retained.component_size, 17);
            assert_eq!(retained.metadata, original.metadata);
        } else {
            assert_eq!(retained.component_size, 0);
        }
        assert!(matches!(
            client.download_component(component_id, revision).await,
            Err(RegistryServiceError::NotFound(_))
        ));
    }
    join_set.abort_all();
}

#[test]
#[timeout("120s")]
async fn runtime_card_grpc_lifecycle_is_shared_across_executor_clients() {
    let (_temp_dir, db_config, mut join_set, port) = start_registry().await;
    let writer = registry_client(port);
    let reader = registry_client(port);

    let parent_id = CardId::new();
    writer
        .create_runtime_card(
            runtime_card(parent_id, Vec::new()),
            runtime_card_provenance(1),
        )
        .await
        .unwrap();

    let card_id = CardId::new();
    let card = runtime_card(card_id, vec![parent_id]);
    let provenance = runtime_card_provenance(2);
    let created = writer
        .create_runtime_card(card.clone(), provenance.clone())
        .await
        .unwrap();

    assert_eq!(
        writer
            .create_runtime_card(card.clone(), provenance.clone())
            .await
            .unwrap(),
        created
    );
    assert_eq!(
        writer
            .create_runtime_card(created.clone(), provenance.clone())
            .await
            .unwrap(),
        created
    );

    let mut conflicting = card.clone().into_concrete().unwrap();
    conflicting.expires_at = Some(Utc::now() + chrono::Duration::days(1));
    assert!(matches!(
        writer
            .create_runtime_card(conflicting.into(), provenance)
            .await,
        Err(RegistryServiceError::AlreadyExists(_))
    ));

    assert_eq!(
        reader.batch_get_cards(vec![card_id]).await.unwrap(),
        vec![created]
    );

    let missing_parent_card_id = CardId::new();
    assert!(matches!(
        writer
            .create_runtime_card(
                runtime_card(missing_parent_card_id, vec![CardId::new()]),
                runtime_card_provenance(3),
            )
            .await,
        Err(RegistryServiceError::NotFound(_))
    ));
    assert!(
        reader
            .batch_get_cards(vec![missing_parent_card_id])
            .await
            .unwrap()
            .is_empty()
    );

    let revoked_parent_id = CardId::new();
    writer
        .create_runtime_card(
            runtime_card(revoked_parent_id, Vec::new()),
            runtime_card_provenance(4),
        )
        .await
        .unwrap();
    let card_repo = DbCardRepo::logged(SqlitePool::configured(&db_config).await.unwrap());
    let _ = card_repo.delete(revoked_parent_id).await.unwrap();

    assert!(matches!(
        writer
            .create_runtime_card(
                runtime_card(revoked_parent_id, Vec::new()),
                runtime_card_provenance(4),
            )
            .await,
        Err(RegistryServiceError::RuntimeCardRevoked(_))
    ));

    let revoked_parent_card_id = CardId::new();
    assert!(matches!(
        writer
            .create_runtime_card(
                runtime_card(revoked_parent_card_id, vec![revoked_parent_id]),
                runtime_card_provenance(5),
            )
            .await,
        Err(RegistryServiceError::NotFound(_))
    ));
    assert!(
        reader
            .batch_get_cards(vec![revoked_parent_card_id])
            .await
            .unwrap()
            .is_empty()
    );

    let sibling_id = CardId::new();
    writer
        .create_runtime_card(
            runtime_card(sibling_id, vec![parent_id]),
            runtime_card_provenance(6),
        )
        .await
        .unwrap();
    let diamond_id = CardId::new();
    writer
        .create_runtime_card(
            runtime_card(diamond_id, vec![card_id, sibling_id]),
            runtime_card_provenance(7),
        )
        .await
        .unwrap();

    let mut expected_revoked = vec![parent_id, card_id, sibling_id, diamond_id];
    expected_revoked.sort_unstable();
    assert_eq!(
        writer.revoke_card(parent_id).await.unwrap(),
        expected_revoked
    );
    let registry_change_repo =
        DbRegistryChangeRepo::new(SqlitePool::configured(&db_config).await.unwrap());
    let events = registry_change_repo
        .get_events_since(ChangeEventId(0))
        .await
        .unwrap();
    let emitted_card_ids = events
        .into_iter()
        .find_map(|event| match event {
            RegistryChangeEvent::CardRevoked { card_ids, .. }
                if card_ids.contains(&parent_id.0) =>
            {
                Some(card_ids)
            }
            _ => None,
        })
        .expect("atomic DAG revoke must emit a card revocation event");
    assert_eq!(
        emitted_card_ids,
        expected_revoked
            .iter()
            .map(|card_id| card_id.0)
            .collect::<Vec<_>>()
    );
    assert!(
        reader
            .batch_get_cards(expected_revoked.clone())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        writer.revoke_card(parent_id).await,
        Err(RegistryServiceError::NotFound(_))
    ));

    join_set.abort_all();
}

#[test]
#[timeout("120s")]
async fn revoked_runtime_card_cannot_be_resurrected_by_an_identical_create_retry() {
    let (_temp_dir, _db_config, mut join_set, port) = start_registry().await;
    let client = registry_client(port);

    let parent_id = CardId::new();
    client
        .create_runtime_card(
            runtime_card(parent_id, Vec::new()),
            runtime_card_provenance(1),
        )
        .await
        .unwrap();

    let card_id = CardId::new();
    let card = runtime_card(card_id, vec![parent_id]);
    let provenance = runtime_card_provenance(2);
    client
        .create_runtime_card(card.clone(), provenance.clone())
        .await
        .unwrap();
    assert_eq!(client.revoke_card(card_id).await.unwrap(), vec![card_id]);

    let retry = client.create_runtime_card(card, provenance).await;
    let cards = client.batch_get_cards(vec![card_id]).await.unwrap();
    assert!(
        retry.is_err() && cards.is_empty(),
        "an idempotent retry must not make a revoked deterministic card ID live again: retry={retry:?}, cards={cards:?}"
    );

    join_set.abort_all();
}

#[test]
#[timeout("120s")]
async fn runtime_card_creation_rejects_an_expired_parent() {
    let (_temp_dir, _db_config, mut join_set, port) = start_registry().await;
    let client = registry_client(port);

    let parent_id = CardId::new();
    let mut parent = runtime_card(parent_id, Vec::new()).into_concrete().unwrap();
    parent.expires_at = Some(Utc::now() + chrono::Duration::milliseconds(100));
    client
        .create_runtime_card(parent.into(), runtime_card_provenance(1))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let child_id = CardId::new();
    let creation = client
        .create_runtime_card(
            runtime_card(child_id, vec![parent_id]),
            runtime_card_provenance(2),
        )
        .await;
    let cards = client.batch_get_cards(vec![child_id]).await.unwrap();
    assert!(
        creation.is_err() && cards.is_empty(),
        "runtime card creation must validate that every parent is live: creation={creation:?}, cards={cards:?}"
    );

    join_set.abort_all();
}
