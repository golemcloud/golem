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

use super::model::BindFields;
use super::model::deployment::{
    CurrentDeploymentExtRevisionRecord, DeploymentCompiledHttpApiRouteWithSecuritySchemeRecord,
    DeploymentRevisionCreationRecord,
};
use super::model::deployment::{
    DeploymentCompiledHttpApiDefinitionRouteRecord, DeploymentComponentRevisionRecord,
    DeploymentDomainHttpApiDefinitionRecord, DeploymentHttpApiDefinitionRevisionRecord,
    DeploymentHttpApiDeploymentRevisionRecord,
};
use crate::repo::model::audit::RevisionAuditFields;
use crate::repo::model::component::ComponentRevisionIdentityRecord;
use crate::repo::model::deployment::{
    CurrentDeploymentRevisionRecord, DeployRepoError, DeployedDeploymentIdentity,
    DeploymentIdentity, DeploymentRevisionRecord,
};
use crate::repo::model::hash::SqlBlake3Hash;
use crate::repo::model::http_api_definition::HttpApiDefinitionRevisionIdentityRecord;
use crate::repo::model::http_api_deployment::HttpApiDeploymentRevisionIdentityRecord;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi};
use golem_service_base::repo::{RepoError, RepoResult, ResultExt};
use indoc::indoc;
use sqlx::{Database, Row};
use std::fmt::Debug;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait DeploymentRepo: Send + Sync {
    async fn get_next_revision_number(&self, environment_id: &Uuid) -> RepoResult<Option<i64>>;

    async fn get_deployed_revision(
        &self,
        environment_id: &Uuid,
        revision_id: i64,
    ) -> RepoResult<Option<DeploymentRevisionRecord>>;

    async fn get_currently_deployed_revision(
        &self,
        environment_id: &Uuid,
    ) -> RepoResult<Option<DeploymentRevisionRecord>>;

    async fn get_latest_revision(
        &self,
        environment_id: &Uuid,
    ) -> RepoResult<Option<DeploymentRevisionRecord>>;

    async fn list_deployment_revisions(
        &self,
        environment_id: &Uuid,
    ) -> RepoResult<Vec<DeploymentRevisionRecord>>;

    async fn list_deployment_history(
        &self,
        environment_id: &Uuid,
    ) -> RepoResult<Vec<CurrentDeploymentRevisionRecord>>;

    async fn get_staged_identity(&self, environment_id: &Uuid) -> RepoResult<DeploymentIdentity>;

    async fn get_deployment_identity(
        &self,
        environment_id: &Uuid,
        revision_id: Option<i64>,
    ) -> RepoResult<Option<DeployedDeploymentIdentity>>;

    async fn deploy(
        &self,
        user_account_id: &Uuid,
        deployment_creation: DeploymentRevisionCreationRecord,
        version_check: bool,
    ) -> Result<CurrentDeploymentExtRevisionRecord, DeployRepoError>;

    async fn list_active_compiled_http_api_routes(
        &self,
        domain: &str,
    ) -> RepoResult<Vec<DeploymentCompiledHttpApiRouteWithSecuritySchemeRecord>>;
}

pub struct LoggedDeploymentRepo<Repo: DeploymentRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "deployment repository";

impl<Repo: DeploymentRepo> LoggedDeploymentRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_env(environment_id: &Uuid) -> Span {
        info_span!(
            SPAN_NAME,
            environment_id = %environment_id,
        )
    }

    fn span_env_and_revision(environment_id: &Uuid, revision_id: i64) -> Span {
        info_span!(
            SPAN_NAME,
            environment_id = %environment_id,
            revision_id
        )
    }

    fn span_user_and_env(user_account_id: &Uuid, environment_id: &Uuid) -> Span {
        info_span!(
            SPAN_NAME,
            user_account_id = %user_account_id,
            environment_id = %environment_id,
        )
    }

    fn span_domain(domain: &str) -> Span {
        info_span!(
            SPAN_NAME,
            domain = %domain,
        )
    }
}

