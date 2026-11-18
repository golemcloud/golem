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

//! Ordinary Shared values keep one fixed schema per identity. Incompatible schema
//! reuse fails rather than creating schema-specific histories. Uninitialized values
//! differ from configured optional absence, false and empty collections.
//!
//! Revisions and removal tombstones remain available by identity. Recreating an
//! exact catalog path creates another identity; it never rewrites historical values
//! or their nested Secret IDs. There is no history purge in this repository.
//!
//! This module supplies persistence, not guest reads or management endpoints.
//! Callers must apply their operation's authority checks before repository access.

use super::model::BindFields;
use super::model::audit::DeletableRevisionAuditFields;
use super::model::shared_config::{
    SharedConfigCreationRecord, SharedConfigRecord, SharedConfigRepoError,
    SharedConfigRevisionData, SharedConfigUpdateRecord, stored_value,
};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use golem_common::model::account::AccountId;
use golem_common::model::agent_config::{SharedConfigId, SharedConfigRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::schema::config::valid_catalog_name;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo::{Blob, PoolLabelledTransaction, ResultExt};
use sqlx::types::Json;

/// Persistence only. Callers must authorize reads, edits and provisioning before
/// entering this repository. Historical lookup deliberately includes tombstones.
#[async_trait]
pub trait SharedConfigRepo: Send + Sync {
    async fn create(
        &self,
        record: SharedConfigCreationRecord,
    ) -> Result<SharedConfigRecord, SharedConfigRepoError>;

    async fn update(
        &self,
        record: SharedConfigUpdateRecord,
    ) -> Result<SharedConfigRecord, SharedConfigRepoError>;

    async fn delete(
        &self,
        id: SharedConfigId,
        current_revision: SharedConfigRevision,
        actor: AccountId,
    ) -> Result<SharedConfigRecord, SharedConfigRepoError>;

    /// Returns the current revision, including a removal tombstone.
    async fn get(
        &self,
        id: SharedConfigId,
    ) -> Result<Option<SharedConfigRecord>, SharedConfigRepoError>;

    /// Resolves the exact catalog path to its currently live identity.
    async fn find(
        &self,
        environment_id: EnvironmentId,
        path: Vec<String>,
    ) -> Result<Option<SharedConfigRecord>, SharedConfigRepoError>;

    async fn get_revision(
        &self,
        id: SharedConfigId,
        revision: SharedConfigRevision,
    ) -> Result<Option<SharedConfigRecord>, SharedConfigRepoError>;
}

pub struct DbSharedConfigRepo<DBP: Pool> {
    db_pool: DBP,
}

impl<DBP: Pool> DbSharedConfigRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }
}

const SELECT_REVISION: &str = "
    SELECT c.shared_config_id, c.environment_id, c.path, c.schema,
           r.revision_id, r.revision_data, r.created_at, r.created_by, r.deleted
    FROM shared_configs c
    JOIN shared_config_revisions r ON r.shared_config_id = c.shared_config_id
