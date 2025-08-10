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
use crate::repo::environment::{
    EnvironmentExtRevisionRecord, EnvironmentSharedQueries, EnvironmentSharedRepoImpl,
};
use crate::repo::model::audit::RevisionAuditFields;
use crate::repo::model::component::ComponentRevisionIdentityRecord;
use crate::repo::model::deployment::{
    CurrentDeploymentRevisionRecord, DeployRepoError, DeployValidationError,
    DeployedDeploymentIdentity, DeploymentIdentity, DeploymentRevisionRecord,
};
use crate::repo::model::hash::SqlBlake3Hash;
use crate::repo::model::http_api_definition::HttpApiDefinitionRevisionIdentityRecord;
use crate::repo::model::http_api_deployment::HttpApiDeploymentRevisionIdentityRecord;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_common::model::diff::Hashable;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{
    LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi, ToBusiness, TxError, TxResult,
};
use golem_service_base::repo::{BusinessResult, RepoResult, ResultExt};
use indoc::indoc;
use sqlx::{Database, Row};
use std::collections::HashSet;
use std::fmt::Display;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait DeploymentRepo: Send + Sync {
    async fn get_deployed_revision(
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
        environment_id: &Uuid,
        current_staged_deployment_revision_id: Option<i64>,
        version: String,
        expected_deployment_hash: SqlBlake3Hash,
    ) -> BusinessResult<CurrentDeploymentRevisionRecord, DeployRepoError>;

    async fn deploy_by_revision_id(
        &self,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        revision_id: i64,
    ) -> BusinessResult<CurrentDeploymentRevisionRecord, DeployRepoError>;

    async fn deploy_by_version(
        &self,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        version: &str,
    ) -> BusinessResult<CurrentDeploymentRevisionRecord, DeployRepoError>;
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

    fn span_user_env_revision(
        user_account_id: &Uuid,
        environment_id: &Uuid,
        revision_id: i64,
    ) -> Span {
        info_span!(
            SPAN_NAME,
            user_account_id = %user_account_id,
            environment_id = %environment_id,
            revision_id
        )
    }

    fn span_user_env_version(user_account_id: &Uuid, environment_id: &Uuid, version: &str) -> Span {
        info_span!(
            SPAN_NAME,
            user_account_id = %user_account_id,
            environment_id = %environment_id,
            version
        )
    }

    fn span_user_and_env(user_account_id: &Uuid, environment_id: &Uuid) -> Span {
        info_span!(
            SPAN_NAME,
            user_account_id = %user_account_id,
            environment_id = %environment_id,
        )
    }
}

#[async_trait]
impl<Repo: DeploymentRepo> DeploymentRepo for LoggedDeploymentRepo<Repo> {
    async fn get_deployed_revision(
        &self,
        environment_id: &Uuid,
    ) -> RepoResult<Option<DeploymentRevisionRecord>> {
        self.repo
            .get_deployed_revision(environment_id)
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
        environment_id: &Uuid,
        current_staged_deployment_revision_id: Option<i64>,
        version: String,
        expected_deployment_hash: SqlBlake3Hash,
    ) -> BusinessResult<CurrentDeploymentRevisionRecord, DeployRepoError> {
        self.repo
            .deploy(
                user_account_id,
                environment_id,
                current_staged_deployment_revision_id,
                version,
                expected_deployment_hash,
            )
            .instrument(Self::span_user_and_env(user_account_id, environment_id))
            .await
    }

    async fn deploy_by_revision_id(
        &self,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        revision_id: i64,
    ) -> BusinessResult<CurrentDeploymentRevisionRecord, DeployRepoError> {
        self.repo
            .deploy_by_revision_id(user_account_id, environment_id, revision_id)
            .instrument(Self::span_user_env_revision(
                user_account_id,
                environment_id,
                revision_id,
            ))
            .await
    }

