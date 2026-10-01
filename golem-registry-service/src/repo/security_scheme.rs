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

use super::model::security_scheme::{
    SecuritySchemeAuthExtRevisionRecord, SecuritySchemeExtRevisionRecord, SecuritySchemeRepoError,
    SecuritySchemeRevisionRecord,
};
use crate::repo::model::BindFields;
pub use crate::repo::model::account::AccountRecord;
use crate::repo::model::security_scheme::SecuritySchemeRecord;
use crate::repo::registry_change::{
    DbRegistryChangeRepo, NewRegistryChangeEvent, RequiresNotificationSignal, RequiresSignalExt,
};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use golem_service_base::repo::{RepoError, ResultExt};
use indoc::indoc;
use sqlx::Row;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait SecuritySchemeRepo: Send + Sync {
    async fn get_http_routing_epoch_for_scheme(
        &self,
        security_scheme_id: Uuid,
    ) -> Result<Option<i64>, SecuritySchemeRepoError>;

    /// Create a security scheme and record a change event in the same transaction.
    async fn create(
        &self,
        environment_id: Uuid,
        expected_http_routing_epoch: i64,
        name: String,
        revision: SecuritySchemeRevisionRecord,
    ) -> Result<RequiresNotificationSignal<SecuritySchemeExtRevisionRecord>, SecuritySchemeRepoError>;

    /// Update a security scheme and record a change event in the same transaction.
    async fn update(
        &self,
        environment_id: Uuid,
        expected_http_routing_epoch: i64,
        revision: SecuritySchemeRevisionRecord,
    ) -> Result<RequiresNotificationSignal<SecuritySchemeExtRevisionRecord>, SecuritySchemeRepoError>;

    /// Delete a security scheme and record a change event in the same transaction.
    async fn delete(
        &self,
        environment_id: Uuid,
        expected_http_routing_epoch: i64,
        revision: SecuritySchemeRevisionRecord,
    ) -> Result<RequiresNotificationSignal<SecuritySchemeExtRevisionRecord>, SecuritySchemeRepoError>;

    async fn get_by_id(
        &self,
        security_scheme_id: Uuid,
    ) -> Result<Option<SecuritySchemeAuthExtRevisionRecord>, SecuritySchemeRepoError>;

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
    async fn get_http_routing_epoch_for_scheme(
        &self,
        security_scheme_id: Uuid,
    ) -> Result<Option<i64>, SecuritySchemeRepoError> {
        self.repo
            .get_http_routing_epoch_for_scheme(security_scheme_id)
            .instrument(Self::span_security_scheme_id(security_scheme_id))
            .await
    }

    async fn create(
        &self,
        environment_id: Uuid,
        expected_http_routing_epoch: i64,
        name: String,
        revision: SecuritySchemeRevisionRecord,
    ) -> Result<RequiresNotificationSignal<SecuritySchemeExtRevisionRecord>, SecuritySchemeRepoError>
    {
        let span = Self::span_environment_id(environment_id);
        self.repo
            .create(environment_id, expected_http_routing_epoch, name, revision)
            .instrument(span)
            .await
    }

    async fn update(
        &self,
        environment_id: Uuid,
        expected_http_routing_epoch: i64,
        revision: SecuritySchemeRevisionRecord,
    ) -> Result<RequiresNotificationSignal<SecuritySchemeExtRevisionRecord>, SecuritySchemeRepoError>
    {
        let span = Self::span_security_scheme_id(revision.security_scheme_id);
        self.repo
            .update(environment_id, expected_http_routing_epoch, revision)
            .instrument(span)
            .await
    }

    async fn delete(
        &self,
        environment_id: Uuid,
        expected_http_routing_epoch: i64,
        revision: SecuritySchemeRevisionRecord,
    ) -> Result<RequiresNotificationSignal<SecuritySchemeExtRevisionRecord>, SecuritySchemeRepoError>
    {
        let span = Self::span_security_scheme_id(revision.security_scheme_id);
        self.repo
            .delete(environment_id, expected_http_routing_epoch, revision)
            .instrument(span)
            .await
    }

    async fn get_by_id(
        &self,
        security_scheme_id: Uuid,
    ) -> Result<Option<SecuritySchemeAuthExtRevisionRecord>, SecuritySchemeRepoError> {
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

    async fn with_tx_err<R, E, F>(&self, api_name: &'static str, f: F) -> Result<R, E>
    where
        R: Send,
        E: std::fmt::Debug + Send + From<golem_service_base::repo::RepoError>,
        F: for<'f> FnOnce(
                &'f mut <DBP::LabelledApi as LabelledPoolApi>::LabelledTransaction,
            ) -> futures::future::BoxFuture<'f, Result<R, E>>
            + Send,
    {
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, api_name, f)
            .await
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
impl DbSecuritySchemeRepo<PostgresPool> {
    async fn claim_http_routing_epoch(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        environment_id: Uuid,
        expected_epoch: i64,
    ) -> Result<(), SecuritySchemeRepoError> {
        let result = tx
            .execute(
                sqlx::query(indoc! { r#"
                    UPDATE environments
                    SET http_routing_mutation_epoch = http_routing_mutation_epoch + 1
                    WHERE environment_id = $1 AND http_routing_mutation_epoch = $2
                "# })
                .bind(environment_id)
                .bind(expected_epoch),
            )
            .await?;
        if result.rows_affected() == 0 {
            return Err(SecuritySchemeRepoError::ConcurrentModification);
        }
        Ok(())
    }

    async fn insert_revision(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        revision: SecuritySchemeRevisionRecord,
    ) -> Result<SecuritySchemeRevisionRecord, SecuritySchemeRepoError> {
        let revision: SecuritySchemeRevisionRecord = tx
            .fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    INSERT INTO security_scheme_revisions
                    (security_scheme_id, revision_id, provider_type, client_id, client_secret, redirect_url, scopes, custom_provider_name, custom_issuer_url, login_config, created_at, created_by, deleted)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
                    RETURNING security_scheme_id, revision_id, provider_type, client_id, client_secret, redirect_url, scopes, custom_provider_name, custom_issuer_url, login_config, created_at, created_by, deleted
                "# })
                .bind(revision.security_scheme_id)
                .bind(revision.revision_id)
                .bind(revision.provider_type)
                .bind(revision.client_id)
                .bind(revision.client_secret)
                .bind(revision.redirect_url)
                .bind(revision.scopes)
                .bind(revision.custom_provider_name)
                .bind(revision.custom_issuer_url)
                .bind(revision.login_config)
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
    async fn get_http_routing_epoch_for_scheme(
        &self,
        security_scheme_id: Uuid,
    ) -> Result<Option<i64>, SecuritySchemeRepoError> {
        let row = self
            .with_ro("get_http_routing_epoch_for_scheme")
            .fetch_optional(
                sqlx::query(indoc! { r#"
                    SELECT e.http_routing_mutation_epoch
                    FROM security_schemes s
                    JOIN environments e ON e.environment_id = s.environment_id
                    WHERE s.security_scheme_id = $1
                "# })
                .bind(security_scheme_id),
            )
            .await?;
        row.map(|row| {
            row.try_get("http_routing_mutation_epoch")
                .map_err(RepoError::from)
                .map_err(Into::into)
        })
        .transpose()
    }

    async fn create(
        &self,
        environment_id: Uuid,
        expected_http_routing_epoch: i64,
        name: String,
        revision: SecuritySchemeRevisionRecord,
    ) -> Result<RequiresNotificationSignal<SecuritySchemeExtRevisionRecord>, SecuritySchemeRepoError>
    {
        let result = self
            .with_tx_err("create", |tx| {
                async move {
                    Self::claim_http_routing_epoch(
                        tx,
                        environment_id,
                        expected_http_routing_epoch,
                    )
                    .await?;

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

                    let change_event =
                        NewRegistryChangeEvent::security_scheme_changed(environment_id);
                    DbRegistryChangeRepo::<PostgresPool>::create_change_event_in_tx(
                        tx,
                        &change_event,
                    )
                    .await?;

                    Ok::<_, SecuritySchemeRepoError>(SecuritySchemeExtRevisionRecord {
                        environment_id: security_scheme_record.environment_id,
                        name: security_scheme_record.name,
                        entity_created_at: security_scheme_record.audit.created_at,
                        revision: revision_record,
                    })
                }
                .boxed()
            })
            .await?;

        Ok(result.requires_notification_signal())
    }

    async fn update(
        &self,
        environment_id: Uuid,
        expected_http_routing_epoch: i64,
        revision: SecuritySchemeRevisionRecord,
    ) -> Result<RequiresNotificationSignal<SecuritySchemeExtRevisionRecord>, SecuritySchemeRepoError>
    {
        let result = self
            .with_tx_err("update", |tx| {
                async move {
                    Self::claim_http_routing_epoch(
                        tx,
                        environment_id,
                        expected_http_routing_epoch,
                    )
                    .await?;

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

                    let change_event =
                        NewRegistryChangeEvent::security_scheme_changed(environment_id);
                    DbRegistryChangeRepo::<PostgresPool>::create_change_event_in_tx(
                        tx,
                        &change_event,
                    )
                    .await?;

                    Ok::<_, SecuritySchemeRepoError>(SecuritySchemeExtRevisionRecord {
                        environment_id: security_scheme_record.environment_id,
                        name: security_scheme_record.name,
                        entity_created_at: security_scheme_record.audit.created_at,
                        revision,
                    })
                }
                .boxed()
            })
            .await?;

        Ok(result.requires_notification_signal())
    }

    async fn delete(
        &self,
        environment_id: Uuid,
        expected_http_routing_epoch: i64,
        revision: SecuritySchemeRevisionRecord,
    ) -> Result<RequiresNotificationSignal<SecuritySchemeExtRevisionRecord>, SecuritySchemeRepoError>
    {
        let result = self
            .with_tx_err("delete", |tx| {
                async move {
                    Self::claim_http_routing_epoch(
                        tx,
                        environment_id,
                        expected_http_routing_epoch,
                    )
                    .await?;

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

                    let change_event =
                        NewRegistryChangeEvent::security_scheme_changed(environment_id);
                    DbRegistryChangeRepo::<PostgresPool>::create_change_event_in_tx(
                        tx,
                        &change_event,
                    )
                    .await?;

                    Ok::<_, SecuritySchemeRepoError>(SecuritySchemeExtRevisionRecord {
                        environment_id: security_scheme_record.environment_id,
                        name: security_scheme_record.name,
                        entity_created_at: security_scheme_record.audit.created_at,
                        revision,
                    })
                }
                .boxed()
            })
            .await?;

        Ok(result.requires_notification_signal())
    }

    async fn get_by_id(
        &self,
        security_scheme_id: Uuid,
    ) -> Result<Option<SecuritySchemeAuthExtRevisionRecord>, SecuritySchemeRepoError> {
        let result: Option<SecuritySchemeAuthExtRevisionRecord> = self.with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT ss.environment_id, ss.name, ss.created_at AS entity_created_at, ssr.security_scheme_id, ssr.revision_id, ssr.provider_type, ssr.client_id, ssr.client_secret, ssr.redirect_url, ssr.scopes, ssr.custom_provider_name, ssr.custom_issuer_url, ssr.login_config, ssr.created_at, ssr.created_by, ssr.deleted,
                        er.name AS environment_name,
                        ap.name AS application_name,
                        a.email AS owner_account_email
                    FROM security_schemes ss
                    JOIN environments e ON e.environment_id = ss.environment_id
                    JOIN environment_revisions er
                        ON er.environment_id = e.environment_id
                        AND er.revision_id = e.current_revision_id
                    JOIN applications ap ON ap.application_id = e.application_id
                    JOIN accounts a ON a.account_id = ap.account_id
                    JOIN security_scheme_revisions ssr ON ssr.security_scheme_id = ss.security_scheme_id AND ssr.revision_id = ss.current_revision_id
                    WHERE ss.security_scheme_id = $1
                        AND ss.deleted_at IS NULL
                        AND e.deleted_at IS NULL
                        AND ap.deleted_at IS NULL
                        AND a.deleted_at IS NULL
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
                    SELECT ss.environment_id, ss.name, ss.created_at AS entity_created_at, ssr.security_scheme_id, ssr.revision_id, ssr.provider_type, ssr.client_id, ssr.client_secret, ssr.redirect_url, ssr.scopes, ssr.custom_provider_name, ssr.custom_issuer_url, ssr.login_config, ssr.created_at, ssr.created_by, ssr.deleted
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
                    SELECT ss.environment_id, ss.name, ss.created_at AS entity_created_at, ssr.security_scheme_id, ssr.revision_id, ssr.provider_type, ssr.client_id, ssr.client_secret, ssr.redirect_url, ssr.scopes, ssr.custom_provider_name, ssr.custom_issuer_url, ssr.login_config, ssr.created_at, ssr.created_by, ssr.deleted
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
