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
    CurrentDeploymentRevisionRecord, DeployError, DeployValidationError, DeploymentRevisionRecord,
    DeploymentStage,
};
use crate::repo::model::hash::SqlBlake3Hash;
use crate::repo::model::http_api_definition::HttpApiDefinitionRevisionIdentityRecord;
use crate::repo::model::http_api_deployment::HttpApiDeploymentRevisionIdentityRecord;
use crate::repo::model::BindFields;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures_util::future::BoxFuture;
use futures_util::FutureExt;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{
    LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi, ToBusiness, TxError,
};
use golem_service_base::repo;
use golem_service_base::repo::RepoError;
use indoc::indoc;
use sqlx::{Database, Row};
use std::collections::HashSet;
use std::fmt::Display;
use tracing::{info_span, Instrument, Span};
use uuid::Uuid;

#[async_trait]
pub trait DeploymentRepo: Send + Sync {
    async fn deploy(
        &self,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        // TODO: current_deployment_revision_id: i64, and make it bit more easy to understand
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
    async fn deploy(
        &self,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        version: String,
        expected_deployment_hash: SqlBlake3Hash,
    ) -> repo::BusinessResult<CurrentDeploymentRevisionRecord, DeployError> {
        self.repo
            .deploy(
                user_account_id,
                environment_id,
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
    async fn deploy(
        &self,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        version: String,
        expected_deployment_hash: SqlBlake3Hash,
    ) -> repo::BusinessResult<CurrentDeploymentRevisionRecord, DeployError> {
        // TODO: match
        let revision_id_row = self
            .with_ro("deploy - get latest revision")
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

        let revision_id: i64 = match revision_id_row {
            Some(row) => row.try_get(0)?,
            None => 1i64,
        };

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

                let stage = Self::get_stage(tx, &environment_id).await?;

                let validation_errors = Self::validate_stage(&stage);
                if !validation_errors.is_empty() {
                    return Err(TxError::Business(DeployError::ValidationErrors(
                        validation_errors,
                    )));
                }

                let diffable_deployment = stage.to_diffable();

                let hash = diffable_deployment.hash();
                if hash.as_blake3_hash() != deployment_revision.hash.as_blake3_hash() {
                    return Err(TxError::Business(DeployError::DeploymentHashMismatch {
                        requested_hash: (*hash.as_blake3_hash()).into(),
                        actual_hash: deployment_revision.hash,
                    }));
                }
                todo!()
            }
            .boxed()
        })
        .await
        .to_business_error_on_unique_violation(|| {
            DeployError::DeploymentRevisionConcurrentModification
        })
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

    async fn get_stage(tx: &mut Self::Tx, environment_id: &Uuid) -> repo::Result<DeploymentStage>;

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

    fn validate_stage(stage: &DeploymentStage) -> Vec<DeployValidationError> {
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

    async fn get_stage(tx: &mut Self::Tx, environment_id: &Uuid) -> repo::Result<DeploymentStage> {
        Ok(DeploymentStage {
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
                SELECT
                    c.component_id as component_id,
                    c.name as name, cr.revision_id as revision_id,
                    cr.revision_id as revision_id,
                    cr.version as version,
                    cr.status as status, cr.hash as hash
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
                SELECT
                    d.http_api_definition_id as http_api_definition_id,
                    d.name as name,
                    dr.revision_id as revision_id,
                    dr.version as version,
                    dr.hash as hash
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
                SELECT
                    d.http_api_deployment_id as http_api_deployment_id,
                    d.name as name,
                    dr.revision_id as revision_id,
                    dr.hash as hash
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
}
