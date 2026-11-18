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

use super::{Deps, TestDb};
use golem_common::model::account::AccountId;
use golem_common::model::agent_config::{SharedConfigId, SharedConfigRevision};
use golem_common::model::agent_secret::{
    AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::schema::{
    SchemaGraph, SchemaType, SchemaTypeDef, SchemaValue, SecretSpec, TypeId,
};
use golem_registry_service::repo::model::agent_secrets::{
    AgentSecretCreationRecord, AgentSecretRevisionData, AgentSecretRevisionRecord,
};
use golem_registry_service::repo::model::shared_config::{
    SharedConfigCreationRecord, SharedConfigRepoError, SharedConfigUpdateRecord,
};
use golem_registry_service::repo::shared_config::{DbSharedConfigRepo, SharedConfigRepo};
use golem_registry_service::services::registry_change_notifier::RequiresNotificationSignalExt;
use golem_service_base::repo::Blob;

fn repository(db: &TestDb) -> Box<dyn SharedConfigRepo> {
    match db {
        TestDb::Postgres(pool) => Box::new(DbSharedConfigRepo::new(pool.clone())),
        TestDb::Sqlite(pool) => Box::new(DbSharedConfigRepo::new(pool.clone())),
    }
}

pub async fn lifecycle(deps: &Deps) {
    let repo = repository(&deps.test_db);
    let other_connection = repository(&deps.routing_test_db);
    let owner = deps.create_account().await;
    let actor = AccountId(owner.revision.account_id);
    let app = deps.create_application(actor.0).await;
    let env = deps.create_env(app.revision.application_id).await;
    let environment_id = EnvironmentId(env.revision.environment_id);
    let id = SharedConfigId::new();
    let path = vec!["service".to_string(), "maxRetries".to_string()];
    let schema = SchemaGraph::anonymous(SchemaType::option(SchemaType::s32()));
    let initial = SharedConfigRevision::INITIAL;
    let first = repo
        .create(SharedConfigCreationRecord {
            id,
            environment_id,
            path: path.clone(),
            schema: schema.clone(),
            value: None,
            actor,
        })
        .await
        .unwrap();
    assert_eq!(first.revision_id, 0);
    assert_eq!(first.revision_data.value().value, None);
    assert_eq!(first.audit.created_by, actor.0);
    assert_eq!(repo.get(id).await.unwrap(), Some(first.clone()));
    assert_eq!(
        repo.find(environment_id, path.clone()).await.unwrap(),
        Some(first.clone())
    );
    for (env, path) in [
        (EnvironmentId::new(), path.clone()),
        (environment_id, vec!["service".into(), "maxretries".into()]),
        (environment_id, vec!["service.maxRetries".into()]),
    ] {
        assert!(repo.find(env, path).await.unwrap().is_none());
    }
    assert!(matches!(
        repo.create(SharedConfigCreationRecord {
            id: SharedConfigId::new(),
            environment_id,
            path: path.clone(),
            schema: schema.clone(),
            value: None,
            actor,
        })
        .await,
        Err(SharedConfigRepoError::PathAlreadyExists)
    ));

    let null = repo
        .update(SharedConfigUpdateRecord {
            id,
            current_revision: initial,
            schema: schema.clone(),
            value: Some(SchemaValue::Option { inner: None }),
            actor,
        })
        .await
        .unwrap();
    assert_eq!(null.revision_id, 1);
    assert_eq!(
        null.revision_data.value().value,
        Some(SchemaValue::Option { inner: None })
    );
    let one = initial.next().unwrap();
    let value = |number| {
        Some(SchemaValue::Option {
            inner: Some(Box::new(SchemaValue::S32(number))),
        })
    };
    let (a, b) = tokio::join!(
        repo.update(SharedConfigUpdateRecord {
            id,
            current_revision: one,
            schema: schema.clone(),
            value: value(17),
            actor,
        }),
        other_connection.update(SharedConfigUpdateRecord {
            id,
            current_revision: one,
            schema: schema.clone(),
            value: value(23),
            actor,
        }),
    );
    let winner = match (a, b) {
        (Ok(record), Err(SharedConfigRepoError::ConcurrentModification)) => {
            assert_eq!(record.revision_data.value().value, value(17));
            record
        }
        (Err(SharedConfigRepoError::ConcurrentModification), Ok(record)) => {
            assert_eq!(record.revision_data.value().value, value(23));
            record
        }
        results => panic!("expected exactly one committed edit and one conflict: {results:?}"),
    };
    assert_eq!(winner.revision_id, 2);
    assert!(matches!(
        repo.delete(id, one, actor).await,
        Err(SharedConfigRepoError::ConcurrentModification)
    ));
    let two = one.next().unwrap();
    assert!(matches!(
        repo.update(SharedConfigUpdateRecord {
            id,
            current_revision: two.next().unwrap(),
            schema: schema.clone(),
            value: value(41),
            actor,
        })
        .await,
        Err(SharedConfigRepoError::ConcurrentModification)
    ));
    assert!(
        repo.get_revision(id, two.next().unwrap())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(repo.get(id).await.unwrap(), Some(winner.clone()));
    assert_eq!(repo.get_revision(id, initial).await.unwrap(), Some(first));
    assert_eq!(repo.get_revision(id, one).await.unwrap(), Some(null));

    let tombstone = repo.delete(id, two, actor).await.unwrap();
    assert_eq!(tombstone.revision_id, 3);
    assert!(tombstone.audit.deleted);
    assert_eq!(repo.get(id).await.unwrap(), Some(tombstone.clone()));
    assert_eq!(
        repo.get_revision(id, two.next().unwrap()).await.unwrap(),
        Some(tombstone)
    );
    assert!(
        repo.find(environment_id, path.clone())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(repo.get_revision(id, two).await.unwrap(), Some(winner));
    assert!(matches!(
        repo.update(SharedConfigUpdateRecord {
            id,
            current_revision: two.next().unwrap(),
            schema: schema.clone(),
            value: value(41),
            actor,
        })
        .await,
        Err(SharedConfigRepoError::Removed)
    ));

    let replacement_id = SharedConfigId::new();
    let replacement = repo
        .create(SharedConfigCreationRecord {
            id: replacement_id,
            environment_id,
            path: path.clone(),
            schema: SchemaGraph::anonymous(SchemaType::bool()),
            value: Some(SchemaValue::Bool(false)),
            actor,
        })
        .await
        .unwrap();
    assert_eq!(
        repo.find(environment_id, path.clone()).await.unwrap(),
        Some(replacement.clone())
    );
    assert!(matches!(
        replacement.check_schema(&schema),
        Err(SharedConfigRepoError::IncompatibleSchema)
    ));
    assert!(repo.get(id).await.unwrap().unwrap().audit.deleted);
    assert!(
        repo.get_revision(replacement_id, one)
            .await
            .unwrap()
            .is_none()
    );

    repo.delete(replacement_id, initial, actor).await.unwrap();
    let compatible = repo
        .create(SharedConfigCreationRecord {
            id: SharedConfigId::new(),
            environment_id,
            path: path.clone(),
            schema: schema.clone(),
            value: value(43),
            actor,
        })
        .await
        .unwrap();
    compatible.check_schema(&schema).unwrap();
    assert_eq!(
        repo.find(environment_id, path).await.unwrap(),
        Some(compatible)
    );
    assert_eq!(
        repo.get_revision(replacement_id, initial).await.unwrap(),
        Some(replacement)
    );
    assert!(repo.get(id).await.unwrap().unwrap().audit.deleted);
}

fn node_schema(id: &str) -> SchemaGraph {
    let id = TypeId::new(id);
    let node = SchemaType::ref_to(id.clone());
    SchemaGraph {
        root: node.clone(),
        defs: vec![SchemaTypeDef {
            id,
            name: None,
            body: SchemaType::record_from_fields([
                ("label", SchemaType::string()),
                ("credential", SchemaType::secret(SecretSpec::default())),
                ("next", SchemaType::option(node)),
            ]),
        }],
    }
}

pub async fn schema_and_secret_identity(deps: &Deps) {
    let repo = repository(&deps.test_db);
    let owner = deps.create_account().await;
    let actor = AccountId(owner.revision.account_id);
    let app = deps.create_application(actor.0).await;
    let env = deps.create_env(app.revision.application_id).await;
    let environment_id = EnvironmentId(env.revision.environment_id);
    let secret_id = AgentSecretId::new();
    let secret = deps
        .agent_secret_repo
        .create(AgentSecretCreationRecord::new(
            secret_id,
            environment_id,
            CanonicalAgentSecretPath(vec!["key".into()]),
            SchemaGraph::anonymous(SchemaType::string()),
            Some(SchemaValue::String("initial".into())),
            actor,
        ))
        .await
        .unwrap()
        .signal_new_events_available(deps.test_registry_change_notifier().as_ref());
    let id = SharedConfigId::new();
    let schema = node_schema("stored-node");
    let value = SchemaValue::Record {
        fields: vec![
            SchemaValue::String("first".into()),
            SchemaValue::Uuid(secret_id.0),
            SchemaValue::Option { inner: None },
        ],
    };
    let stored = repo
        .create(SharedConfigCreationRecord {
            id,
            environment_id,
            path: vec!["public".into()],
            schema: schema.clone(),
            value: Some(value.clone()),
            actor,
        })
        .await
        .unwrap();
    assert_eq!(stored.schema.value(), &schema);
    assert_eq!(stored.revision_data.value().value, Some(value.clone()));

    let mut rotated = secret.revision;
    rotated.revision_id += 1;
    rotated.agent_secret_revision_data = Blob::new(AgentSecretRevisionData {
        secret_value: Some(SchemaValue::String("rotated".into())),
    });
    let _ = deps.agent_secret_repo.update(rotated).await.unwrap();
    assert_eq!(repo.get(id).await.unwrap(), Some(stored.clone()));

    let _ = deps
        .agent_secret_repo
        .delete(
            AgentSecretRevisionRecord::delete(
                secret_id,
                AgentSecretRevision::INITIAL.next().unwrap(),
                actor,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let new_secret_id = AgentSecretId::new();
    let _ = deps
        .agent_secret_repo
        .create(AgentSecretCreationRecord::new(
            new_secret_id,
            environment_id,
            CanonicalAgentSecretPath(vec!["key".into()]),
            SchemaGraph::anonymous(SchemaType::string()),
            Some(SchemaValue::String("replacement".into())),
            actor,
        ))
        .await
        .unwrap();
    assert_ne!(secret_id, new_secret_id);
    assert_eq!(repo.get(id).await.unwrap(), Some(stored.clone()));
    assert_eq!(
        repo.get_revision(id, SharedConfigRevision::INITIAL)
            .await
            .unwrap(),
        Some(stored)
    );

    let equivalent = node_schema("independent-node");
    let updated = repo
        .update(SharedConfigUpdateRecord {
            id,
            current_revision: SharedConfigRevision::INITIAL,
            schema: equivalent,
            value: Some(value.clone()),
            actor,
        })
        .await
        .unwrap();
    assert_eq!(updated.schema.value(), &schema);

    let mut incompatible = schema.clone();
    if let SchemaType::Record { fields, .. } = &mut incompatible.defs[0].body {
        fields.swap(0, 1);
    }
    assert!(matches!(
        repo.update(SharedConfigUpdateRecord {
            id,
            current_revision: SharedConfigRevision::INITIAL.next().unwrap(),
            schema: incompatible,
            value: Some(value.clone()),
            actor,
        })
        .await,
        Err(SharedConfigRepoError::IncompatibleSchema)
    ));

    let mut capability_value = value;
    if let SchemaValue::Record { fields } = &mut capability_value {
        fields[1] = SchemaValue::Secret(golem_common::schema::schema_value::SecretValuePayload {
            secret_id: secret_id.0,
            config_key: None,
            version: 7,
            resolved_at: chrono::Utc::now(),
            category: None,
        });
    }
    assert!(matches!(
        repo.update(SharedConfigUpdateRecord {
            id,
            current_revision: SharedConfigRevision::INITIAL.next().unwrap(),
            schema: schema.clone(),
            value: Some(capability_value),
            actor,
        })
        .await,
        Err(SharedConfigRepoError::InvalidConfig(_))
    ));
    assert_eq!(repo.get(id).await.unwrap(), Some(updated));

    for schema in [
        SchemaGraph::anonymous(SchemaType::option(SchemaType::string())),
        SchemaGraph::anonymous(SchemaType::secret(SecretSpec {
            inner: Box::new(SchemaType::secret(SecretSpec::default())),
            category: None,
        })),
        SchemaGraph::anonymous(SchemaType::ref_to(TypeId::new("missing"))),
    ] {
        let wrong_value = if matches!(schema.root, SchemaType::Option { .. }) {
            Some(SchemaValue::Bool(false))
        } else {
            None
        };
        assert!(matches!(
            repo.create(SharedConfigCreationRecord {
                id: SharedConfigId::new(),
                environment_id,
                path: vec!["invalid".into()],
                schema,
                value: wrong_value,
                actor,
            })
            .await,
            Err(SharedConfigRepoError::InvalidConfig(_))
        ));
        assert!(
            repo.find(environment_id, vec!["invalid".into()])
                .await
                .unwrap()
                .is_none()
        );
    }
}
