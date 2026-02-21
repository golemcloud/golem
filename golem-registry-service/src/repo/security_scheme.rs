// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use super::model::security_scheme::{
    SecuritySchemeExtRevisionRecord, SecuritySchemeRepoError, SecuritySchemeRevisionRecord,
};
use crate::repo::model::BindFields;
pub use crate::repo::model::account::AccountRecord;
use crate::repo::model::security_scheme::SecuritySchemeRecord;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use golem_service_base::repo::ResultExt;
use indoc::indoc;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait SecuritySchemeRepo: Send + Sync {
    async fn create(
        &self,
        environment_id: Uuid,
        name: String,
        revision: SecuritySchemeRevisionRecord,
    ) -> Result<SecuritySchemeExtRevisionRecord, SecuritySchemeRepoError>;

    async fn update(
        &self,
        revision: SecuritySchemeRevisionRecord,
    ) -> Result<SecuritySchemeExtRevisionRecord, SecuritySchemeRepoError>;

    async fn delete(
        &self,
        revision: SecuritySchemeRevisionRecord,
    ) -> Result<SecuritySchemeExtRevisionRecord, SecuritySchemeRepoError>;

    async fn get_by_id(
        &self,
        security_scheme_id: Uuid,
    ) -> Result<Option<SecuritySchemeExtRevisionRecord>, SecuritySchemeRepoError>;

    async fn get_for_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<SecuritySchemeExtRevisionRecord>, SecuritySchemeRepoError>;

    async fn get_for_environment_and_name(
        &self,
        environment_id: Uuid,
        name: &str,
    ) -> Result<Option<SecuritySchemeExtRevisionRecord>, SecuritySchemeRepoError>;
}

pub struct LoggedSecuritySchemeRepo<Repo: SecuritySchemeRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "security scheme repository";

impl<Repo: SecuritySchemeRepo> LoggedSecuritySchemeRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_environment_id(environment_id: Uuid) -> Span {
        info_span!(SPAN_NAME, environment_id=%environment_id)
    }

    fn span_security_scheme_id(security_scheme_id: Uuid) -> Span {
        info_span!(SPAN_NAME, security_scheme_id=%security_scheme_id)
    }
}

#[async_trait]
impl<Repo: SecuritySchemeRepo> SecuritySchemeRepo for LoggedSecuritySchemeRepo<Repo> {
    async fn create(
        &self,
        environment_id: Uuid,
        name: String,
        revision: SecuritySchemeRevisionRecord,
    ) -> Result<SecuritySchemeExtRevisionRecord, SecuritySchemeRepoError> {
        let span = Self::span_environment_id(environment_id);
        self.repo
            .create(environment_id, name, revision)
            .instrument(span)
            .await
    }

    async fn update(
        &self,
        revision: SecuritySchemeRevisionRecord,
    ) -> Result<SecuritySchemeExtRevisionRecord, SecuritySchemeRepoError> {
        let span = Self::span_security_scheme_id(revision.security_scheme_id);
        self.repo.update(revision).instrument(span).await
    }

    async fn delete(
        &self,
        revision: SecuritySchemeRevisionRecord,
    ) -> Result<SecuritySchemeExtRevisionRecord, SecuritySchemeRepoError> {
        let span = Self::span_security_scheme_id(revision.security_scheme_id);
        self.repo.delete(revision).instrument(span).await
    }

    async fn get_by_id(
        &self,
        security_scheme_id: Uuid,
    ) -> Result<Option<SecuritySchemeExtRevisionRecord>, SecuritySchemeRepoError> {
        self.repo
            .get_by_id(security_scheme_id)
            .instrument(Self::span_security_scheme_id(security_scheme_id))
            .await
    }

    async fn get_for_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<SecuritySchemeExtRevisionRecord>, SecuritySchemeRepoError> {
        self.repo
            .get_for_environment(environment_id)
            .instrument(Self::span_environment_id(environment_id))
            .await
    }

    async fn get_for_environment_and_name(
        &self,
        environment_id: Uuid,
        name: &str,
    ) -> Result<Option<SecuritySchemeExtRevisionRecord>, SecuritySchemeRepoError> {
        self.repo
            .get_for_environment_and_name(environment_id, name)
            .instrument(Self::span_environment_id(environment_id))
            .await
    }
}

