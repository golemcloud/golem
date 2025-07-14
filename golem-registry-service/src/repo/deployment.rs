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

use crate::repo::model::deployment::CurrentDeploymentRevisionRecord;
use crate::repo::model::hash::SqlBlake3Hash;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures_util::future::BoxFuture;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction, Pool};
use golem_service_base::repo;
use golem_service_base::repo::RepoError;
use sqlx::Database;
use tracing::{info_span, Instrument, Span};
use uuid::Uuid;

#[async_trait]
pub trait DeploymentRepo: Send + Sync {
    async fn deploy(
        &self,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        // TODO: current_deployment_revision_id: i64, and make it bit more easy to understand
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
        expected_deployment_hash: SqlBlake3Hash,
    ) -> repo::Result<CurrentDeploymentRevisionRecord> {
        self.repo
            .deploy(user_account_id, environment_id, expected_deployment_hash)
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
        expected_deployment_hash: SqlBlake3Hash,
    ) -> repo::Result<CurrentDeploymentRevisionRecord> {
        todo!()
    }
}

#[async_trait]
trait DeploymentRepoInternal: DeploymentRepo {
    type Db: Database;
    type Tx: LabelledPoolTransaction;
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl DeploymentRepoInternal for DbDeploymentRepo<PostgresPool> {
    type Db = <PostgresPool as Pool>::Db;
    type Tx = <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction;
}
