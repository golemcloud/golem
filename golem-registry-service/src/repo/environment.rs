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

use super::model::environment::{
    EnvironmentRepoError, EnvironmentWithDetailsRecord, OptionalEnvironmentExtRevisionRecord,
};
use crate::repo::model::BindFields;
pub use crate::repo::model::environment::{
    EnvironmentExtRecord, EnvironmentExtRevisionRecord, EnvironmentRevisionRecord,
};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi};
use golem_service_base::repo::{BindingsStack, RepoError, ResultExt};
use indoc::{formatdoc, indoc};
use sqlx::Database;
use std::fmt::Debug;
use tap::Pipe;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait EnvironmentRepo: Send + Sync {
    async fn get_by_name(
        &self,
        application_id: Uuid,
        name: &str,
        actor: Uuid,
        override_visibility: bool,
    ) -> Result<Option<EnvironmentExtRevisionRecord>, EnvironmentRepoError>;

    async fn get_by_id(
        &self,
        environment_id: Uuid,
        actor: Uuid,
        include_deleted: bool,
        override_visibility: bool,
    ) -> Result<Option<EnvironmentExtRevisionRecord>, EnvironmentRepoError>;

    async fn list_by_app(
        &self,
        application_id: Uuid,
        actor: Uuid,
        override_visibility: bool,
    ) -> Result<Vec<OptionalEnvironmentExtRevisionRecord>, EnvironmentRepoError>;

    async fn create(
        &self,
        application_id: Uuid,
        revision: EnvironmentRevisionRecord,
    ) -> Result<EnvironmentExtRevisionRecord, EnvironmentRepoError>;

    async fn update(
        &self,
        revision: EnvironmentRevisionRecord,
    ) -> Result<EnvironmentExtRevisionRecord, EnvironmentRepoError>;

    async fn delete(
        &self,
        revision: EnvironmentRevisionRecord,
    ) -> Result<EnvironmentExtRevisionRecord, EnvironmentRepoError>;

    async fn list_visible_to_account(
        &self,
        account_id: Uuid,
        account_email: Option<&str>,
        app_name: Option<&str>,
        env_name: Option<&str>,
    ) -> Result<Vec<EnvironmentWithDetailsRecord>, EnvironmentRepoError>;
}

pub struct LoggedEnvironmentRepo<Repo: EnvironmentRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "environment repository";

impl<Repo: EnvironmentRepo> LoggedEnvironmentRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_name(application_id: Uuid, name: &str) -> Span {
        info_span!(SPAN_NAME, application_id = %application_id, name)
    }

    fn span_env(environment_id: Uuid) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id)
    }

    fn span_app_id(application_id: Uuid) -> Span {
        info_span!(SPAN_NAME, application_id = %application_id)
    }
}

#[async_trait]
impl<Repo: EnvironmentRepo> EnvironmentRepo for LoggedEnvironmentRepo<Repo> {
    async fn get_by_name(
        &self,
        application_id: Uuid,
        name: &str,
        actor: Uuid,
        override_visibility: bool,
    ) -> Result<Option<EnvironmentExtRevisionRecord>, EnvironmentRepoError> {
        self.repo
            .get_by_name(application_id, name, actor, override_visibility)
            .instrument(Self::span_name(application_id, name))
            .await
    }