pub struct DbSecuritySchemeRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "security_schemes";

impl<DBP: Pool> DbSecuritySchemeRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedSecuritySchemeRepo<Self>
    where
        Self: SecuritySchemeRepo,
    {
        LoggedSecuritySchemeRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
impl DbSecuritySchemeRepo<PostgresPool> {
    async fn insert_revision(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        revision: SecuritySchemeRevisionRecord,
    ) -> Result<SecuritySchemeRevisionRecord, SecuritySchemeRepoError> {
        let revision: SecuritySchemeRevisionRecord = tx
            .fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    INSERT INTO security_scheme_revisions
                    (security_scheme_id, revision_id, provider_type, client_id, client_secret, redirect_url, scopes, created_at, created_by, deleted)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                    RETURNING security_scheme_id, revision_id, provider_type, client_id, client_secret, redirect_url, scopes, created_at, created_by, deleted
                "# })
                .bind(revision.security_scheme_id)
                .bind(revision.revision_id)
                .bind(revision.provider_type)
                .bind(revision.client_id)
                .bind(revision.client_secret)
                .bind(revision.redirect_url)
                .bind(revision.scopes)
                .bind_deletable_revision_audit(revision.audit),
            )
            .await
            .to_error_on_unique_violation(SecuritySchemeRepoError::ConcurrentModification)?;

        Ok(revision)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl SecuritySchemeRepo for DbSecuritySchemeRepo<PostgresPool> {
    async fn create(
        &self,
        environment_id: Uuid,
        name: String,
        revision: SecuritySchemeRevisionRecord,
    ) -> Result<SecuritySchemeExtRevisionRecord, SecuritySchemeRepoError> {
        self.db_pool.with_tx_err(METRICS_SVC_NAME, "create", |tx| {
            async move {
                let security_scheme_record: SecuritySchemeRecord = tx
                    .fetch_one_as(
                        sqlx::query_as(indoc! {r#"
                            INSERT INTO security_schemes (security_scheme_id, environment_id, name, created_at, updated_at, deleted_at, modified_by, current_revision_id)
                            VALUES ($1, $2, $3, $4, $4, NULL, $5, $6)
                            RETURNING security_scheme_id, environment_id, name, created_at, updated_at, deleted_at, modified_by, current_revision_id
                        "#})
                            .bind(revision.security_scheme_id)
                            .bind(environment_id)
                            .bind(name)
                            .bind(&revision.audit.created_at)
                            .bind(revision.audit.created_by)
                            .bind(revision.revision_id)
                    )
                    .await
                    .to_error_on_unique_violation(SecuritySchemeRepoError::SecuritySchemeViolatesUniqueness)?;

                let revision_record = Self::insert_revision(tx, revision).await?;

                Ok(SecuritySchemeExtRevisionRecord {
                    environment_id: security_scheme_record.environment_id,
                    name: security_scheme_record.name,
                    entity_created_at: security_scheme_record.audit.created_at,
                    revision: revision_record
                })
            }.boxed()
        }).await
    }

    async fn update(
        &self,
        revision: SecuritySchemeRevisionRecord,
    ) -> Result<SecuritySchemeExtRevisionRecord, SecuritySchemeRepoError> {
        self.db_pool.with_tx_err(METRICS_SVC_NAME, "update", |tx| {
            async move {
                let revision = Self::insert_revision(tx, revision).await?;

                let security_scheme_record: SecuritySchemeRecord = tx
                    .fetch_optional_as(
                        sqlx::query_as(indoc! {r#"
                            UPDATE security_schemes
                            SET updated_at = $1, modified_by = $2, current_revision_id = $3
                            WHERE security_scheme_id = $4
                            RETURNING security_scheme_id, environment_id, name, created_at, updated_at, deleted_at, modified_by, current_revision_id
                        "#})
                            .bind(&revision.audit.created_at)
                            .bind(revision.audit.created_by)
                            .bind(revision.revision_id)
                            .bind(revision.security_scheme_id)
                    ).await?
                    .ok_or(SecuritySchemeRepoError::ConcurrentModification)?;

                Ok(SecuritySchemeExtRevisionRecord {
                    environment_id: security_scheme_record.environment_id,
                    name: security_scheme_record.name,
                    entity_created_at: security_scheme_record.audit.created_at,
                    revision
                })
            }.boxed()
        }).await
    }

    async fn delete(
        &self,
        revision: SecuritySchemeRevisionRecord,
    ) -> Result<SecuritySchemeExtRevisionRecord, SecuritySchemeRepoError> {
        self.db_pool.with_tx_err(METRICS_SVC_NAME, "update", |tx| {
            async move {
                let revision = Self::insert_revision(tx, revision.clone()).await?;

                let security_scheme_record: SecuritySchemeRecord = tx
                    .fetch_optional_as(
                        sqlx::query_as(indoc! {r#"
                            UPDATE security_schemes
                            SET updated_at = $1, deleted_at = $1, modified_by = $2, current_revision_id = $3
                            WHERE security_scheme_id = $4
                            RETURNING security_scheme_id, environment_id, name, created_at, updated_at, deleted_at, modified_by, current_revision_id
                        "#})
                            .bind(&revision.audit.created_at)
                            .bind(revision.audit.created_by)
                            .bind(revision.revision_id)
                            .bind(revision.security_scheme_id)
                    ).await?
                    .ok_or(SecuritySchemeRepoError::ConcurrentModification)?;

                Ok(SecuritySchemeExtRevisionRecord {
                    environment_id: security_scheme_record.environment_id,
                    name: security_scheme_record.name,
                    entity_created_at: security_scheme_record.audit.created_at,
                    revision
                })
            }.boxed()
        }).await
    }

    async fn get_by_id(
        &self,
        security_scheme_id: Uuid,
    ) -> Result<Option<SecuritySchemeExtRevisionRecord>, SecuritySchemeRepoError> {
        let result: Option<SecuritySchemeExtRevisionRecord> = self.with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT ss.environment_id, ss.name, ss.created_at AS entity_created_at, ssr.security_scheme_id, ssr.revision_id, ssr.provider_type, ssr.client_id, ssr.client_secret, ssr.redirect_url, ssr.scopes, ssr.created_at, ssr.created_by, ssr.deleted
                    FROM security_schemes ss
                    JOIN security_scheme_revisions ssr ON ssr.security_scheme_id = ss.security_scheme_id AND ssr.revision_id = ss.current_revision_id
                    WHERE ss.security_scheme_id = $1 AND ss.deleted_at IS NULL
                "#})
                    .bind(security_scheme_id),
            )
            .await?;

        Ok(result)
    }

    async fn get_for_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<SecuritySchemeExtRevisionRecord>, SecuritySchemeRepoError> {
        let results: Vec<SecuritySchemeExtRevisionRecord> = self.with_ro("get_for_environment")
            .fetch_all_as(
                sqlx::query_as(indoc! {r#"
                    SELECT ss.environment_id, ss.name, ss.created_at AS entity_created_at, ssr.security_scheme_id, ssr.revision_id, ssr.provider_type, ssr.client_id, ssr.client_secret, ssr.redirect_url, ssr.scopes, ssr.created_at, ssr.created_by, ssr.deleted
                    FROM security_schemes ss
                    JOIN security_scheme_revisions ssr ON ssr.security_scheme_id = ss.security_scheme_id AND ssr.revision_id = ss.current_revision_id
                    WHERE ss.environment_id = $1 AND ss.deleted_at IS NULL
                "#})
                    .bind(environment_id),
            )
            .await?;

        Ok(results)
    }

    async fn get_for_environment_and_name(
        &self,
        environment_id: Uuid,
        name: &str,
    ) -> Result<Option<SecuritySchemeExtRevisionRecord>, SecuritySchemeRepoError> {
        let result: Option<SecuritySchemeExtRevisionRecord> = self.with_ro("get_for_environment_and_name")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT ss.environment_id, ss.name, ss.created_at AS entity_created_at, ssr.security_scheme_id, ssr.revision_id, ssr.provider_type, ssr.client_id, ssr.client_secret, ssr.redirect_url, ssr.scopes, ssr.created_at, ssr.created_by, ssr.deleted
                    FROM security_schemes ss
                    JOIN security_scheme_revisions ssr ON ssr.security_scheme_id = ss.security_scheme_id AND ssr.revision_id = ss.current_revision_id
                    WHERE ss.environment_id = $1 AND ss.name = $2 AND ss.deleted_at IS NULL
                "#})
                .bind(environment_id)
                .bind(name)
            )
            .await?;

        Ok(result)
    }
}
