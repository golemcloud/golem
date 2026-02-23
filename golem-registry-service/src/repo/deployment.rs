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
use super::model::deployment::{CurrentDeploymentExtRevisionRecord, DeploymentCompiledRouteWithSecuritySchemeRecord, DeploymentMcpCapabilityRecord, DeploymentRevisionCreationRecord};
use super::model::deployment::{
    DeploymentCompiledRouteRecord, DeploymentComponentRevisionRecord,
    DeploymentHttpApiDeploymentRevisionRecord, DeploymentRegisteredAgentTypeRecord,
};
use crate::repo::model::audit::RevisionAuditFields;
use crate::repo::model::component::ComponentRevisionIdentityRecord;
use crate::repo::model::deployment::{
    CurrentDeploymentRevisionRecord, DeployRepoError, DeployedDeploymentIdentity,
    DeploymentIdentity, DeploymentRevisionRecord,
};
use crate::repo::model::hash::SqlBlake3Hash;
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
    async fn get_next_revision_number(&self, environment_id: Uuid) -> RepoResult<Option<i64>>;

    async fn get_currently_deployed_revision(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Option<DeploymentRevisionRecord>>;

    async fn get_latest_revision(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Option<DeploymentRevisionRecord>>;

    async fn list_deployment_revisions(
        &self,
        environment_id: Uuid,
        version: Option<&str>,
    ) -> RepoResult<Vec<DeploymentRevisionRecord>>;

    async fn list_deployment_history(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Vec<CurrentDeploymentRevisionRecord>>;

    async fn get_staged_identity(&self, environment_id: Uuid) -> RepoResult<DeploymentIdentity>;

    async fn get_deployment_revision(
        &self,
        environment_id: Uuid,
        revision_id: i64,
    ) -> RepoResult<Option<DeploymentRevisionRecord>>;

    async fn get_deployment_identity(
        &self,
        environment_id: Uuid,
        revision_id: i64,
    ) -> RepoResult<Option<DeployedDeploymentIdentity>>;

    async fn deploy(
        &self,
        user_account_id: Uuid,
        deployment_creation: DeploymentRevisionCreationRecord,
        version_check: bool,
    ) -> Result<CurrentDeploymentExtRevisionRecord, DeployRepoError>;

    async fn list_active_compiled_routes_for_domain(
        &self,
        domain: &str,
    ) -> RepoResult<Vec<DeploymentCompiledRouteWithSecuritySchemeRecord>>;
    
    async fn get_active_mcp_for_domain(
        &self,
        domain: &str,
    ) -> RepoResult<Option<DeploymentMcpCapabilityRecord>>;

    async fn list_compiled_routes_for_domain_and_deployment(
        &self,
        environment_id: Uuid,
        deployment_revision_id: i64,
        domain: &str,
    ) -> RepoResult<Vec<DeploymentCompiledRouteWithSecuritySchemeRecord>>;

    async fn get_deployment_agent_type(
        &self,
        environment_id: Uuid,
        deployment_revision_id: i64,
        agent_type_name: &str,
    ) -> RepoResult<Option<DeploymentRegisteredAgentTypeRecord>>;

    async fn list_deployment_agent_types(
        &self,
        environment_id: Uuid,
        deployment_revision_id: i64,
    ) -> RepoResult<Vec<DeploymentRegisteredAgentTypeRecord>>;

    async fn get_deployed_agent_type(
        &self,
        environment_id: Uuid,
        agent_type_name: &str,
    ) -> RepoResult<Option<DeploymentRegisteredAgentTypeRecord>>;

    async fn list_deployed_agent_types(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Vec<DeploymentRegisteredAgentTypeRecord>>;

    async fn get_latest_deployed_agent_type_by_component_revision(
        &self,
        environment_id: &Uuid,
        component_id: &Uuid,
        component_revision_id: i64,
        agent_type_name: &str,
    ) -> RepoResult<Option<DeploymentRegisteredAgentTypeRecord>>;

    async fn list_latest_deployed_agent_types_by_component_revision(
        &self,
        environment_id: &Uuid,
        component_id: &Uuid,
        component_revision_id: i64,
    ) -> RepoResult<Vec<DeploymentRegisteredAgentTypeRecord>>;

    async fn set_current_deployment(
        &self,
        user_account_id: Uuid,
        environment_id: Uuid,
        deployment_revision_id: i64,
    ) -> Result<CurrentDeploymentRevisionRecord, DeployRepoError>;
}

pub struct LoggedDeploymentRepo<Repo: DeploymentRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "deployment repository";

impl<Repo: DeploymentRepo> LoggedDeploymentRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_env(environment_id: Uuid) -> Span {
        info_span!(
            SPAN_NAME,
            environment_id = %environment_id,
        )
    }

    fn span_env_and_revision(environment_id: Uuid, revision_id: i64) -> Span {
        info_span!(
            SPAN_NAME,
            environment_id = %environment_id,
            revision_id
        )
    }

    fn span_user_and_env(user_account_id: Uuid, environment_id: Uuid) -> Span {
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
    async fn get_next_revision_number(&self, environment_id: Uuid) -> RepoResult<Option<i64>> {
        self.repo
            .get_next_revision_number(environment_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn get_deployment_revision(
        &self,
        environment_id: Uuid,
        revision_id: i64,
    ) -> RepoResult<Option<DeploymentRevisionRecord>> {
        self.repo
            .get_deployment_revision(environment_id, revision_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn get_currently_deployed_revision(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Option<DeploymentRevisionRecord>> {
        self.repo
            .get_currently_deployed_revision(environment_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn get_latest_revision(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Option<DeploymentRevisionRecord>> {
        self.repo
            .get_latest_revision(environment_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn list_deployment_revisions(
        &self,
        environment_id: Uuid,
        version: Option<&str>,
    ) -> RepoResult<Vec<DeploymentRevisionRecord>> {
        self.repo
            .list_deployment_revisions(environment_id, version)
            .instrument(info_span!(
                SPAN_NAME,
                environment_id = %environment_id,
                version = ?version
            ))
            .await
    }

    async fn list_deployment_history(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Vec<CurrentDeploymentRevisionRecord>> {
        self.repo
            .list_deployment_history(environment_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn get_staged_identity(&self, environment_id: Uuid) -> RepoResult<DeploymentIdentity> {
        self.repo
            .get_staged_identity(environment_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn get_deployment_identity(
        &self,
        environment_id: Uuid,
        revision_id: i64,
    ) -> RepoResult<Option<DeployedDeploymentIdentity>> {
        self.repo
            .get_deployment_identity(environment_id, revision_id)
            .instrument(Self::span_env_and_revision(environment_id, revision_id))
            .await
    }

    async fn deploy(
        &self,
        user_account_id: Uuid,
        deployment_creation: DeploymentRevisionCreationRecord,
        version_check: bool,
    ) -> Result<CurrentDeploymentExtRevisionRecord, DeployRepoError> {
        let span = Self::span_user_and_env(user_account_id, deployment_creation.environment_id);
        self.repo
            .deploy(user_account_id, deployment_creation, version_check)
            .instrument(span)
            .await
    }

    async fn list_active_compiled_routes_for_domain(
        &self,
        domain: &str,
    ) -> RepoResult<Vec<DeploymentCompiledRouteWithSecuritySchemeRecord>> {
        self.repo
            .list_active_compiled_routes_for_domain(domain)
            .instrument(Self::span_domain(domain))
            .await
    }
    
    async fn get_active_mcp_for_domain(
        &self,
        domain: &str,
    ) -> RepoResult<Option<DeploymentMcpCapabilityRecord>> {
        self.repo
            .get_active_mcp_for_domain(domain)
            .instrument(Self::span_domain(domain))
            .await
    }

    async fn list_compiled_routes_for_domain_and_deployment(
        &self,
        environment_id: Uuid,
        deployment_revision_id: i64,
        domain: &str,
    ) -> RepoResult<Vec<DeploymentCompiledRouteWithSecuritySchemeRecord>> {
        self.repo
            .list_compiled_routes_for_domain_and_deployment(
                environment_id,
                deployment_revision_id,
                domain,
            )
            .instrument(info_span!(
                SPAN_NAME,
                environment_id = %environment_id,
                deployment_revision_id,
                domain
            ))
            .await
    }

    async fn get_deployment_agent_type(
        &self,
        environment_id: Uuid,
        deployment_revision_id: i64,
        agent_type_name: &str,
    ) -> RepoResult<Option<DeploymentRegisteredAgentTypeRecord>> {
        self.repo
            .get_deployment_agent_type(environment_id, deployment_revision_id, agent_type_name)
            .instrument(info_span!(
                SPAN_NAME,
                environment_id = %environment_id,
                deployment_revision_id,
                agent_type_name
            ))
            .await
    }

    async fn list_deployment_agent_types(
        &self,
        environment_id: Uuid,
        deployment_revision_id: i64,
    ) -> RepoResult<Vec<DeploymentRegisteredAgentTypeRecord>> {
        self.repo
            .list_deployment_agent_types(environment_id, deployment_revision_id)
            .instrument(info_span!(
                SPAN_NAME,
                environment_id = %environment_id,
                deployment_revision_id
            ))
            .await
    }

    async fn get_deployed_agent_type(
        &self,
        environment_id: Uuid,
        agent_type_name: &str,
    ) -> RepoResult<Option<DeploymentRegisteredAgentTypeRecord>> {
        self.repo
            .get_deployed_agent_type(environment_id, agent_type_name)
            .instrument(info_span!(
                SPAN_NAME,
                environment_id = %environment_id,
                agent_type_name
            ))
            .await
    }

    async fn list_deployed_agent_types(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Vec<DeploymentRegisteredAgentTypeRecord>> {
        self.repo
            .list_deployed_agent_types(environment_id)
            .instrument(info_span!(
                SPAN_NAME,
                environment_id = %environment_id,
            ))
            .await
    }

    async fn get_latest_deployed_agent_type_by_component_revision(
        &self,
        environment_id: &Uuid,
        component_id: &Uuid,
        component_revision_id: i64,
        agent_type_name: &str,
    ) -> RepoResult<Option<DeploymentRegisteredAgentTypeRecord>> {
        self.repo
            .get_latest_deployed_agent_type_by_component_revision(
                environment_id,
                component_id,
                component_revision_id,
                agent_type_name,
            )
            .instrument(info_span!(
                SPAN_NAME,
                environment_id = %environment_id,
                component_id = %component_id,
                component_revision_id = %component_revision_id,
                agent_type_name = %agent_type_name,
            ))
            .await
    }

    async fn list_latest_deployed_agent_types_by_component_revision(
        &self,
        environment_id: &Uuid,
        component_id: &Uuid,
        component_revision_id: i64,
    ) -> RepoResult<Vec<DeploymentRegisteredAgentTypeRecord>> {
        self.repo
            .list_latest_deployed_agent_types_by_component_revision(
                environment_id,
                component_id,
                component_revision_id,
            )
            .instrument(info_span!(
                SPAN_NAME,
                environment_id = %environment_id,
                component_id = %component_id,
                component_revision_id = %component_revision_id,
            ))
            .await
    }

    async fn set_current_deployment(
        &self,
        user_account_id: Uuid,
        environment_id: Uuid,
        deployment_revision_id: i64,
    ) -> Result<CurrentDeploymentRevisionRecord, DeployRepoError> {
        self.repo
            .set_current_deployment(user_account_id, environment_id, deployment_revision_id)
            .instrument(info_span!(
                SPAN_NAME,
                user_account_id = %user_account_id,
                environment_id = %environment_id,
                deployment_revision_id
            ))
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
    async fn get_next_revision_number(&self, environment_id: Uuid) -> RepoResult<Option<i64>> {
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

    async fn get_deployment_revision(
        &self,
        environment_id: Uuid,
        revision_id: i64,
    ) -> RepoResult<Option<DeploymentRevisionRecord>> {
        self.with_ro("get_deployment_revision").fetch_optional_as(
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
        environment_id: Uuid,
    ) -> RepoResult<Option<DeploymentRevisionRecord>> {
        self.with_ro("get_currently_deployed_revision").fetch_optional_as(
            sqlx::query_as(indoc! { r#"
                SELECT dr.environment_id, dr.revision_id, dr.version, dr.hash, dr.created_at, dr.created_by
                FROM current_deployments cd
                JOIN current_deployment_revisions cdr
                    ON dr.environment_id = cd.environment_id AND cdr.current_revision_id = cd.current_revision_id
                WHERE cd.environment_id = $1
            "#})
                .bind(environment_id),
        ).await
    }

    async fn get_latest_revision(
        &self,
        environment_id: Uuid,
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
        environment_id: Uuid,
        version: Option<&str>,
    ) -> RepoResult<Vec<DeploymentRevisionRecord>> {
        self.with_ro("list_deployment_revisions")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                SELECT environment_id, revision_id, version, hash, created_at, created_by
                FROM deployment_revisions
                WHERE environment_id = $1
                    AND ($2 IS NULL OR version = $2)
                ORDER BY revision_id
            "#})
                .bind(environment_id)
                .bind(version),
            )
            .await
    }

    async fn list_deployment_history(
        &self,
        environment_id: Uuid,
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

    async fn get_staged_identity(&self, environment_id: Uuid) -> RepoResult<DeploymentIdentity> {
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
        environment_id: Uuid,
        revision_id: i64,
    ) -> RepoResult<Option<DeployedDeploymentIdentity>> {
        let deployment_revision = self
            .get_deployment_revision(environment_id, revision_id)
            .await?;

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
                http_api_deployments: self
                    .get_deployed_http_api_deployments(environment_id, revision_id)
                    .await?,
            },
        }))
    }

    async fn deploy(
        &self,
        user_account_id: Uuid,
        deployment_creation: DeploymentRevisionCreationRecord,
        version_check: bool,
    ) -> Result<CurrentDeploymentExtRevisionRecord, DeployRepoError> {
        if version_check
            && self
                .version_exists(
                    deployment_creation.environment_id,
                    &deployment_creation.version,
                )
                .await?
        {
            return Err(DeployRepoError::VersionAlreadyExists {
                version: deployment_creation.version,
            });
        }

        self.with_tx_err("deploy", |tx| {
            async move {
                let environment_id = deployment_creation.environment_id;
                let deployment_revision_id = deployment_creation.deployment_revision_id;

                let deployment_revision = Self::create_deployment_revision(
                    tx,
                    user_account_id,
                    environment_id,
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

                for deployment in &deployment_creation.http_api_deployments {
                    Self::create_deployment_http_api_deployment_revision(tx, deployment).await?
                }

                for compiled_route in &deployment_creation.compiled_routes {
                    Self::create_deployment_compiled_route(tx, compiled_route).await?
                }

                for registered_agent_type in &deployment_creation.registered_agent_types {
                    Self::create_deployment_registered_agent_type(tx, registered_agent_type)
                        .await?;
                }

                let revision = Self::set_current_deployment_internal(
                    tx,
                    user_account_id,
                    deployment_revision.environment_id,
                    deployment_revision.revision_id,
                )
                .await?;

                let ext_revision = CurrentDeploymentExtRevisionRecord {
                    revision,
                    deployment_version: deployment_revision.version,
                    deployment_hash: deployment_revision.hash,
                };

                Ok(ext_revision)
            }
            .boxed()
        })
        .await
    }
    
    async fn get_active_mcp_for_domain(
        &self,
        domain: &str
    ) -> RepoResult<Option<DeploymentMcpCapabilityRecord>> {
        
        self.with_ro("list_active_mcp_for_domain")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        ac.account_id,
                        e.environment_id,
                        cd.current_revision_id as deployment_revision_id,
                        dr.domain,
                        array_agg(DISTINCT r.agent_type_name) AS agent_types

                    FROM deployment_compiled_mcp cm
                    
                    -- active deployment
                    JOIN current_deployments cd
                      ON cd.environment_id = r.environment_id
                      AND cd.current_revision_id = r.deployment_revision_id

                    -- parent objects not deleted
                    JOIN environments e
                      ON e.environment_id = r.environment_id
                      AND e.deleted_at IS NULL
                    JOIN applications a
                      ON a.application_id = e.application_id
                      AND a.deleted_at IS NULL
                    JOIN accounts ac
                      ON ac.account_id = a.account_id
                      AND ac.deleted_at IS NULL

                    -- registered domains
                    JOIN domain_registrations dr
                      ON dr.environment_id = r.environment_id
                      AND dr.domain = r.domain
                      AND dr.deleted_at IS NULL

                    WHERE r.domain = $1
                "#})
                .bind(domain),
            )
            .await
    }
    

    async fn list_active_compiled_routes_for_domain(
        &self,
        domain: &str,
    ) -> RepoResult<Vec<DeploymentCompiledRouteWithSecuritySchemeRecord>> {
        self.with_ro("list_active_compiled_http_api_routes_for_domain")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        ac.account_id,
                        e.environment_id,
                        r.deployment_revision_id,
                        r.domain,
                        r.route_id,
                        FALSE as security_scheme_missing,
                        s.security_scheme_id,
                        s.name AS security_scheme_name,
                        sr.provider_type AS security_scheme_provider_type,
                        sr.client_id AS security_scheme_client_id,
                        sr.client_secret AS security_scheme_client_secret,
                        sr.redirect_url AS security_scheme_redirect_url,
                        sr.scopes AS security_scheme_scopes,
                        r.compiled_route

                    FROM deployment_compiled_routes r

                    -- active deployment
                    JOIN current_deployments cd
                      ON cd.environment_id = r.environment_id
                      AND cd.current_revision_id = r.deployment_revision_id

                    -- parent objects not deleted
                    JOIN environments e
                      ON e.environment_id = r.environment_id
                      AND e.deleted_at IS NULL
                    JOIN applications a
                      ON a.application_id = e.application_id
                      AND a.deleted_at IS NULL
                    JOIN accounts ac
                      ON ac.account_id = a.account_id
                      AND ac.deleted_at IS NULL

                    -- registered domains
                    JOIN domain_registrations dr
                      ON dr.environment_id = r.environment_id
                      AND dr.domain = r.domain
                      AND dr.deleted_at IS NULL

                    -- route-level optional security scheme
                    LEFT JOIN security_schemes s
                      ON s.environment_id = r.environment_id
                      AND s.name = r.security_scheme
                      AND s.deleted_at IS NULL

                    LEFT JOIN security_scheme_revisions sr
                      ON sr.security_scheme_id = s.security_scheme_id
                      AND sr.revision_id = s.current_revision_id

                    WHERE r.domain = $1 AND (r.security_scheme IS NULL OR s.security_scheme_id IS NOT NULL)

                    ORDER BY r.route_id
                "#})
                .bind(domain),
            )
            .await
    }

    async fn list_compiled_routes_for_domain_and_deployment(
        &self,
        environment_id: Uuid,
        deployment_revision_id: i64,
        domain: &str,
    ) -> RepoResult<Vec<DeploymentCompiledRouteWithSecuritySchemeRecord>> {
        self.with_ro("list_compiled_http_api_routes_for_http_api_definition")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        ac.account_id,
                        e.environment_id,
                        r.deployment_revision_id,
                        r.domain,
                        r.route_id,
                        (r.security_scheme IS NOT NULL AND s.security_scheme_id IS NULL) AS security_scheme_missing,
                        s.security_scheme_id,
                        s.name AS security_scheme_name,
                        sr.provider_type AS security_scheme_provider_type,
                        sr.client_id AS security_scheme_client_id,
                        sr.client_secret AS security_scheme_client_secret,
                        sr.redirect_url AS security_scheme_redirect_url,
                        sr.scopes AS security_scheme_scopes,
                        r.compiled_route

                    FROM deployment_compiled_routes r

                    -- parent objects not deleted
                    JOIN environments e
                      ON e.environment_id = d.environment_id
                      AND e.deleted_at IS NULL
                    JOIN applications a
                      ON a.application_id = e.application_id
                      AND a.deleted_at IS NULL
                    JOIN accounts ac
                      ON ac.account_id = a.account_id
                      AND ac.deleted_at IS NULL

                    -- route-level optional security scheme
                    LEFT JOIN security_schemes s
                      ON s.environment_id = r.environment_id
                      AND s.name = r.security_scheme
                      AND s.deleted_at IS NULL

                    LEFT JOIN security_scheme_revisions sr
                      ON sr.security_scheme_id = s.security_scheme_id
                      AND sr.revision_id = s.current_revision_id

                     WHERE r.environment_id = $1
                       AND r.deployment_revision_id = $2
                       AND r.domain = $3

                    ORDER BY r.route_id
                "#})
                .bind(environment_id)
                .bind(deployment_revision_id)
                .bind(domain),
            )
            .await
    }

    async fn get_deployment_agent_type(
        &self,
        environment_id: Uuid,
        deployment_revision_id: i64,
        agent_type_name: &str,
    ) -> RepoResult<Option<DeploymentRegisteredAgentTypeRecord>> {
        self.with_ro("get_deployment_agent_type")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        r.environment_id,
                        r.deployment_revision_id,
                        r.agent_type_name,
                        r.agent_wrapper_type_name,
                        r.component_id,
                        r.component_revision_id,
                        r.webhook_prefix_authority_and_path,
                        r.agent_type
                    FROM deployment_registered_agent_types r
                    WHERE r.environment_id = $1 AND r.deployment_revision_id = $2
                        AND (r.agent_type_name = $3 OR r.agent_wrapper_type_name = $3)
                "#})
                .bind(environment_id)
                .bind(deployment_revision_id)
                .bind(agent_type_name),
            )
            .await
    }

    async fn list_deployment_agent_types(
        &self,
        environment_id: Uuid,
        deployment_revision_id: i64,
    ) -> RepoResult<Vec<DeploymentRegisteredAgentTypeRecord>> {
        self.with_ro("list_deployment_agent_types")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        r.environment_id,
                        r.deployment_revision_id,
                        r.agent_type_name,
                        r.agent_wrapper_type_name,
                        r.component_id,
                        r.component_revision_id,
                        r.webhook_prefix_authority_and_path,
                        r.agent_type
                    FROM deployment_registered_agent_types r
                    WHERE r.environment_id = $1 AND r.deployment_revision_id = $2
                    ORDER BY r.agent_type_name
                "#})
                .bind(environment_id)
                .bind(deployment_revision_id),
            )
            .await
    }

    async fn get_deployed_agent_type(
        &self,
        environment_id: Uuid,
        agent_type_name: &str,
    ) -> RepoResult<Option<DeploymentRegisteredAgentTypeRecord>> {
        self.with_ro("get_deployed_agent_type")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        r.environment_id,
                        r.deployment_revision_id,
                        r.agent_type_name,
                        r.agent_wrapper_type_name,
                        r.component_id,
                        r.component_revision_id,
                        r.webhook_prefix_authority_and_path,
                        r.agent_type
                    FROM current_deployments cd
                    JOIN current_deployment_revisions cdr
                        ON cdr.environment_id = cd.environment_id AND cdr.revision_id = cd.current_revision_id
                    JOIN deployment_registered_agent_types r
                        ON r.environment_id = cdr.environment_id AND r.deployment_revision_id = cdr.deployment_revision_id
                    WHERE cd.environment_id = $1 AND (r.agent_type_name = $2 OR r.agent_wrapper_type_name = $2)
                "#})
                .bind(environment_id)
                .bind(agent_type_name)
            )
            .await
    }

    async fn list_deployed_agent_types(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Vec<DeploymentRegisteredAgentTypeRecord>> {
        self.with_ro("get_deployed_agent_type")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        r.environment_id,
                        r.deployment_revision_id,
                        r.agent_type_name,
                        r.agent_wrapper_type_name,
                        r.component_id,
                        r.component_revision_id,
                        r.webhook_prefix_authority_and_path,
                        r.agent_type
                    FROM current_deployments cd
                    JOIN current_deployment_revisions cdr
                        ON cdr.environment_id = cd.environment_id AND cdr.revision_id = cd.current_revision_id
                    JOIN deployment_registered_agent_types r
                        ON r.environment_id = cdr.environment_id AND r.deployment_revision_id = cdr.deployment_revision_id
                    WHERE cd.environment_id = $1
                    ORDER BY r.agent_type_name
                "#})
                .bind(environment_id)
            )
            .await
    }

    async fn set_current_deployment(
        &self,
        user_account_id: Uuid,
        environment_id: Uuid,
        deployment_revision_id: i64,
    ) -> Result<CurrentDeploymentRevisionRecord, DeployRepoError> {
        self.with_tx_err("set_current_deployment", |tx| {
            Box::pin(async move {
                Self::set_current_deployment_internal(
                    tx,
                    user_account_id,
                    environment_id,
                    deployment_revision_id,
                )
                .await
            })
        })
        .await
    }

    async fn get_latest_deployed_agent_type_by_component_revision(
        &self,
        environment_id: &Uuid,
        component_id: &Uuid,
        component_revision_id: i64,
        agent_type_name: &str,
    ) -> RepoResult<Option<DeploymentRegisteredAgentTypeRecord>> {
        self.with_ro("get_latest_deployed_agent_type_by_component_revision")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        r.environment_id,
                        r.deployment_revision_id,
                        r.agent_type_name,
                        r.agent_wrapper_type_name,
                        r.component_id,
                        r.component_revision_id,
                        r.webhook_prefix_authority_and_path,
                        r.agent_type
                    FROM deployment_registered_agent_types r
                    WHERE r.environment_id = $1
                        AND r.deployment_revision_id = (
                            SELECT deployment_revision_id FROM deployment_registered_agent_types
                            WHERE component_id = $2 AND component_revision_id = $3
                            ORDER BY deployment_revision_id DESC
                            LIMIT 1
                        )
                        AND (r.agent_type_name = $4 OR r.agent_wrapper_type_name = $4)
                    ORDER BY r.agent_type_name
                "#})
                .bind(environment_id)
                .bind(component_id)
                .bind(component_revision_id)
                .bind(agent_type_name),
            )
            .await
    }

    async fn list_latest_deployed_agent_types_by_component_revision(
        &self,
        environment_id: &Uuid,
        component_id: &Uuid,
        component_revision_id: i64,
    ) -> RepoResult<Vec<DeploymentRegisteredAgentTypeRecord>> {
        self.with_ro("get_deployed_agent_type")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        r.environment_id,
                        r.deployment_revision_id,
                        r.agent_type_name,
                        r.agent_wrapper_type_name,
                        r.component_id,
                        r.component_revision_id,
                        r.webhook_prefix_authority_and_path,
                        r.agent_type
                    FROM deployment_registered_agent_types r
                    WHERE r.environment_id = $1
                        AND r.deployment_revision_id = (
                            SELECT deployment_revision_id FROM deployment_registered_agent_types
                            WHERE component_id = $2 AND component_revision_id = $3
                            ORDER BY deployment_revision_id DESC
                            LIMIT 1
                        )
                    ORDER BY r.agent_type_name
                "#})
                .bind(environment_id)
                .bind(component_id)
                .bind(component_revision_id),
            )
            .await
    }
}

#[async_trait]
trait DeploymentRepoInternal: DeploymentRepo {
    type Db: Database;
    type Tx: LabelledPoolTransaction;

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
        environment_id: Uuid,
    ) -> RepoResult<DeploymentIdentity>;

    async fn get_staged_components(
        tx: &mut Self::Tx,
        environment_id: Uuid,
    ) -> RepoResult<Vec<ComponentRevisionIdentityRecord>>;

    async fn get_staged_http_api_deployments(
        tx: &mut Self::Tx,
        environment_id: Uuid,
    ) -> RepoResult<Vec<HttpApiDeploymentRevisionIdentityRecord>>;

    async fn create_deployment_component_revision(
        tx: &mut Self::Tx,
        environment_id: Uuid,
        deployment_revision_id: i64,
        component: &DeploymentComponentRevisionRecord,
    ) -> RepoResult<()>;

    async fn create_deployment_http_api_deployment_revision(
        tx: &mut Self::Tx,
        http_api_deployment: &DeploymentHttpApiDeploymentRevisionRecord,
    ) -> RepoResult<()>;

    async fn create_deployment_compiled_route(
        tx: &mut Self::Tx,
        compiled_route: &DeploymentCompiledRouteRecord,
    ) -> RepoResult<()>;

    async fn create_deployment_registered_agent_type(
        tx: &mut Self::Tx,
        registered_agent_type: &DeploymentRegisteredAgentTypeRecord,
    ) -> RepoResult<()>;

    async fn set_current_deployment_internal(
        tx: &mut Self::Tx,
        user_account_id: Uuid,
        environment_id: Uuid,
        deployment_revision_id: i64,
    ) -> Result<CurrentDeploymentRevisionRecord, DeployRepoError>;

    async fn get_deployed_components(
        &self,
        environment_id: Uuid,
        revision_id: i64,
    ) -> RepoResult<Vec<ComponentRevisionIdentityRecord>>;

    async fn get_deployed_http_api_deployments(
        &self,
        environment_id: Uuid,
        revision_id: i64,
    ) -> RepoResult<Vec<HttpApiDeploymentRevisionIdentityRecord>>;

    async fn version_exists(&self, environment_id: Uuid, version: &str) -> RepoResult<bool>;
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl DeploymentRepoInternal for DbDeploymentRepo<PostgresPool> {
    type Db = <PostgresPool as Pool>::Db;
    type Tx = <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction;

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
        environment_id: Uuid,
    ) -> RepoResult<DeploymentIdentity> {
        Ok(DeploymentIdentity {
            components: Self::get_staged_components(tx, environment_id).await?,
            http_api_deployments: Self::get_staged_http_api_deployments(tx, environment_id).await?,
        })
    }

    async fn get_staged_components(
        tx: &mut Self::Tx,
        environment_id: Uuid,
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

    async fn get_staged_http_api_deployments(
        tx: &mut Self::Tx,
        environment_id: Uuid,
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
        environment_id: Uuid,
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

    async fn create_deployment_compiled_route(
        tx: &mut Self::Tx,
        compiled_route: &DeploymentCompiledRouteRecord,
    ) -> RepoResult<()> {
        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO deployment_compiled_routes
                    (environment_id, deployment_revision_id, domain, route_id, security_scheme, compiled_route)
                VALUES ($1, $2, $3, $4, $5, $6)
            "#})
                .bind(compiled_route.environment_id)
                .bind(compiled_route.deployment_revision_id)
                .bind(&compiled_route.domain)
                .bind(compiled_route.route_id)
                .bind(&compiled_route.security_scheme)
                .bind(&compiled_route.compiled_route)
        )
            .await?;

        Ok(())
    }

    async fn create_deployment_registered_agent_type(
        tx: &mut Self::Tx,
        registered_agent_type: &DeploymentRegisteredAgentTypeRecord,
    ) -> RepoResult<()> {
        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO deployment_registered_agent_types
                    (environment_id, deployment_revision_id,
                     agent_type_name, agent_wrapper_type_name,
                     component_id, component_revision_id,
                     webhook_prefix_authority_and_path, agent_type)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#})
            .bind(registered_agent_type.environment_id)
            .bind(registered_agent_type.deployment_revision_id)
            .bind(&registered_agent_type.agent_type_name)
            .bind(&registered_agent_type.agent_wrapper_type_name)
            .bind(registered_agent_type.component_id)
            .bind(registered_agent_type.component_revision_id)
            .bind(&registered_agent_type.webhook_prefix_authority_and_path)
            .bind(&registered_agent_type.agent_type),
        )
        .await?;

        Ok(())
    }

    async fn set_current_deployment_internal(
        tx: &mut Self::Tx,
        user_account_id: Uuid,
        environment_id: Uuid,
        deployment_revision_id: i64,
    ) -> Result<CurrentDeploymentRevisionRecord, DeployRepoError> {
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
            Some(row) => row.try_get::<i64, _>(0).map_err(RepoError::from)? + 1,
            None => 0,
        };

        let revision: CurrentDeploymentRevisionRecord = tx
            .fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    INSERT INTO current_deployment_revisions
                    (environment_id, revision_id, created_at, created_by, deployment_revision_id)
                    VALUES ($1, $2, $3, $4, $5)
                    RETURNING environment_id, revision_id, created_at, created_by, deployment_revision_id
                "#})
                .bind(environment_id)
                .bind(revision_id)
                .bind_revision_audit(RevisionAuditFields::new(user_account_id))
                .bind(deployment_revision_id)
            )
            .await
            .to_error_on_unique_violation(DeployRepoError::ConcurrentModification)?;

        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO current_deployments (environment_id, current_revision_id)
                VALUES ($1, $2)
                ON CONFLICT (environment_id)
                DO UPDATE SET current_revision_id = excluded.current_revision_id
            "#})
            .bind(environment_id)
            .bind(revision_id),
        )
        .await?;

        Ok(revision)
    }

    async fn get_deployed_components(
        &self,
        environment_id: Uuid,
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

    async fn get_deployed_http_api_deployments(
        &self,
        environment_id: Uuid,
        revision_id: i64,
    ) -> RepoResult<Vec<HttpApiDeploymentRevisionIdentityRecord>> {
        let deployments: Vec<HttpApiDeploymentRevisionIdentityRecord> = self.with_ro("get_deployed_http_api_deployments - deployments")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT had.http_api_deployment_id, had.domain, hadr.revision_id, hadr.hash
                    FROM http_api_deployments had
                    JOIN http_api_deployment_revisions hadr ON had.http_api_deployment_id = hadr.http_api_deployment_id
                    JOIN deployment_http_api_deployment_revisions dhadr
                        ON dhadr.http_api_deployment_id = hadr.http_api_deployment_id
                            AND dhadr.http_api_deployment_revision_id = hadr.revision_id
                    WHERE dhadr.environment_id = $1 AND dhadr.deployment_revision_id = $2
                    ORDER BY had.domain
                "#})
                    .bind(environment_id)
                    .bind(revision_id),
            )
            .await?;

        Ok(deployments)
    }

    async fn version_exists(&self, environment_id: Uuid, version: &str) -> RepoResult<bool> {
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