    async fn get_by_id(
        &self,
        environment_id: Uuid,
        actor: Uuid,
        include_deleted: bool,
        override_visibility: bool,
    ) -> Result<Option<EnvironmentExtRevisionRecord>, EnvironmentRepoError> {
        self.repo
            .get_by_id(environment_id, actor, include_deleted, override_visibility)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn list_by_app(
        &self,
        application_id: Uuid,
        actor: Uuid,
        override_visibility: bool,
    ) -> Result<Vec<OptionalEnvironmentExtRevisionRecord>, EnvironmentRepoError> {
        self.repo
            .list_by_app(application_id, actor, override_visibility)
            .instrument(Self::span_env(application_id))
            .await
    }

    async fn create(
        &self,
        application_id: Uuid,
        revision: EnvironmentRevisionRecord,
    ) -> Result<EnvironmentExtRevisionRecord, EnvironmentRepoError> {
        self.repo
            .create(application_id, revision)
            .instrument(Self::span_app_id(application_id))
            .await
    }

    async fn update(
        &self,
        revision: EnvironmentRevisionRecord,
    ) -> Result<EnvironmentExtRevisionRecord, EnvironmentRepoError> {
        let span = Self::span_env(revision.environment_id);
        self.repo.update(revision).instrument(span).await
    }

    async fn delete(
        &self,
        revision: EnvironmentRevisionRecord,
    ) -> Result<EnvironmentExtRevisionRecord, EnvironmentRepoError> {
        let span = Self::span_env(revision.environment_id);
        self.repo.delete(revision).instrument(span).await
    }

    async fn list_visible_to_account(
        &self,
        account_id: Uuid,
        account_email: Option<&str>,
        app_name: Option<&str>,
        env_name: Option<&str>,
    ) -> Result<Vec<EnvironmentWithDetailsRecord>, EnvironmentRepoError> {
        self.repo
            .list_visible_to_account(account_id, account_email, app_name, env_name)
            .instrument(info_span!(
                SPAN_NAME,
                account_id = %account_id,
                account_email = ?account_email,
                app_name = ?app_name,
                env_name = ?env_name
            ))
            .await
    }
}

pub struct DbEnvironmentRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "environment";

impl<DBP: Pool> DbEnvironmentRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedEnvironmentRepo<Self>
    where
        Self: EnvironmentRepo,
    {
        LoggedEnvironmentRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }

    async fn with_tx_err<R, E, F>(&self, api_name: &'static str, f: F) -> Result<R, E>
    where
        R: Send,
        E: Debug + Send + From<RepoError>,
        F: for<'f> FnOnce(
                &'f mut <DBP::LabelledApi as LabelledPoolApi>::LabelledTransaction,
            ) -> BoxFuture<'f, Result<R, E>>
            + Send,
    {
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, api_name, f)
            .await
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl EnvironmentRepo for DbEnvironmentRepo<PostgresPool> {
    async fn get_by_name(
        &self,
        application_id: Uuid,
        name: &str,
        actor: Uuid,
        override_visibility: bool,
    ) -> Result<Option<EnvironmentExtRevisionRecord>, EnvironmentRepoError> {
        let result = self
            .with_ro("get_by_name")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        e.name, e.application_id,
                        r.environment_id, r.revision_id, r.hash,
                        r.created_at, r.created_by, r.deleted,
                        r.compatibility_check, r.version_check, r.security_overrides,

                        a.account_id as owner_account_id,
                        COALESCE(esr.roles, 0) AS environment_roles_from_shares,

                        cdr.revision_id as current_deployment_revision,
                        dr.revision_id as current_deployment_deployment_revision,
                        dr.version as current_deployment_deployment_version,
                        dr.hash as current_deployment_deployment_hash
                    FROM accounts a
                    JOIN applications ap
                        ON ap.account_id = a.account_id
                    JOIN environments e
                        ON e.application_id = ap.application_id
                    JOIN environment_revisions r
                        ON r.environment_id = e.environment_id
                        AND r.revision_id = e.current_revision_id

                    LEFT JOIN environment_shares es
                        ON es.environment_id = e.environment_id
                        AND es.grantee_account_id = $3
                        AND es.deleted_at IS NULL
                    LEFT JOIN environment_share_revisions esr
                        ON esr.environment_share_id = es.environment_share_id
                        AND esr.revision_id = es.current_revision_id

                    LEFT JOIN current_deployments cd
                        ON cd.environment_id = e.environment_id
                    LEFT JOIN current_deployment_revisions cdr
                        ON cdr.environment_id = cd.environment_id
                        AND cdr.revision_id = cd.current_revision_id
                    LEFT JOIN deployment_revisions dr
                        ON dr.environment_id = cdr.environment_id
                        AND dr.revision_id = cdr.deployment_revision_id

                    WHERE
                        ap.application_id = $1
                        AND e.name = $2
                        AND a.deleted_at IS NULL
                        AND ap.deleted_at IS NULL
                        AND e.deleted_at IS NULL
                        AND (
                            $4
                            OR a.account_id = $3
                            OR esr.roles IS NOT NULL
                        )
                "# })
                .bind(application_id)
                .bind(name)
                .bind(actor)
                .bind(override_visibility),
            )
            .await?;

        Ok(result)
    }