    async fn deploy_by_version(
        &self,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        version: &str,
    ) -> BusinessResult<CurrentDeploymentRevisionRecord, DeployRepoError> {
        self.repo
            .deploy_by_version(user_account_id, environment_id, version)
            .instrument(Self::span_user_env_version(
                user_account_id,
                environment_id,
                version,
            ))
            .await
    }
}

pub struct DbDeploymentRepo<DBP: Pool> {
    db_pool: DBP,
    environment: EnvironmentSharedRepoImpl<DBP>,
}

static METRICS_SVC_NAME: &str = "deployment";

impl<DBP: Pool> DbDeploymentRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self {
            db_pool: db_pool.clone(),
            environment: EnvironmentSharedRepoImpl::new(db_pool),
        }
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }

    async fn with_tx<R, F>(&self, api_name: &'static str, f: F) -> RepoResult<R>
    where
        R: Send,
        F: for<'f> FnOnce(
                &'f mut <DBP::LabelledApi as LabelledPoolApi>::LabelledTransaction,
            ) -> BoxFuture<'f, RepoResult<R>>
            + Send,
    {
        self.db_pool.with_tx(METRICS_SVC_NAME, api_name, f).await
    }

    async fn with_tx_err<R, E, F>(&self, api_name: &'static str, f: F) -> TxResult<R, E>
    where
        R: Send,
        E: Display + Send,
        F: for<'f> FnOnce(
                &'f mut <DBP::LabelledApi as LabelledPoolApi>::LabelledTransaction,
            ) -> BoxFuture<'f, TxResult<R, E>>
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
    async fn get_deployed_revision(
        &self,
        environment_id: &Uuid,
    ) -> RepoResult<Option<DeploymentRevisionRecord>> {
        self.with_ro("get_deployed_revision").fetch_optional_as(
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
                SELECT environment_id, revision_id, created_at, created_by, deployment_revision_id, deployment_version
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
            None => self.get_deployed_revision(environment_id).await?,
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
        environment_id: &Uuid,
        current_staged_deployment_revision_id: Option<i64>,
        version: String,
        expected_deployment_hash: SqlBlake3Hash,
    ) -> BusinessResult<CurrentDeploymentRevisionRecord, DeployRepoError> {
        let actual_current_staged_revision_id_row = self
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

        let actual_current_staged_revision_id: Option<i64> =
            match actual_current_staged_revision_id_row {
                Some(row) => Some(row.try_get("revision_id")?),
                None => None,
            };

        if current_staged_deployment_revision_id != actual_current_staged_revision_id {
            return Ok(Err(DeployRepoError::ConcurrentModification));
        };

        let revision_id = current_staged_deployment_revision_id.unwrap_or(-1) + 1;

        let environment = self.environment.must_get_by_id(environment_id).await?;
        if environment.revision.version_check
            && self.version_exists(environment_id, &version).await?
        {
            return Ok(Err(DeployRepoError::VersionAlreadyExists { version }));
        }

        let user_account_id = *user_account_id;
        let environment_id = *environment_id;

        self.with_tx_err("deploy", |tx| {
            async move {
                let deployment_revision = Self::create_deployment_revision(
                    tx,
                    user_account_id,
                    environment_id,
                    revision_id,
                    version,
                    expected_deployment_hash,
                )
                .await?;

                let staged_deployment = Self::get_staged_deployment(tx, &environment_id).await?;

                let validation_errors = Self::validate_stage(&environment, &staged_deployment);
                if !validation_errors.is_empty() {
                    return Err(TxError::Business(DeployRepoError::ValidationErrors(
                        validation_errors,
                    )));
                }

                let diffable_deployment = staged_deployment.to_diffable();

                let hash = diffable_deployment.hash();
                if hash.as_blake3_hash() != deployment_revision.hash.as_blake3_hash() {
                    return Err(TxError::Business(DeployRepoError::DeploymentHashMismatch {
                        requested_hash: (*hash.as_blake3_hash()).into(),
                        actual_hash: deployment_revision.hash,
                    }));
                }

                Self::create_deployment_relations(
                    tx,
                    &environment_id,
                    deployment_revision.revision_id,
                    &staged_deployment,
                )
                .await?;

                let revision = Self::set_current_deployment(
                    tx,
                    &user_account_id,
                    &environment_id,
                    deployment_revision.revision_id,
                    &deployment_revision.version,
                )
                .await?;

                Ok(revision)
            }
            .boxed()
        })
        .await
        .to_business_result_on_unique_violation(|| DeployRepoError::ConcurrentModification)
    }

    async fn deploy_by_revision_id(
        &self,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        revision_id: i64,
    ) -> BusinessResult<CurrentDeploymentRevisionRecord, DeployRepoError> {
        let Some(deployment_revision) = self
            .get_deployment_revision(environment_id, revision_id)
            .await?
        else {
            return Ok(Err(DeployRepoError::DeploymentNotFoundByRevision {
                revision_id,
            }));
        };

        let user_account_id = *user_account_id;
        let environment_id = *environment_id;

        self.with_tx("deploy_by_revision_id", |tx| {
            async move {
                Self::set_current_deployment(
                    tx,
                    &user_account_id,
                    &environment_id,
                    deployment_revision.revision_id,
                    &deployment_revision.version,
                )
                .await
            }
            .boxed()
        })
        .await
        .to_business_result_on_unique_violation(|| DeployRepoError::ConcurrentModification)
    }

    async fn deploy_by_version(
        &self,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        version: &str,
    ) -> BusinessResult<CurrentDeploymentRevisionRecord, DeployRepoError> {
        let mut deployment_revisions = self
            .get_deployment_revisions_by_version(environment_id, version)
            .await?;
        if deployment_revisions.len() > 1 {
            return Ok(Err(DeployRepoError::DeploymentIsNotUniqueByVersion {
                version: version.to_string(),
            }));
        }
        let Some(deployment_revision) = deployment_revisions.pop() else {
            return Ok(Err(DeployRepoError::DeploymentNotfoundByVersion {
                version: version.to_string(),
            }));
        };

        let user_account_id = *user_account_id;
        let environment_id = *environment_id;

        self.with_tx("deploy_by_version", |tx| {
            async move {
                Self::set_current_deployment(
                    tx,
                    &user_account_id,
                    &environment_id,
                    deployment_revision.revision_id,
                    &deployment_revision.version,
                )
                .await
            }
            .boxed()
        })
        .await
        .to_business_result_on_unique_violation(|| DeployRepoError::ConcurrentModification)
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

    async fn get_deployment_revisions_by_version(
        &self,
        environment_id: &Uuid,
        version: &str,
    ) -> RepoResult<Vec<DeploymentRevisionRecord>>;

    async fn create_deployment_revision(
        tx: &mut Self::Tx,
        user_account_id: Uuid,
        environment_id: Uuid,
        revision_id: i64,
        version: String,
        hash: SqlBlake3Hash,
    ) -> RepoResult<DeploymentRevisionRecord>;

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

    async fn create_deployment_relations(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
        deployment_revision_id: i64,
        stage: &DeploymentIdentity,
    ) -> RepoResult<()>;

    async fn create_deployment_component_revision(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
        deployment_revision_id: i64,
        component: &ComponentRevisionIdentityRecord,
    ) -> RepoResult<()>;

    async fn create_deployment_http_api_definition_revision(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
        deployment_revision_id: i64,
        http_api_definition: &HttpApiDefinitionRevisionIdentityRecord,
    ) -> RepoResult<()>;

    async fn create_deployment_http_api_deployment_revision(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
        deployment_revision_id: i64,
        http_api_deployment: &HttpApiDeploymentRevisionIdentityRecord,
    ) -> RepoResult<()>;

    async fn set_current_deployment(
        tx: &mut Self::Tx,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        deployment_revision_id: i64,
        deployment_version: &str,
    ) -> RepoResult<CurrentDeploymentRevisionRecord>;

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

    fn validate_stage(
        environment: &EnvironmentExtRevisionRecord,
        stage: &DeploymentIdentity,
    ) -> Vec<DeployValidationError> {
        let http_api_definition_ids = stage
            .http_api_definitions
            .iter()
            .map(|d| d.http_api_definition_id)
            .collect::<HashSet<_>>();

        let mut errors = Vec::new();

        for http_api_deployment in &stage.http_api_deployments {
            let mut missing_http_api_definition_ids = Vec::new();

            for definition_id in &http_api_deployment.http_api_definitions {
                if !http_api_definition_ids.contains(definition_id) {
                    missing_http_api_definition_ids.push(*definition_id);
                }
            }

            if !missing_http_api_definition_ids.is_empty() {
                errors.push(
                    DeployValidationError::HttpApiDeploymentMissingHttpApiDefinition {
                        http_api_deployment_id: http_api_deployment.http_api_deployment_id,
                        missing_http_api_definition_ids: vec![],
                    },
                )
            }
        }

        if environment.revision.compatibility_check {
            // TODO: validate api def constraints on components
        }

        errors
    }
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

    async fn get_deployment_revisions_by_version(
        &self,
        environment_id: &Uuid,
        version: &str,
    ) -> RepoResult<Vec<DeploymentRevisionRecord>> {
        self.with_ro("get_deployment_revisions_by_version")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT environment_id, revision_id, version, hash, created_at, created_by
                    FROM deployment_revisions
                    WHERE environment_id = $1 AND version = $2
                "#})
                .bind(environment_id)
                .bind(version),
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
    ) -> RepoResult<DeploymentRevisionRecord> {
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
                SELECT c.component_id, c.name, cr.revision_id, cr.revision_id, cr.version, cr.status, cr.hash
                FROM components c
                JOIN component_revisions cr ON
                    cr.component_id = c.component_id
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
                JOIN http_api_definition_revisions dr ON
                    d.http_api_definition_id = dr.http_api_definition_id
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
        let mut deployments: Vec<HttpApiDeploymentRevisionIdentityRecord> = tx
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT d.http_api_deployment_id, d.name, dr.revision_id, dr.hash
                    FROM http_api_deployments d
                    JOIN http_api_deployment_revisions dr ON
                        d.http_api_deployment_id = dr.http_api_deployment_id
                            AND d.current_revision_id = dr.revision_id
                    WHERE d.environment_id = $1 AND d.deleted_at IS NULL
                "#})
                .bind(environment_id),
            )
            .await?;

        // NOTE: this is an N+1 problem / implementation, but we expect very low cardinality around
        //       these for now, and this way we avoid many other inconsistencies or DB specific ways
        //       and limitations to use "IN" properly
        for deployment in &mut deployments {
            let definitions = tx
                .fetch_all(
                    sqlx::query(indoc! { r#"
                        SELECT http_definition_id
                        FROM http_api_deployment_definitions
                        WHERE http_api_deployment_id = $1 AND revision_id = $2
                    "#})
                    .bind(deployment.http_api_deployment_id)
                    .bind(deployment.revision_id),
                )
                .await?;
            deployment.http_api_definitions = definitions
                .iter()
                .map(|row| row.try_get("http_definition_id"))
                .collect::<Result<_, _>>()?;
        }

        Ok(deployments)
    }

    async fn create_deployment_relations(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
        deployment_revision_id: i64,
        stage: &DeploymentIdentity,
    ) -> RepoResult<()> {
        for component in &stage.components {
            Self::create_deployment_component_revision(
                tx,
                environment_id,
                deployment_revision_id,
                component,
            )
            .await?
        }

        for definition in &stage.http_api_definitions {
            Self::create_deployment_http_api_definition_revision(
                tx,
                environment_id,
                deployment_revision_id,
                definition,
            )
            .await?
        }

        for deployment in &stage.http_api_deployments {
            Self::create_deployment_http_api_deployment_revision(
                tx,
                environment_id,
                deployment_revision_id,
                deployment,
            )
            .await?
        }

        Ok(())
    }

    async fn create_deployment_component_revision(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
        deployment_revision_id: i64,
        component: &ComponentRevisionIdentityRecord,
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
            .bind(component.revision_id),
        )
        .await
        .map(|_| ())
    }

    async fn create_deployment_http_api_definition_revision(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
        deployment_revision_id: i64,
        http_api_definition: &HttpApiDefinitionRevisionIdentityRecord,
    ) -> RepoResult<()> {
        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO deployment_http_api_definition_revisions
                    (environment_id, deployment_revision_id, http_api_definition_id, http_api_definition_revision_id)
                VALUES ($1, $2, $3, $4)
            "#})
                .bind(environment_id)
                .bind(deployment_revision_id)
                .bind(http_api_definition.http_api_definition_id)
                .bind(http_api_definition.revision_id),
        )
            .await
            .map(|_| ())
    }

    async fn create_deployment_http_api_deployment_revision(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
        deployment_revision_id: i64,
        http_api_deployment: &HttpApiDeploymentRevisionIdentityRecord,
    ) -> RepoResult<()> {
        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO deployment_http_api_deployment_revisions
                    (environment_id, deployment_revision_id, http_api_deployment_id, http_api_deployment_revision_id)
                VALUES ($1, $2, $3, $4)
            "#})
                .bind(environment_id)
                .bind(deployment_revision_id)
                .bind(http_api_deployment.http_api_deployment_id)
                .bind(http_api_deployment.revision_id)
        )
            .await?;

        Ok(())
    }

    async fn set_current_deployment(
        tx: &mut Self::Tx,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        deployment_revision_id: i64,
        deployment_version: &str,
    ) -> RepoResult<CurrentDeploymentRevisionRecord> {
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

        let revision: CurrentDeploymentRevisionRecord = tx
            .fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    INSERT INTO current_deployment_revisions
                    (environment_id, revision_id, created_at, created_by, deployment_revision_id, deployment_version)
                    VALUES ($1, $2, $3, $4, $5, $6)
                    RETURNING environment_id, revision_id, created_at, created_by, deployment_revision_id, deployment_version
                "#})
                    .bind(environment_id)
                    .bind(revision_id)
                    .bind_revision_audit(RevisionAuditFields::new(*user_account_id))
                    .bind(deployment_revision_id)
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
                    SELECT c.component_id, c.name, cr.revision_id, cr.revision_id, cr.version, cr.status, cr.hash
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
        let mut deployments: Vec<HttpApiDeploymentRevisionIdentityRecord> = self.with_ro("get_deployed_http_api_deployments - deployments")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT had.http_api_deployment_id, had.name, hadr.revision_id, hadr.hash
                    FROM http_api_deployments had
                    JOIN http_api_deployment_revisions hadr ON had.http_api_deployment_id = hadr.http_api_deployment_id
                    JOIN deployment_http_api_definition_revisions dhadr
                        ON dhadr.http_api_definition_id = hadr.http_api_deployment_id
                            AND dhadr.http_api_definition_revision_id = hadr.revision_id
                    WHERE dhadr.environment_id = $1 AND dhadr.deployment_revision_id = $2
                    ORDER BY had.host, had.subdomain
                "#})
                    .bind(environment_id)
                    .bind(revision_id),
            )
            .await?;

        for deployment in &mut deployments {
            let definitions = self
                .with_ro("get_deployed_http_api_deployments - definitions")
                .fetch_all(
                    sqlx::query(indoc! { r#"
                        SELECT http_definition_id
                        FROM http_api_deployment_definitions
                        WHERE http_api_deployment_id = $1 AND revision_id = $2
                    "#})
                    .bind(deployment.http_api_deployment_id)
                    .bind(deployment.revision_id),
                )
                .await?;
            deployment.http_api_definitions = definitions
                .iter()
                .map(|row| row.try_get("http_definition_id"))
                .collect::<Result<_, _>>()?;
        }

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