";

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
impl DbSharedConfigRepo<PostgresPool> {
    async fn insert_revision(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        id: SharedConfigId,
        revision: SharedConfigRevision,
        data: SharedConfigRevisionData,
        audit: DeletableRevisionAuditFields,
    ) -> Result<SharedConfigRecord, SharedConfigRepoError> {
        tx.execute(
            sqlx::query(
                "INSERT INTO shared_config_revisions
             (shared_config_id, revision_id, revision_data, created_at, created_by, deleted)
             VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(id.0)
            .bind(i64::from(revision))
            .bind(Blob::new(data))
            .bind_deletable_revision_audit(audit),
        )
        .await
        .to_error_on_unique_violation(SharedConfigRepoError::ConcurrentModification)?;

        tx.fetch_one_as(
            sqlx::query_as(&format!(
                "{SELECT_REVISION} WHERE c.shared_config_id = $1 AND r.revision_id = $2"
            ))
            .bind(id.0)
            .bind(i64::from(revision)),
        )
        .await
        .map_err(Into::into)
    }

    async fn change_within_transaction(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        record: SharedConfigRecord,
        expected: SharedConfigRevision,
        data: SharedConfigRevisionData,
        audit: DeletableRevisionAuditFields,
    ) -> Result<SharedConfigRecord, SharedConfigRepoError> {
        if record.audit.deleted {
            return Err(SharedConfigRepoError::Removed);
        }
        let next = expected.next()?;
        let updated = tx
            .execute(
                sqlx::query(
                    "UPDATE shared_configs
             SET current_revision_id = $1, updated_at = $2, modified_by = $3, deleted_at = $4
             WHERE shared_config_id = $5 AND current_revision_id = $6 AND deleted_at IS NULL",
                )
                .bind(i64::from(next))
                .bind(&audit.created_at)
                .bind(audit.created_by)
                .bind(audit.deleted.then_some(audit.created_at.clone()))
                .bind(record.shared_config_id)
                .bind(i64::from(expected)),
            )
            .await?;
        if updated.rows_affected() != 1 {
            return Err(SharedConfigRepoError::ConcurrentModification);
        }
        Self::insert_revision(
            tx,
            SharedConfigId(record.shared_config_id),
            next,
            data,
            audit,
        )
        .await
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl SharedConfigRepo for DbSharedConfigRepo<PostgresPool> {
    async fn create(
        &self,
        record: SharedConfigCreationRecord,
    ) -> Result<SharedConfigRecord, SharedConfigRepoError> {
        if record.path.is_empty()
            || !record
                .path
                .iter()
                .all(|segment| valid_catalog_name(segment))
        {
            return Err(SharedConfigRepoError::InvalidConfig(
                "invalid catalog path".into(),
            ));
        }
        let data = stored_value(&record.schema, record.value)?;
        let audit = DeletableRevisionAuditFields::new(record.actor.0);
        self.db_pool
            .with_tx_err("shared-config", "create", |tx| {
                async move {
                    tx.execute(
                        sqlx::query(
                            "INSERT INTO shared_configs
                     (shared_config_id, environment_id, path, schema, current_revision_id,
                      created_at, updated_at, deleted_at, modified_by)
                     VALUES ($1, $2, $3, $4, $5, $6, $6, NULL, $7)",
                        )
                        .bind(record.id.0)
                        .bind(record.environment_id.0)
                        .bind(Json(record.path))
                        .bind(Blob::new(record.schema))
                        .bind(i64::from(SharedConfigRevision::INITIAL))
                        .bind(&audit.created_at)
                        .bind(audit.created_by),
                    )
                    .await
                    .to_error_on_unique_violation(SharedConfigRepoError::PathAlreadyExists)?;

                    Self::insert_revision(tx, record.id, SharedConfigRevision::INITIAL, data, audit)
                        .await
                }
                .boxed()
            })
            .await
    }

    async fn update(
        &self,
        update: SharedConfigUpdateRecord,
    ) -> Result<SharedConfigRecord, SharedConfigRepoError> {
        self.db_pool.with_tx_err("shared-config", "update", |tx| {
            async move {
                let current: SharedConfigRecord = tx.fetch_optional_as(sqlx::query_as(&format!(
                    "{SELECT_REVISION} WHERE c.shared_config_id = $1 AND r.revision_id = c.current_revision_id"
                )).bind(update.id.0)).await?.ok_or(SharedConfigRepoError::NotFound)?;
                current.check_schema(&update.schema)?;
                let data = stored_value(current.schema.value(), update.value)?;
                Self::change_within_transaction(tx, current, update.current_revision, data,
                    DeletableRevisionAuditFields::new(update.actor.0)).await
            }.boxed()
        }).await
    }

    async fn delete(
        &self,
        id: SharedConfigId,
        current_revision: SharedConfigRevision,
        actor: AccountId,
    ) -> Result<SharedConfigRecord, SharedConfigRepoError> {
        self.db_pool.with_tx_err("shared-config", "delete", |tx| {
            async move {
                let current = tx.fetch_optional_as(sqlx::query_as(&format!(
                    "{SELECT_REVISION} WHERE c.shared_config_id = $1 AND r.revision_id = c.current_revision_id"
                )).bind(id.0)).await?.ok_or(SharedConfigRepoError::NotFound)?;
                Self::change_within_transaction(tx, current, current_revision,
                    SharedConfigRevisionData { value: None },
                    DeletableRevisionAuditFields::deletion(actor.0)).await
            }.boxed()
        }).await
    }

    async fn get(
        &self,
        id: SharedConfigId,
    ) -> Result<Option<SharedConfigRecord>, SharedConfigRepoError> {
        self.db_pool.with_ro("shared-config", "get").fetch_optional_as(sqlx::query_as(&format!(
            "{SELECT_REVISION} WHERE c.shared_config_id = $1 AND r.revision_id = c.current_revision_id"
        )).bind(id.0)).await.map_err(Into::into)
    }

    async fn find(
        &self,
        environment_id: EnvironmentId,
        path: Vec<String>,
    ) -> Result<Option<SharedConfigRecord>, SharedConfigRepoError> {
        self.db_pool
            .with_ro("shared-config", "find")
            .fetch_optional_as(
                sqlx::query_as(&format!(
                    "{SELECT_REVISION}
             JOIN environments e ON e.environment_id = c.environment_id
             JOIN applications a ON a.application_id = e.application_id
             JOIN accounts o ON o.account_id = a.account_id
             WHERE c.environment_id = $1 AND c.path = $2
               AND r.revision_id = c.current_revision_id AND c.deleted_at IS NULL
               AND e.deleted_at IS NULL AND a.deleted_at IS NULL AND o.deleted_at IS NULL"
                ))
                .bind(environment_id.0)
                .bind(Json(path)),
            )
            .await
            .map_err(Into::into)
    }

    async fn get_revision(
        &self,
        id: SharedConfigId,
        revision: SharedConfigRevision,
    ) -> Result<Option<SharedConfigRecord>, SharedConfigRepoError> {
        self.db_pool
            .with_ro("shared-config", "get_revision")
            .fetch_optional_as(
                sqlx::query_as(&format!(
                    "{SELECT_REVISION} WHERE c.shared_config_id = $1 AND r.revision_id = $2"
                ))
                .bind(id.0)
                .bind(i64::from(revision)),
            )
            .await
            .map_err(Into::into)
    }
}