    async fn get_by_id(
        &self,
        environment_id: Uuid,
        actor: Uuid,
        include_deleted: bool,
        override_visibility: bool,
    ) -> Result<Option<EnvironmentExtRevisionRecord>, EnvironmentRepoError> {
        let result = self
            .with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        e.name, e.application_id,
                        r.environment_id, r.revision_id, r.hash,
                        r.created_at, r.created_by, r.deleted,
                        r.compatibility_check, r.version_check, r.security_overrides,

                        a.account_id as owner_account_id,
                        COALESCE(esr.roles, 0) AS environment_roles_from_shares,

                        cdr.revision_id as current_deployment_revision,
                        dr.revision_id as current_deployment_deployment_revision,
                        dr.version as current_deployment_deployment_version,
                        dr.hash as current_deployment_deployment_hash
                    FROM accounts a
                    JOIN applications ap
                        ON ap.account_id = a.account_id
                    JOIN environments e
                        ON e.application_id = ap.application_id
                    JOIN environment_revisions r
                        ON r.environment_id = e.environment_id
                        AND r.revision_id = e.current_revision_id

                    LEFT JOIN environment_shares es
                        ON es.environment_id = e.environment_id
                        AND es.grantee_account_id = $2
                        -- only join shares if the environment itself is not deleted
                        AND (
                            a.deleted_at IS NULL
                            AND ap.deleted_at IS NULL
                            AND e.deleted_at IS NULL
                        )
                        AND es.deleted_at IS NULL
                    LEFT JOIN environment_share_revisions esr
                        ON esr.environment_share_id = es.environment_share_id
                        AND esr.revision_id = es.current_revision_id

                    LEFT JOIN current_deployments cd
                        ON cd.environment_id = e.environment_id
                    LEFT JOIN current_deployment_revisions cdr
                        ON cdr.environment_id = cd.environment_id
                        AND cdr.revision_id = cd.current_revision_id
                    LEFT JOIN deployment_revisions dr
                        ON dr.environment_id = cdr.environment_id
                        AND dr.revision_id = cdr.deployment_revision_id

                    WHERE
                        e.environment_id = $1
                        AND (
                            $3
                            OR (
                                a.deleted_at IS NULL
                                AND ap.deleted_at IS NULL
                                AND e.deleted_at IS NULL
                            )
                        )
                        AND (
                            $4
                            OR a.account_id = $2
                            OR esr.roles IS NOT NULL
                        )
                "# })
                .bind(environment_id)
                .bind(actor)
                .bind(include_deleted)
                .bind(override_visibility),
            )
            .await?;

