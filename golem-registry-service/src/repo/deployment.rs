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

use crate::model::diff;
use crate::model::diff::Hashable;
use crate::repo::model::audit::RevisionAuditFields;
use crate::repo::model::deployment::{
    ComponentRevisionForDeploymentRecord, CurrentDeploymentRevisionRecord, DeploymentRevisionRecord,
};
use crate::repo::model::hash::SqlBlake3Hash;
use crate::repo::model::BindFields;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures_util::future::BoxFuture;
use futures_util::FutureExt;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi};
use golem_service_base::repo;
use golem_service_base::repo::RepoError;
use indoc::indoc;
use sqlx::{Database, Row};
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
    ) -> repo::Result<CurrentDeploymentRevisionRecord>;
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
    ) -> repo::Result<CurrentDeploymentRevisionRecord> {
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

    async fn with_tx<F, R>(&self, api_name: &'static str, f: F) -> Result<R, RepoError>
    where
        R: Send,
        F: for<'f> FnOnce(
                &'f mut <DBP::LabelledApi as LabelledPoolApi>::LabelledTransaction,
            ) -> BoxFuture<'f, Result<R, RepoError>>
            + Send,
    {
        self.db_pool.with_tx(METRICS_SVC_NAME, api_name, f).await
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
    ) -> repo::Result<CurrentDeploymentRevisionRecord> {
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

        self.with_tx("deploy", |tx| {
            async move {
                // TODO: if env requires check version uniqueness

                let deployment_revision = Self::create_deployment_revision(
                    tx,
                    user_account_id,
                    environment_id,
                    revision_id,
                    version,
                    expected_deployment_hash,
                )
                .await?;

                let components = Self::get_components(tx, &environment_id).await?;
                // TODO: validate component state and existence of hashes

                let diff_deployment = diff::Deployment {
                    components: components
                        .into_iter()
                        .map(|component| {
                            (
                                component.name,
                                diff::HashOf::<diff::Component>::from_blake3_hash(
                                    component.hash.expect("TODO").into(),
                                ),
                            )
                        })
                        .collect(),
                    http_api_definitions: todo!(),
                    http_api_deployments: todo!(),
                };

                let hash = diff_deployment.hash();
                if hash.as_blake3_hash() != deployment_revision.hash.as_blake3_hash() {
                    // TODO: rollback with user error
                }
                todo!()
            }
            .boxed()
        })
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
    ) -> Result<DeploymentRevisionRecord, RepoError>;

    async fn get_components(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
    ) -> Result<Vec<ComponentRevisionForDeploymentRecord>, RepoError>;
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

    async fn get_components(
        tx: &mut Self::Tx,
        environment_id: &Uuid,
    ) -> Result<Vec<ComponentRevisionForDeploymentRecord>, RepoError> {
        tx.fetch_all(
            sqlx::query_as(indoc! { r#"
                SELECT c.component_id as component_id, c.name as name, cr.status as status, cr.hash as hash
                FROM components c
                LEFT JOIN component_revisions cr ON
                    cr.component_id = c.component_id AND cr.revision_id = c.current_revision_id
                WHERE c.environment_id = $1 AND c.deleted_at IS NULL
            "#})
                .bind(environment_id)
        ).await
    }
}
