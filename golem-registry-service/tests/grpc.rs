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
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentId, Empty, IdempotencyKey, OplogIndex};
use golem_registry_service::RegistryService as RegistryServer;
use golem_registry_service::config::{
    ComponentCompilationConfig, LoginConfig, RegistryServiceConfig,
};
use golem_registry_service::repo::card::{CardRepo, DbCardRepo};
use golem_registry_service::repo::registry_change::{
    ChangeEventId, DbRegistryChangeRepo, RegistryChangeEvent, RegistryChangeRepo,
};
use golem_service_base::clients::registry::{
    GrpcRegistryService, GrpcRegistryServiceConfig, RegistryService, RegistryServiceError,
};
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::db::sqlite::SqlitePool;
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
        agent_id: AgentId {
            component_id: ComponentId::new(),
            agent_id: "runtime-card-grpc-test".to_string(),
        },
        invocation_key: IdempotencyKey::new(format!("runtime-card-grpc-test-{oplog_index}")),
        oplog_index: OplogIndex::from_u64(oplog_index),
    }
}

async fn start_registry() -> (
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