        Ok(result)
    }

    async fn list_by_app(
        &self,
        application_id: Uuid,
        actor: Uuid,
        override_visibility: bool,
    ) -> Result<Vec<OptionalEnvironmentExtRevisionRecord>, EnvironmentRepoError> {
        let result = self
            .with_ro("list_by_owner")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        e.name, e.application_id,
                        r.environment_id, r.revision_id, r.hash,
                        r.created_at, r.created_by, r.deleted,
                        r.compatibility_check, r.version_check, r.security_overrides,

                        a.account_id as owner_account_id,
                        COALESCE(esr.roles, 0) AS environment_roles_from_shares,

                        cdr.revision_id as current_deployment_revision,
                        dr.revision_id as current_deployment_deployment_revision,
                        dr.version as current_deployment_deployment_version,
                        dr.hash as current_deployment_deployment_hash
                    FROM accounts a
                    INNER JOIN applications ap
                        ON ap.account_id = a.account_id
                        AND ap.deleted_at IS NULL

                    LEFT JOIN environments e
                        ON e.application_id = ap.application_id
                        AND e.deleted_at IS NULL
                    LEFT JOIN environment_revisions r
                        ON r.environment_id = e.environment_id
                        AND r.revision_id = e.current_revision_id

                    LEFT JOIN environment_shares es
                        ON es.environment_id = e.environment_id
                        AND es.grantee_account_id = $2
                        AND es.deleted_at IS NULL
                    LEFT JOIN environment_share_revisions esr
                        ON esr.environment_share_id = es.environment_share_id
                        AND esr.revision_id = es.current_revision_id

                    LEFT JOIN current_deployments cd
                        ON cd.environment_id = e.environment_id
                    LEFT JOIN current_deployment_revisions cdr
                        ON cdr.environment_id = cd.environment_id
                        AND cdr.revision_id = cd.current_revision_id
                    LEFT JOIN deployment_revisions dr
                        ON dr.environment_id = cdr.environment_id
                        AND dr.revision_id = cdr.deployment_revision_id

                    WHERE
                        ap.application_id = $1
                        AND a.deleted_at IS NULL
                        AND (
                            $3
                            OR a.account_id = $2
                            OR esr.roles IS NOT NULL
                        )
                    ORDER BY e.name
                "#})
                .bind(application_id)
                .bind(actor)
                .bind(override_visibility),
            )
            .await?;

        Ok(result)
    }

    async fn create(
        &self,
        application_id: Uuid,
        revision: EnvironmentRevisionRecord,
    ) -> Result<EnvironmentExtRevisionRecord, EnvironmentRepoError> {
        // Note no {access,deletion}-based filtering is done here. That needs to be handled in higher layer before ever calling this function
        self.with_tx_err("create", |tx| async move {
            let environment_record: EnvironmentExtRecord = tx.fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    INSERT INTO environments
                      (environment_id, name, application_id, created_at, updated_at, deleted_at, modified_by, current_revision_id)
                    VALUES
                      ($1, $2, $3, $4, $4, NULL, $5, 0)
                    RETURNING
                      environment_id,
                      name,
                      application_id,
                      created_at,
                      updated_at,
                      deleted_at,
                      modified_by,
                      current_revision_id,

                      -- Owner account id via scalar subquery
                      (SELECT a.account_id
                       FROM applications ap
                       JOIN accounts a ON a.account_id = ap.account_id
                       WHERE ap.application_id = environments.application_id) AS owner_account_id,

                      -- Hard-coded defaults
                      0 AS environment_roles_from_shares,
                      NULL AS current_deployment_revision,
                      NULL AS current_deployment_deployment_revision,
                      NULL AS current_deployment_deployment_version,
                      NULL AS current_deployment_deployment_hash;
                "# })
                    .bind(revision.environment_id)
                    .bind(&revision.name)
                    .bind(application_id)
                    .bind(&revision.audit.created_at)
                    .bind(revision.audit.created_by)
            ).await
            .to_error_on_unique_violation(EnvironmentRepoError::EnvironmentViolatesUniqueness)?;

            let revision = Self::insert_revision(tx, revision).await?;

            Ok(EnvironmentExtRevisionRecord {
                application_id,
                revision,

                owner_account_id: environment_record.owner_account_id,
                environment_roles_from_shares: environment_record.environment_roles_from_shares,

                current_deployment_revision: environment_record.current_deployment_revision,
                current_deployment_deployment_revision: environment_record.current_deployment_deployment_revision,
                current_deployment_deployment_version: environment_record.current_deployment_deployment_version,
                current_deployment_deployment_hash: environment_record.current_deployment_deployment_hash
            })
        }.boxed()).await
    }

    async fn update(
        &self,
        revision: EnvironmentRevisionRecord,
    ) -> Result<EnvironmentExtRevisionRecord, EnvironmentRepoError> {
        self.with_tx_err("update", |tx| {
            async move {
                let revision: EnvironmentRevisionRecord = Self::insert_revision(tx, revision).await?;

                // TODO: atomic:
                //       should use something like:
                //         WITH updated AS (
                //            UPDATE..
                //            RETURNING..
                //         )
                //         SELECT -- with joins...
                //       so we avoid selecting the same tables multiple times, same goes for logical DELETE


                // Note no {access,deletion}-based filtering is done here. That needs to be handled in higher layer before ever calling this function
                let environment_record: EnvironmentExtRecord = tx.fetch_optional_as(
                    sqlx::query_as(indoc! { r#"
                        UPDATE environments
                        SET name = $1,
                            updated_at = $2,
                            modified_by = $3,
                            current_revision_id = $4
                        WHERE environment_id = $5
                        RETURNING
                            environment_id,
                            name,
                            application_id,
                            created_at,
                            updated_at,
                            deleted_at,
                            modified_by,
                            current_revision_id,

                            -- Owner account id
                            (SELECT a.account_id
                             FROM applications ap
                             JOIN accounts a ON a.account_id = ap.account_id
                             WHERE ap.application_id = environments.application_id) AS owner_account_id,

                            -- Environment roles from shares
                            COALESCE((
                              SELECT esr.roles
                              FROM environment_shares es
                              JOIN environment_share_revisions esr
                                ON esr.environment_share_id = es.environment_share_id
                               AND esr.revision_id = es.current_revision_id
                              WHERE es.environment_id = environments.environment_id
                                AND es.grantee_account_id = $3
                                AND es.deleted_at IS NULL
                            ), 0) AS environment_roles_from_shares,

                            -- Current deployment info
                            (
                                SELECT cd.current_revision_id
                                FROM current_deployments cd
                                WHERE cd.environment_id = environments.environment_id
                            ) AS current_deployment_revision,

                            (
                                SELECT cdr.deployment_revision_id
                                FROM current_deployments cd
                                JOIN current_deployment_revisions cdr
                                    ON cdr.environment_id = cd.environment_id
                                    AND cdr.revision_id = cd.current_revision_id
                                WHERE cd.environment_id = environments.environment_id
                            ) AS current_deployment_deployment_revision,

                            (
                                SELECT dr.version
                                FROM current_deployments cd
                                JOIN current_deployment_revisions cdr
                                    ON cdr.environment_id = cd.environment_id
                                    AND cdr.revision_id = cd.current_revision_id
                                JOIN deployment_revisions dr
                                    ON dr.environment_id = cdr.environment_id
                                    AND dr.revision_id = cdr.deployment_revision_id
                                WHERE cd.environment_id = environments.environment_id
                            ) AS current_deployment_deployment_version,

                            (
                                SELECT dr.hash
                                FROM current_deployments cd
                                JOIN current_deployment_revisions cdr
                                    ON cdr.environment_id = cd.environment_id
                                    AND cdr.revision_id = cd.current_revision_id
                                JOIN deployment_revisions dr
                                    ON dr.environment_id = cdr.environment_id
                                    AND dr.revision_id = cdr.deployment_revision_id
                                WHERE cd.environment_id = environments.environment_id
                            ) AS current_deployment_deployment_hash;
                    "#})
                    .bind(&revision.name)
                    .bind(&revision.audit.created_at)
                    .bind(revision.audit.created_by)
                    .bind(revision.revision_id)
                    .bind(revision.environment_id)
                )
                .await
                .to_error_on_unique_violation(EnvironmentRepoError::EnvironmentViolatesUniqueness)?
                .ok_or(EnvironmentRepoError::ConcurrentModification)?;

                Ok(EnvironmentExtRevisionRecord {
                    application_id: environment_record.application_id,
                    revision,

                    owner_account_id: environment_record.owner_account_id,
                    environment_roles_from_shares: environment_record.environment_roles_from_shares,

                    current_deployment_revision: environment_record.current_deployment_revision,
                    current_deployment_deployment_revision: environment_record.current_deployment_deployment_revision,
                    current_deployment_deployment_version: environment_record.current_deployment_deployment_version,
                    current_deployment_deployment_hash: environment_record.current_deployment_deployment_hash,
                })
            }
            .boxed()
        })
        .await
    }

    async fn delete(
        &self,
        revision: EnvironmentRevisionRecord,
    ) -> Result<EnvironmentExtRevisionRecord, EnvironmentRepoError> {
        self.with_tx_err("delete", |tx| {
            async move {
                let revision: EnvironmentRevisionRecord = Self::insert_revision(tx, revision).await?;

                let environment_record: EnvironmentExtRecord = tx.fetch_optional_as(
                    sqlx::query_as(indoc! { r#"
                        UPDATE environments
                        SET name = $1,
                            updated_at = $2,
                            deleted_at = $2,
                            modified_by = $3,
                            current_revision_id = $4
                        WHERE environment_id = $5
                        RETURNING
                            environment_id,
                            name,
                            application_id,
                            created_at,
                            updated_at,
                            deleted_at,
                            modified_by,
                            current_revision_id,

                            -- Owner account id
                            (SELECT a.account_id
                             FROM applications ap
                             JOIN accounts a ON a.account_id = ap.account_id
                             WHERE ap.application_id = environments.application_id) AS owner_account_id,

                            -- Environment roles from shares
                            COALESCE((
                              SELECT esr.roles
                              FROM environment_shares es
                              JOIN environment_share_revisions esr
                                ON esr.environment_share_id = es.environment_share_id
                               AND esr.revision_id = es.current_revision_id
                              WHERE es.environment_id = environments.environment_id
                                AND es.grantee_account_id = $3
                                AND es.deleted_at IS NULL
                            ), 0) AS environment_roles_from_shares,

                            -- Current deployment info
                            (
                                SELECT cd.current_revision_id
                                FROM current_deployments cd
                                WHERE cd.environment_id = environments.environment_id
                            ) AS current_deployment_revision,

                            (
                                SELECT cdr.deployment_revision_id
                                FROM current_deployments cd
                                JOIN current_deployment_revisions cdr
                                    ON cdr.environment_id = cd.environment_id
                                    AND cdr.revision_id = cd.current_revision_id
                                WHERE cd.environment_id = environments.environment_id
                            ) AS current_deployment_deployment_revision,

                            (
                                SELECT dr.version
                                FROM current_deployments cd
                                JOIN current_deployment_revisions cdr
                                    ON cdr.environment_id = cd.environment_id
                                    AND cdr.revision_id = cd.current_revision_id
                                JOIN deployment_revisions dr
                                    ON dr.environment_id = cdr.environment_id
                                    AND dr.revision_id = cdr.deployment_revision_id
                                WHERE cd.environment_id = environments.environment_id
                            ) AS current_deployment_deployment_version,

                            (
                                SELECT dr.hash
                                FROM current_deployments cd
                                JOIN current_deployment_revisions cdr
                                    ON cdr.environment_id = cd.environment_id
                                    AND cdr.revision_id = cd.current_revision_id
                                JOIN deployment_revisions dr
                                    ON dr.environment_id = cdr.environment_id
                                    AND dr.revision_id = cdr.deployment_revision_id
                                WHERE cd.environment_id = environments.environment_id
                            ) AS current_deployment_deployment_hash;

                    "#})
                    .bind(&revision.name)
                    .bind(&revision.audit.created_at)
                    .bind(revision.audit.created_by)
                    .bind(revision.revision_id)
                    .bind(revision.environment_id)
                )
                .await?
                .ok_or(EnvironmentRepoError::ConcurrentModification)?;

                Ok(EnvironmentExtRevisionRecord {
                    application_id: environment_record.application_id,
                    revision,

                    owner_account_id: environment_record.owner_account_id,
                    environment_roles_from_shares: environment_record.environment_roles_from_shares,

                    current_deployment_revision: environment_record.current_deployment_revision,
                    current_deployment_deployment_revision: environment_record.current_deployment_deployment_revision,
                    current_deployment_deployment_version: environment_record.current_deployment_deployment_version,
                    current_deployment_deployment_hash: environment_record.current_deployment_deployment_hash,
                })
            }
            .boxed()
        })
        .await
    }

    async fn list_visible_to_account(
        &self,
        account_id: Uuid,
        account_email: Option<&str>,
        app_name: Option<&str>,
        env_name: Option<&str>,
    ) -> Result<Vec<EnvironmentWithDetailsRecord>, EnvironmentRepoError> {
        let mut binding_stack = BindingsStack::new(2);

        let account_email_filter = if let Some(account_email) = account_email {
            let i = binding_stack.push(account_email);
            format!("AND a.email = ${i}")
        } else {
            "".to_string()
        };

        let app_name_filter = if let Some(app_name) = app_name {
            let i = binding_stack.push(app_name);
            format!("AND ap.name = ${i}")
        } else {
            "".to_string()
        };

        let env_name_filter = if let Some(env_name) = env_name {
            let i = binding_stack.push(env_name);
            format!("AND e.name = ${i}")
        } else {
            "".to_string()
        };

        let query = formatdoc! { r#"
            SELECT
                -- Environment
                e.environment_id,
                r.revision_id AS environment_revision_id,
                e.name AS environment_name,
                r.compatibility_check AS environment_compatibility_check,
                r.version_check AS environment_version_check,
                r.security_overrides AS environment_security_overrides,

                COALESCE(esr.roles, 0) AS environment_roles_from_shares,

                -- Current deployment (optional)
                cdr.revision_id AS current_deployment_revision,
                dr.revision_id AS current_deployment_deployment_revision,
                dr.version AS current_deployment_deployment_version,
                dr.hash AS current_deployment_deployment_hash,

                -- Parent application
                ap.application_id,
                ap.name AS application_name,

                -- Parent account (owner of the application)
                a.account_id,
                ar.name AS account_name,
                ar.email AS account_email

            FROM accounts a
            INNER JOIN account_revisions ar
                ON ar.account_id = a.account_id
                AND ar.revision_id = a.current_revision_id

            INNER JOIN applications ap
                ON ap.account_id = a.account_id
                AND ap.deleted_at IS NULL

            INNER JOIN environments e
                ON e.application_id = ap.application_id
                AND e.deleted_at IS NULL

            INNER JOIN environment_revisions r
                ON r.environment_id = e.environment_id
                AND r.revision_id = e.current_revision_id

            -- Environment shares
            LEFT JOIN environment_shares es
                ON es.environment_id = e.environment_id
                AND es.grantee_account_id = $1
                AND es.deleted_at IS NULL

            LEFT JOIN environment_share_revisions esr
                ON esr.environment_share_id = es.environment_share_id
                AND esr.revision_id = es.current_revision_id

            -- Current deployment
            LEFT JOIN current_deployments cd
                ON cd.environment_id = e.environment_id
            LEFT JOIN current_deployment_revisions cdr
                ON cdr.environment_id = cd.environment_id
                AND cdr.revision_id = cd.current_revision_id
            LEFT JOIN deployment_revisions dr
                ON dr.environment_id = cdr.environment_id
                AND dr.revision_id = cdr.deployment_revision_id

            WHERE
                a.deleted_at IS NULL
                AND (
                    a.account_id = $1
                    OR esr.roles IS NOT NULL
                )
                {account_email_filter}
                {app_name_filter}
                {env_name_filter}
            ORDER BY a.email, ap.name, e.name
        "#};

        let query_as = {
            let binding_stack = binding_stack;
            sqlx::query_as(&query)
                .bind(account_id)
                .pipe(|q| binding_stack.apply(q))
        };

        let result = self
            .with_ro("list_visible_to_account")
            .fetch_all_as(query_as)
            .await?;

        Ok(result)
    }
}

#[async_trait]
trait EnvironmentRepoInternal: EnvironmentRepo {
    type Db: Database;
    type Tx: LabelledPoolTransaction;

    async fn insert_revision(
        tx: &mut Self::Tx,
        revision: EnvironmentRevisionRecord,
    ) -> Result<EnvironmentRevisionRecord, EnvironmentRepoError>;
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl EnvironmentRepoInternal for DbEnvironmentRepo<PostgresPool> {
    type Db = <PostgresPool as Pool>::Db;
    type Tx = <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction;

    async fn insert_revision(
        tx: &mut Self::Tx,
        revision: EnvironmentRevisionRecord,
    ) -> Result<EnvironmentRevisionRecord, EnvironmentRepoError> {
        let revision = revision.with_updated_hash();

        let revision = tx.fetch_one_as(sqlx::query_as(indoc! { r#"
            INSERT INTO environment_revisions
            (environment_id, revision_id, name, hash, created_at, created_by, deleted, compatibility_check, version_check, security_overrides)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            RETURNING environment_id, revision_id, name, hash, created_at, created_by, deleted, compatibility_check, version_check, security_overrides
        "# })
            .bind(revision.environment_id)
            .bind(revision.revision_id)
            .bind(revision.name)
            .bind(revision.hash)
            .bind_deletable_revision_audit(revision.audit)
            .bind(revision.compatibility_check)
            .bind(revision.version_check)
            .bind(revision.security_overrides))
            .await
            .to_error_on_unique_violation(EnvironmentRepoError::ConcurrentModification)?;

        Ok(revision)
    }
}