#[async_trait]
impl<Repo: DeploymentRepo> DeploymentRepo for LoggedDeploymentRepo<Repo> {
    async fn get_next_revision_number(&self, environment_id: &Uuid) -> RepoResult<Option<i64>> {
        self.repo
            .get_next_revision_number(environment_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn get_deployed_revision(
        &self,
        environment_id: &Uuid,
        revision_id: i64,
    ) -> RepoResult<Option<DeploymentRevisionRecord>> {
        self.repo
            .get_deployed_revision(environment_id, revision_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn get_currently_deployed_revision(
        &self,
        environment_id: &Uuid,
    ) -> RepoResult<Option<DeploymentRevisionRecord>> {
        self.repo
            .get_currently_deployed_revision(environment_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn get_latest_revision(
        &self,
        environment_id: &Uuid,
    ) -> RepoResult<Option<DeploymentRevisionRecord>> {
        self.repo
            .get_latest_revision(environment_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn list_deployment_revisions(
        &self,
        environment_id: &Uuid,
    ) -> RepoResult<Vec<DeploymentRevisionRecord>> {
        self.repo
            .list_deployment_revisions(environment_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn list_deployment_history(
        &self,
        environment_id: &Uuid,
    ) -> RepoResult<Vec<CurrentDeploymentRevisionRecord>> {
        self.repo
            .list_deployment_history(environment_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn get_staged_identity(&self, environment_id: &Uuid) -> RepoResult<DeploymentIdentity> {
        self.repo
            .get_staged_identity(environment_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn get_deployment_identity(
        &self,
        environment_id: &Uuid,
        revision_id: Option<i64>,
    ) -> RepoResult<Option<DeployedDeploymentIdentity>> {
        self.repo
            .get_deployment_identity(environment_id, revision_id)
            .instrument(match revision_id {
                Some(revision_id) => Self::span_env_and_revision(environment_id, revision_id),
                None => Self::span_env(environment_id),
            })
            .await
    }

    async fn deploy(
        &self,
        user_account_id: &Uuid,
        deployment_creation: DeploymentRevisionCreationRecord,
        version_check: bool,
    ) -> Result<CurrentDeploymentExtRevisionRecord, DeployRepoError> {
        let span = Self::span_user_and_env(user_account_id, &deployment_creation.environment_id);
        self.repo
            .deploy(user_account_id, deployment_creation, version_check)
            .instrument(span)
            .await
    }

    async fn list_active_compiled_http_api_routes(
        &self,
        domain: &str,
    ) -> RepoResult<Vec<DeploymentCompiledHttpApiRouteWithSecuritySchemeRecord>> {
        self.repo
            .list_active_compiled_http_api_routes(domain)
            .instrument(Self::span_domain(domain))
            .await
    }
}

pub struct DbDeploymentRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "deployment";

impl<DBP: Pool> DbDeploymentRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedDeploymentRepo<Self>
    where
        Self: DeploymentRepo,
    {
        LoggedDeploymentRepo::new(Self::new(db_pool))
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
impl DeploymentRepo for DbDeploymentRepo<PostgresPool> {
    async fn get_next_revision_number(&self, environment_id: &Uuid) -> RepoResult<Option<i64>> {
        let current_staged_revision_id_row = self
            .with_ro("deploy - get current staged revision")
            .fetch_optional(
                sqlx::query(indoc! { r#"
                    SELECT revision_id FROM deployment_revisions
                    WHERE environment_id = $1
                    ORDER BY revision_id DESC
                    LIMIT 1
                "#})
                .bind(environment_id),
            )
            .await?;

        let current_staged_revision_id = match current_staged_revision_id_row {
            Some(row) => Some(row.try_get("revision_id").map_err(RepoError::from)?),
            None => None,
        };

        Ok(current_staged_revision_id)
    }

    async fn get_deployed_revision(
        &self,
        environment_id: &Uuid,
        revision_id: i64,
    ) -> RepoResult<Option<DeploymentRevisionRecord>> {
        self.with_ro("get_deployed_revision").fetch_optional_as(
            sqlx::query_as(indoc! { r#"
                SELECT dr.environment_id, dr.revision_id, dr.version, dr.hash, dr.created_at, dr.created_by
                FROM deployment_revisions dr
                WHERE dr.environment_id = $1 AND dr.revision_id = $2
            "#})
                .bind(environment_id)
                .bind(revision_id),
        ).await
    }

    async fn get_currently_deployed_revision(
        &self,
        environment_id: &Uuid,
    ) -> RepoResult<Option<DeploymentRevisionRecord>> {
        self.with_ro("get_currently_deployed_revision").fetch_optional_as(
            sqlx::query_as(indoc! { r#"
                SELECT dr.environment_id, dr.revision_id, dr.version, dr.hash, dr.created_at, dr.created_by
                FROM current_deployments cd
                JOIN deployment_revisions dr
                    ON dr.environment_id = cd.environment_id AND dr.revision_id = cd.current_revision_id
                WHERE cd.environment_id = $1
            "#})
                .bind(environment_id),
        ).await
    }

    async fn get_latest_revision(
        &self,
        environment_id: &Uuid,
    ) -> RepoResult<Option<DeploymentRevisionRecord>> {
        self.with_ro("get_latest_revision").fetch_optional_as(
            sqlx::query_as(indoc! { r#"
                SELECT dr.environment_id, dr.revision_id, dr.version, dr.hash, dr.created_at, dr.created_by
                FROM deployment_revisions dr
                WHERE dr.environment_id = $1
                ORDER BY dr.revision_id DESC LIMIT 1
            "#})
                .bind(environment_id),
        ).await
    }

    async fn list_deployment_revisions(
        &self,
        environment_id: &Uuid,
    ) -> RepoResult<Vec<DeploymentRevisionRecord>> {
        self.with_ro("list_deployment_revisions")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                SELECT environment_id, revision_id, version, hash, created_at, created_by
                FROM deployment_revisions
                WHERE environment_id = $1
                ORDER BY revision_id
            "#})
                .bind(environment_id),
            )
            .await
    }

    async fn list_deployment_history(
        &self,
        environment_id: &Uuid,
    ) -> RepoResult<Vec<CurrentDeploymentRevisionRecord>> {
        self.with_ro("list_deployment_history")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                SELECT environment_id, revision_id, created_at, created_by, deployment_revision_id
                FROM current_deployment_revisions
                WHERE environment_id = $1
                ORDER BY revision_id
            "#})
                .bind(environment_id),
            )
            .await
    }

    async fn get_staged_identity(&self, environment_id: &Uuid) -> RepoResult<DeploymentIdentity> {
        // TODO: maybe add helper for readonly tx helpers OR create common abstraction on top
        //      of transactions and pool, so both cna be used for selects
        let mut tx = self.with_ro("get_staged_identity").begin().await?;
        match Self::get_staged_deployment(&mut tx, environment_id).await {
            Ok(result) => {
                tx.commit().await?;
                Ok(result)
            }
            Err(err) => {
                let _ = tx.rollback().await;
                Err(err)
            }
        }
    }

    async fn get_deployment_identity(
        &self,
        environment_id: &Uuid,
        revision_id: Option<i64>,
    ) -> RepoResult<Option<DeployedDeploymentIdentity>> {
        let deployment_revision = match revision_id {
            Some(revision_id) => {
                self.get_deployment_revision(environment_id, revision_id)
                    .await?
            }
            None => self.get_currently_deployed_revision(environment_id).await?,
        };

        let Some(deployment_revision) = deployment_revision else {
            return Ok(None);
        };

        let revision_id = deployment_revision.revision_id;
        Ok(Some(DeployedDeploymentIdentity {
            deployment_revision,
            identity: DeploymentIdentity {
                components: self
                    .get_deployed_components(environment_id, revision_id)
                    .await?,
                http_api_definitions: self
                    .get_deployed_http_api_definitions(environment_id, revision_id)
                    .await?,
                http_api_deployments: self
                    .get_deployed_http_api_deployments(environment_id, revision_id)
                    .await?,
            },
        }))
    }

    async fn deploy(
        &self,
        user_account_id: &Uuid,
        deployment_creation: DeploymentRevisionCreationRecord,
        version_check: bool,
    ) -> Result<CurrentDeploymentExtRevisionRecord, DeployRepoError> {
        if version_check
            && self
                .version_exists(
                    &deployment_creation.environment_id,
                    &deployment_creation.version,
                )
                .await?
        {
            return Err(DeployRepoError::VersionAlreadyExists {
                version: deployment_creation.version,
            });
        }

        let user_account_id = *user_account_id;

        self.with_tx_err("deploy", |tx| {
            async move {
                let environment_id = &deployment_creation.environment_id;
                let deployment_revision_id = deployment_creation.deployment_revision_id;

                let deployment_revision = Self::create_deployment_revision(
                    tx,
                    user_account_id,
                    *environment_id,
                    deployment_creation.deployment_revision_id,
                    deployment_creation.version,
                    deployment_creation.hash,
                )
                .await?;

                for component in &deployment_creation.components {
                    Self::create_deployment_component_revision(
                        tx,
                        environment_id,
                        deployment_revision_id,
                        component,
                    )
                    .await?
                }

                for definition in &deployment_creation.http_api_definitions {
                    Self::create_deployment_http_api_definition_revision(tx, definition).await?
                }

                for deployment in &deployment_creation.http_api_deployments {
                    Self::create_deployment_http_api_deployment_revision(tx, deployment).await?
                }

                for domain_http_api_definition in &deployment_creation.domain_http_api_definitions {
                    Self::create_deployment_domain_http_api_definition(
                        tx,
                        domain_http_api_definition,
                    )
                    .await?
                }

                for compiled_route in &deployment_creation.compiled_http_api_definition_routes {
                    Self::create_deployment_compiled_http_api_definition_route(tx, compiled_route)
                        .await?
                }

                let revision = Self::set_current_deployment(
                    tx,
                    &user_account_id,
                    &deployment_revision.environment_id,
                    deployment_revision.revision_id,
                    &deployment_revision.hash,
                    &deployment_revision.version,
                )
                .await?;

                Ok(revision)
            }
            .boxed()
        })
        .await
    }

    async fn list_active_compiled_http_api_routes(
        &self,
        domain: &str,
    ) -> RepoResult<Vec<DeploymentCompiledHttpApiRouteWithSecuritySchemeRecord>> {
        self.with_ro("list_active_compiled_http_api_routes")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        ac.account_id,
                        e.environment_id,
                        d.deployment_revision_id,
                        d.domain,
                        s.security_scheme_id,
                        sr.provider_type AS security_scheme_provider_type,
                        sr.client_id AS security_scheme_client_id,
                        sr.client_secret AS security_scheme_client_secret,
                        sr.redirect_url AS security_scheme_redirect_url,
                        sr.scopes AS security_scheme_scopes,
                        r.compiled_route
                    FROM deployment_domain_http_api_definitions d

                    JOIN deployment_compiled_http_api_definition_routes r
                      ON r.environment_id = d.environment_id
                     AND r.deployment_revision_id = d.deployment_revision_id
                     AND r.http_api_definition_id = d.http_api_definition_id

                    -- active deployment
                    JOIN current_deployment_revisions cdr
                      ON d.environment_id = cdr.environment_id
                     AND d.deployment_revision_id = cdr.deployment_revision_id

                    -- parent objects not deleted
                    JOIN environments e
                      ON d.environment_id = e.environment_id
                     AND e.deleted_at IS NULL
                    JOIN applications a
                      ON e.application_id = a.application_id
                     AND a.deleted_at IS NULL
                    JOIN accounts ac
                      ON a.account_id = ac.account_id
                     AND ac.deleted_at IS NULL

                    -- registered domains
                    JOIN domain_registrations dr
                      ON d.environment_id = dr.environment_id
                     AND d.domain = dr.domain
                     AND dr.deleted_at IS NULL

                    -- route-level optional security scheme
                    LEFT JOIN security_schemes s
                      ON r.environment_id = s.environment_id
                     AND r.security_scheme = s.name
                     AND s.deleted_at IS NULL

                    LEFT JOIN security_scheme_revisions sr
                      ON sr.security_scheme_id = s.security_scheme_id
                     AND sr.revision_id = s.current_revision_id

                    WHERE d.domain = $1
                      AND (r.security_scheme IS NULL OR s.security_scheme_id IS NOT NULL);
                "#})
                .bind(domain),
            )
            .await
    }
}

#[async_trait]
trait DeploymentRepoInternal: DeploymentRepo {
    type Db: Database;
    type Tx: LabelledPoolTransaction;

    async fn get_deployment_revision(
        &self,
        environment_id: &Uuid,
        revision_id: i64,
    ) -> RepoResult<Option<DeploymentRevisionRecord>>;

    async fn create_deployment_revision(
        tx: &mut Self::Tx,
        user_account_id: Uuid,
        environment_id: Uuid,
        revision_id: i64,
        version: String,
        hash: SqlBlake3Hash,
    ) -> Result<DeploymentRevisionRecord, DeployRepoError>;

    async fn get_staged_deployment(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
    ) -> RepoResult<DeploymentIdentity>;

    async fn get_staged_components(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
    ) -> RepoResult<Vec<ComponentRevisionIdentityRecord>>;

    async fn get_staged_http_api_definitions(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
    ) -> RepoResult<Vec<HttpApiDefinitionRevisionIdentityRecord>>;

    async fn get_staged_http_api_deployments(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
    ) -> RepoResult<Vec<HttpApiDeploymentRevisionIdentityRecord>>;

    async fn create_deployment_component_revision(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
        deployment_revision_id: i64,
        component: &DeploymentComponentRevisionRecord,
    ) -> RepoResult<()>;

    async fn create_deployment_http_api_definition_revision(
        tx: &mut Self::Tx,
        http_api_definition: &DeploymentHttpApiDefinitionRevisionRecord,
    ) -> RepoResult<()>;

    async fn create_deployment_http_api_deployment_revision(
        tx: &mut Self::Tx,
        http_api_deployment: &DeploymentHttpApiDeploymentRevisionRecord,
    ) -> RepoResult<()>;

    async fn create_deployment_domain_http_api_definition(
        tx: &mut Self::Tx,
        domain_http_api_defintion: &DeploymentDomainHttpApiDefinitionRecord,
    ) -> RepoResult<()>;

    async fn create_deployment_compiled_http_api_definition_route(
        tx: &mut Self::Tx,
        compiled_route: &DeploymentCompiledHttpApiDefinitionRouteRecord,
    ) -> RepoResult<()>;

    async fn set_current_deployment(
        tx: &mut Self::Tx,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        deployment_revision_id: i64,
        deployment_hash: &SqlBlake3Hash,
        deployment_version: &str,
    ) -> RepoResult<CurrentDeploymentExtRevisionRecord>;

    async fn get_deployed_components(
        &self,
        environment_id: &Uuid,
        revision_id: i64,
    ) -> RepoResult<Vec<ComponentRevisionIdentityRecord>>;

    async fn get_deployed_http_api_definitions(
        &self,
        environment_id: &Uuid,
        revision_id: i64,
    ) -> RepoResult<Vec<HttpApiDefinitionRevisionIdentityRecord>>;

    async fn get_deployed_http_api_deployments(
        &self,
        environment_id: &Uuid,
        revision_id: i64,
    ) -> RepoResult<Vec<HttpApiDeploymentRevisionIdentityRecord>>;

    async fn version_exists(&self, environment_id: &Uuid, version: &str) -> RepoResult<bool>;
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl DeploymentRepoInternal for DbDeploymentRepo<PostgresPool> {
    type Db = <PostgresPool as Pool>::Db;
    type Tx = <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction;

    async fn get_deployment_revision(
        &self,
        environment_id: &Uuid,
        revision_id: i64,
    ) -> RepoResult<Option<DeploymentRevisionRecord>> {
        self.with_ro("get_deployment_revision")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT environment_id, revision_id, version, hash, created_at, created_by
                    FROM deployment_revisions
                    WHERE environment_id = $1 AND revision_id = $2
                "#})
                .bind(environment_id)
                .bind(revision_id),
            )
            .await
    }

    async fn create_deployment_revision(
        tx: &mut Self::Tx,
        user_account_id: Uuid,
        environment_id: Uuid,
        revision_id: i64,
        version: String,
        hash: SqlBlake3Hash,
    ) -> Result<DeploymentRevisionRecord, DeployRepoError> {
        let revision = DeploymentRevisionRecord {
            environment_id,
            revision_id,
            version,
            hash,
            audit: RevisionAuditFields::new(user_account_id),
        };

        tx.fetch_one_as(
            sqlx::query_as(indoc! { r#"
                INSERT INTO deployment_revisions
                    (environment_id, revision_id, version, hash, created_at, created_by)
                VALUES ($1, $2, $3, $4, $5, $6)
                RETURNING environment_id, revision_id, version, hash, created_at, created_by
            "#})
            .bind(revision.environment_id)
            .bind(revision.revision_id)
            .bind(revision.version)
            .bind(revision.hash)
            .bind_revision_audit(revision.audit),
        )
        .await
        .to_error_on_unique_violation(DeployRepoError::ConcurrentModification)
    }

    async fn get_staged_deployment(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
    ) -> RepoResult<DeploymentIdentity> {
        Ok(DeploymentIdentity {
            components: Self::get_staged_components(tx, environment_id).await?,
            http_api_definitions: Self::get_staged_http_api_definitions(tx, environment_id).await?,
            http_api_deployments: Self::get_staged_http_api_deployments(tx, environment_id).await?,
        })
    }

    async fn get_staged_components(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
    ) -> RepoResult<Vec<ComponentRevisionIdentityRecord>> {
        tx.fetch_all_as(
            sqlx::query_as(indoc! { r#"
                SELECT c.component_id, c.name, cr.revision_id, cr.version, cr.hash
                FROM components c
                JOIN component_revisions cr
                    ON cr.component_id = c.component_id
                    AND cr.revision_id = c.current_revision_id
                WHERE c.environment_id = $1 AND c.deleted_at IS NULL
            "#})
            .bind(environment_id),
        )
        .await
    }

    async fn get_staged_http_api_definitions(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
    ) -> RepoResult<Vec<HttpApiDefinitionRevisionIdentityRecord>> {
        tx.fetch_all_as(
            sqlx::query_as(indoc! { r#"
                SELECT d.http_api_definition_id, d.name, dr.revision_id, dr.version, dr.hash
                FROM http_api_definitions d
                JOIN http_api_definition_revisions dr
                    ON d.http_api_definition_id = dr.http_api_definition_id
                    AND d.current_revision_id = dr.revision_id
                WHERE d.environment_id = $1 AND d.deleted_at IS NULL
            "#})
            .bind(environment_id),
        )
        .await
    }

    async fn get_staged_http_api_deployments(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
    ) -> RepoResult<Vec<HttpApiDeploymentRevisionIdentityRecord>> {
        tx.fetch_all_as(
            sqlx::query_as(indoc! { r#"
                    SELECT d.http_api_deployment_id, d.domain, dr.revision_id, dr.hash
                    FROM http_api_deployments d
                    JOIN http_api_deployment_revisions dr
                        ON d.http_api_deployment_id = dr.http_api_deployment_id
                        AND d.current_revision_id = dr.revision_id
                    WHERE d.environment_id = $1 AND d.deleted_at IS NULL
                "#})
            .bind(environment_id),
        )
        .await
    }

    async fn create_deployment_component_revision(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
        deployment_revision_id: i64,
        component: &DeploymentComponentRevisionRecord,
    ) -> RepoResult<()> {
        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO deployment_component_revisions
                    (environment_id, deployment_revision_id, component_id, component_revision_id)
                VALUES ($1, $2, $3, $4)
            "#})
            .bind(environment_id)
            .bind(deployment_revision_id)
            .bind(component.component_id)
            .bind(component.component_revision_id),
        )
        .await
        .map(|_| ())
    }

    async fn create_deployment_http_api_definition_revision(
        tx: &mut Self::Tx,
        http_api_definition: &DeploymentHttpApiDefinitionRevisionRecord,
    ) -> RepoResult<()> {
        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO deployment_http_api_definition_revisions
                    (environment_id, deployment_revision_id, http_api_definition_id, http_api_definition_revision_id)
                VALUES ($1, $2, $3, $4)
            "#})
                .bind(http_api_definition.environment_id)
                .bind(http_api_definition.deployment_revision_id)
                .bind(http_api_definition.http_api_definition_id)
                .bind(http_api_definition.http_api_definition_revision_id),
        )
            .await
            .map(|_| ())
    }

    async fn create_deployment_http_api_deployment_revision(
        tx: &mut Self::Tx,
        http_api_deployment: &DeploymentHttpApiDeploymentRevisionRecord,
    ) -> RepoResult<()> {
        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO deployment_http_api_deployment_revisions
                    (environment_id, deployment_revision_id, http_api_deployment_id, http_api_deployment_revision_id)
                VALUES ($1, $2, $3, $4)
            "#})
                .bind(http_api_deployment.environment_id)
                .bind(http_api_deployment.deployment_revision_id)
                .bind(http_api_deployment.http_api_deployment_id)
                .bind(http_api_deployment.http_api_deployment_revision_id)
        )
            .await?;

        Ok(())
    }

    async fn create_deployment_domain_http_api_definition(
        tx: &mut Self::Tx,
        domain_http_api_defintion: &DeploymentDomainHttpApiDefinitionRecord,
    ) -> RepoResult<()> {
        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO deployment_domain_http_api_definitions
                    (environment_id, deployment_revision_id, domain, http_api_definition_id)
                VALUES ($1, $2, $3, $4)
            "#})
            .bind(domain_http_api_defintion.environment_id)
            .bind(domain_http_api_defintion.deployment_revision_id)
            .bind(&domain_http_api_defintion.domain)
            .bind(domain_http_api_defintion.http_api_definition_id),
        )
        .await?;

        Ok(())
    }

    async fn create_deployment_compiled_http_api_definition_route(
        tx: &mut Self::Tx,
        compiled_route: &DeploymentCompiledHttpApiDefinitionRouteRecord,
    ) -> RepoResult<()> {
        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO deployment_compiled_http_api_definition_routes
                    (environment_id, deployment_revision_id, http_api_definition_id, id, security_scheme, compiled_route)
                VALUES ($1, $2, $3, $4, $5, $6)
            "#})
                .bind(compiled_route.environment_id)
                .bind(compiled_route.deployment_revision_id)
                .bind(compiled_route.http_api_definition_id)
                .bind(compiled_route.id)
                .bind(&compiled_route.security_scheme)
                .bind(&compiled_route.compiled_route)
        )
            .await?;

        Ok(())
    }

    async fn set_current_deployment(
        tx: &mut Self::Tx,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        deployment_revision_id: i64,
        deployment_hash: &SqlBlake3Hash,
        deployment_version: &str,
    ) -> RepoResult<CurrentDeploymentExtRevisionRecord> {
        let opt_row = tx
            .fetch_optional(
                sqlx::query(indoc! { r#"
                    SELECT revision_id FROM current_deployment_revisions
                    WHERE environment_id = $1
                    ORDER BY revision_id DESC
                    LIMIT 1
                "#})
                .bind(environment_id),
            )
            .await?;

        let revision_id: i64 = match opt_row {
            Some(row) => row.try_get::<i64, _>(0)? + 1,
            None => 0,
        };

        let revision: CurrentDeploymentExtRevisionRecord = tx
            .fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    INSERT INTO current_deployment_revisions
                    (environment_id, revision_id, created_at, created_by, deployment_revision_id)
                    VALUES ($1, $2, $3, $4, $5)
                    RETURNING
                        environment_id, revision_id, created_at, created_by,
                        deployment_revision_id,

                        $6 as deployment_hash,
                        $7 as deployment_version
                "#})
                .bind(environment_id)
                .bind(revision_id)
                .bind_revision_audit(RevisionAuditFields::new(*user_account_id))
                .bind(deployment_revision_id)
                .bind(deployment_hash)
                .bind(deployment_version),
            )
            .await?;

        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO current_deployments (environment_id, current_revision_id)
                VALUES ($1, $2)
                ON CONFLICT (environment_id, current_revision_id)
                DO UPDATE SET current_revision_id = excluded.current_revision_id
            "#})
            .bind(environment_id)
            .bind(deployment_revision_id),
        )
        .await?;

        Ok(revision)
    }

    async fn get_deployed_components(
        &self,
        environment_id: &Uuid,
        revision_id: i64,
    ) -> RepoResult<Vec<ComponentRevisionIdentityRecord>> {
        self.with_ro("get_deployed_components")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT c.component_id, c.name, cr.revision_id, cr.version, cr.hash
                    FROM components c
                    JOIN component_revisions cr ON c.component_id = cr.component_id
                    JOIN deployment_component_revisions dcr
                        ON dcr.component_id = c.component_id AND dcr.component_revision_id = cr.revision_id
                    WHERE dcr.environment_id = $1 AND dcr.deployment_revision_id = $2
                    ORDER BY c.name
                "#})
                    .bind(environment_id)
                    .bind(revision_id),
            )
            .await
    }

    async fn get_deployed_http_api_definitions(
        &self,
        environment_id: &Uuid,
        revision_id: i64,
    ) -> RepoResult<Vec<HttpApiDefinitionRevisionIdentityRecord>> {
        self.with_ro("get_deployed_http_api_definitions")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT had.http_api_definition_id, had.name, hadr.revision_id, hadr.version, hadr.hash
                    FROM http_api_definitions had
                    JOIN http_api_definition_revisions hadr ON had.http_api_definition_id = hadr.http_api_definition_id
                    JOIN deployment_http_api_definition_revisions dhadr
                        ON dhadr.http_api_definition_id = hadr.http_api_definition_id
                            AND dhadr.http_api_definition_revision_id = hadr.revision_id
                    WHERE dhadr.environment_id = $1 AND dhadr.deployment_revision_id = $2
                    ORDER BY had.name
                "#})
                    .bind(environment_id)
                    .bind(revision_id),
            )
            .await
    }

    async fn get_deployed_http_api_deployments(
        &self,
        environment_id: &Uuid,
        revision_id: i64,
    ) -> RepoResult<Vec<HttpApiDeploymentRevisionIdentityRecord>> {
        let deployments: Vec<HttpApiDeploymentRevisionIdentityRecord> = self.with_ro("get_deployed_http_api_deployments - deployments")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT had.http_api_deployment_id, had.domain, hadr.revision_id, hadr.hash
                    FROM http_api_deployments had
                    JOIN http_api_deployment_revisions hadr ON had.http_api_deployment_id = hadr.http_api_deployment_id
                    JOIN deployment_http_api_definition_revisions dhadr
                        ON dhadr.http_api_definition_id = hadr.http_api_deployment_id
                            AND dhadr.http_api_definition_revision_id = hadr.revision_id
                    WHERE dhadr.environment_id = $1 AND dhadr.deployment_revision_id = $2
                    ORDER BY had.domain
                "#})
                    .bind(environment_id)
                    .bind(revision_id),
            )
            .await?;

        Ok(deployments)
    }

    async fn version_exists(&self, environment_id: &Uuid, version: &str) -> RepoResult<bool> {
        Ok(self
            .with_ro("version_exists")
            .fetch_optional(
                sqlx::query(indoc! { r#"
                    SELECT 1 FROM deployment_revisions
                    WHERE environment_id = $1 AND version = $2
                    LIMIT 1
                "#})
                .bind(environment_id)
                .bind(version),
            )
            .await?
            .is_some())
    }
}
