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

use crate::model::diff::Hashable;
use crate::repo::model::audit::RevisionAuditFields;
use crate::repo::model::component::ComponentRevisionIdentityRecord;
use crate::repo::model::deployment::{
    CurrentDeploymentRevisionRecord, DeployError, DeployValidationError,
    DeployedDeploymentIdentity, DeploymentIdentity, DeploymentRevisionRecord,
};
use crate::repo::model::hash::SqlBlake3Hash;
use crate::repo::model::http_api_definition::HttpApiDefinitionRevisionIdentityRecord;
use crate::repo::model::http_api_deployment::HttpApiDeploymentRevisionIdentityRecord;
use crate::repo::model::BindFields;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::future::BoxFuture;
use futures::FutureExt;
use golem_service_base_next::db::postgres::PostgresPool;
use golem_service_base_next::db::sqlite::SqlitePool;
use golem_service_base_next::db::{
    LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi, ToBusiness, TxError,
};
use golem_service_base_next::repo;
use golem_service_base_next::repo::RepoError;
use indoc::indoc;
use sqlx::{Database, Row};
use std::collections::HashSet;
use std::fmt::Display;
use tracing::{info_span, Instrument, Span};
use uuid::Uuid;

#[async_trait]
pub trait DeploymentRepo: Send + Sync {
    async fn get_deployed_revision(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Option<DeploymentRevisionRecord>>;

    async fn get_staged_identity(&self, environment_id: &Uuid) -> repo::Result<DeploymentIdentity>;

    async fn get_deployed_identity(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Option<DeployedDeploymentIdentity>>;

    async fn deploy(
        &self,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        current_staged_deployment_revision_id: Option<i64>,
        version: String,
        expected_deployment_hash: SqlBlake3Hash,
    ) -> repo::BusinessResult<CurrentDeploymentRevisionRecord, DeployError>;
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
    ) -> repo::Result<Option<DeploymentRevisionRecord>> {
        self.repo
            .get_deployed_revision(environment_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn get_staged_identity(&self, environment_id: &Uuid) -> repo::Result<DeploymentIdentity> {
        self.repo
            .get_staged_identity(environment_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn get_deployed_identity(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Option<DeployedDeploymentIdentity>> {
        self.repo
            .get_deployed_identity(environment_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn deploy(
        &self,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        current_staged_deployment_revision_id: Option<i64>,
        version: String,
        expected_deployment_hash: SqlBlake3Hash,
    ) -> repo::BusinessResult<CurrentDeploymentRevisionRecord, DeployError> {
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
}

pub struct DbDeploymentRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "deployment";

impl<DBP: Pool> DbDeploymentRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }

    async fn with_tx_err<R, E, F>(&self, api_name: &'static str, f: F) -> Result<R, TxError<E>>
    where
        R: Send,
        E: Display + Send,
        F: for<'f> FnOnce(
                &'f mut <DBP::LabelledApi as LabelledPoolApi>::LabelledTransaction,
            ) -> BoxFuture<'f, Result<R, TxError<E>>>
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
    ) -> repo::Result<Option<DeploymentRevisionRecord>> {
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

    async fn get_staged_identity(&self, environment_id: &Uuid) -> repo::Result<DeploymentIdentity> {
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

    async fn get_deployed_identity(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Option<DeployedDeploymentIdentity>> {
        let Some(deployment_revision) = self.get_deployed_revision(environment_id).await? else {
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
    ) -> repo::BusinessResult<CurrentDeploymentRevisionRecord, DeployError> {
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
                Some(row) => Some(row.try_get(0)?),
                None => None,
            };

        if current_staged_deployment_revision_id != actual_current_staged_revision_id {
            return Ok(Err(DeployError::DeploymentConcurrentRevisionCreation));
        };

        let revision_id = current_staged_deployment_revision_id.unwrap_or(-1) + 1;

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

                let validation_errors = Self::validate_stage(&staged_deployment);
                if !validation_errors.is_empty() {
                    return Err(TxError::Business(DeployError::ValidationErrors(
                        validation_errors,
                    )));
                }

                let diffable_deployment = staged_deployment.to_diffable();

                let hash = diffable_deployment.hash();
                if hash.as_blake3_hash() != deployment_revision.hash.as_blake3_hash() {
                    return Err(TxError::Business(DeployError::DeploymentHashMismatch {
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
                )
                .await?;

                Ok(revision)
            }
            .boxed()
        })
        .await
        .to_business_error_on_unique_violation(|| DeployError::DeploymentConcurrentRevisionCreation)
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
    ) -> Result<DeploymentRevisionRecord, RepoError>;

    async fn get_staged_deployment(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
    ) -> repo::Result<DeploymentIdentity>;

    async fn get_staged_components(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
    ) -> Result<Vec<ComponentRevisionIdentityRecord>, RepoError>;

    async fn get_staged_http_api_definitions(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
    ) -> Result<Vec<HttpApiDefinitionRevisionIdentityRecord>, RepoError>;

    async fn get_staged_http_api_deployments(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
    ) -> Result<Vec<HttpApiDeploymentRevisionIdentityRecord>, RepoError>;

    fn validate_stage(stage: &DeploymentIdentity) -> Vec<DeployValidationError> {
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

        // TODO: validate api def constraints on components

        errors
    }

    async fn create_deployment_relations(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
        deployment_revision_id: i64,
        stage: &DeploymentIdentity,
    ) -> repo::Result<()>;

    async fn create_deployment_component_revision(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
        deployment_revision_id: i64,
        component: &ComponentRevisionIdentityRecord,
    ) -> repo::Result<()>;

    async fn create_deployment_http_api_definition_revision(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
        deployment_revision_id: i64,
        http_api_definition: &HttpApiDefinitionRevisionIdentityRecord,
    ) -> repo::Result<()>;

    async fn create_deployment_http_api_deployment_revision(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
        deployment_revision_id: i64,
        http_api_deployment: &HttpApiDeploymentRevisionIdentityRecord,
    ) -> repo::Result<()>;

    async fn set_current_deployment(
        tx: &mut Self::Tx,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        deployment_revision_id: i64,
    ) -> Result<CurrentDeploymentRevisionRecord, RepoError>;

    async fn get_deployed_components(
        &self,
        environment_id: &Uuid,
        revision_id: i64,
    ) -> Result<Vec<ComponentRevisionIdentityRecord>, RepoError>;

    async fn get_deployed_http_api_definitions(
        &self,
        environment_id: &Uuid,
        revision_id: i64,
    ) -> Result<Vec<HttpApiDefinitionRevisionIdentityRecord>, RepoError>;

    async fn get_deployed_http_api_deployments(
        &self,
        environment_id: &Uuid,
        revision_id: i64,
    ) -> Result<Vec<HttpApiDeploymentRevisionIdentityRecord>, RepoError>;
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
    ) -> Result<DeploymentRevisionRecord, RepoError> {
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
    ) -> repo::Result<DeploymentIdentity> {
        Ok(DeploymentIdentity {
            components: Self::get_staged_components(tx, environment_id).await?,
            http_api_definitions: Self::get_staged_http_api_definitions(tx, environment_id).await?,
            http_api_deployments: Self::get_staged_http_api_deployments(tx, environment_id).await?,
        })
    }

    async fn get_staged_components(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
    ) -> Result<Vec<ComponentRevisionIdentityRecord>, RepoError> {
        tx.fetch_all_as(
            sqlx::query_as(indoc! { r#"
                SELECT c.component_id, c.name, cr.revision_id, cr.revision_id, cr.version, cr.status, cr.hash
                FROM components c
                INNER JOIN component_revisions cr ON
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
    ) -> Result<Vec<HttpApiDefinitionRevisionIdentityRecord>, RepoError> {
        tx.fetch_all_as(
            sqlx::query_as(indoc! { r#"
                SELECT d.http_api_definition_id, d.name, dr.revision_id, dr.version, dr.hash
                FROM http_api_definitions d
                INNER JOIN http_api_definition_revisions dr ON
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
    ) -> Result<Vec<HttpApiDeploymentRevisionIdentityRecord>, RepoError> {
        let mut deployments: Vec<HttpApiDeploymentRevisionIdentityRecord> = tx
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT d.http_api_deployment_id, d.name, dr.revision_id, dr.hash
                    FROM http_api_deployments d
                    INNER JOIN http_api_deployment_revisions dr ON
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
                .map(|row| row.try_get(0))
                .collect::<Result<_, _>>()?;
        }

        Ok(deployments)
    }

    async fn create_deployment_relations(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
        deployment_revision_id: i64,
        stage: &DeploymentIdentity,
    ) -> repo::Result<()> {
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
    ) -> repo::Result<()> {
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
    ) -> repo::Result<()> {
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
    ) -> repo::Result<()> {
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
    ) -> Result<CurrentDeploymentRevisionRecord, RepoError> {
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
                    (environment_id, revision_id, created_at, created_by, current_revision_id)
                    VALUES ($1, $2, $3, $4, $5)
                    RETURNING environment_id, revision_id, created_at, created_by, current_revision_id
                "#})
                    .bind(environment_id)
                    .bind(revision_id)
                    .bind_revision_audit(RevisionAuditFields::new(*user_account_id)),
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
    ) -> Result<Vec<ComponentRevisionIdentityRecord>, RepoError> {
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
    ) -> Result<Vec<HttpApiDefinitionRevisionIdentityRecord>, RepoError> {
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
    ) -> Result<Vec<HttpApiDeploymentRevisionIdentityRecord>, RepoError> {
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
                .map(|row| row.try_get(0))
                .collect::<Result<_, _>>()?;
        }

        Ok(deployments)
    }
}
