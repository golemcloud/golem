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

use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo::RepoResult;
use indoc::indoc;
use tracing::Instrument;
use tracing::info_span;

/// The one row of the `installation` table: what the installation as a whole owns, as opposed to
/// any account, application or environment.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct InstallationRecord {
    pub golem_issuer: String,
}

#[async_trait]
pub trait InstallationRepo: Send + Sync {
    /// The stored Golem issuer, if the installation has one.
    async fn get(&self) -> RepoResult<Option<InstallationRecord>>;

    /// Stores `golem_issuer` unless the installation already has one, in one atomic statement,
    /// and returns the stored record: two callers that race end with the same value.
    async fn store_golem_issuer_if_absent(
        &self,
        golem_issuer: &str,
    ) -> RepoResult<InstallationRecord>;
}

pub struct LoggedInstallationRepo<Repo: InstallationRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "installation repository";

impl<Repo: InstallationRepo> LoggedInstallationRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }
}

#[async_trait]
#[golem_async_trait::region_simple]
impl<Repo: InstallationRepo> InstallationRepo for LoggedInstallationRepo<Repo> {
    async fn get(&self) -> RepoResult<Option<InstallationRecord>> {
        self.repo.get().instrument(info_span!(SPAN_NAME)).await
    }

    async fn store_golem_issuer_if_absent(
        &self,
        golem_issuer: &str,
    ) -> RepoResult<InstallationRecord> {
        self.repo
            .store_golem_issuer_if_absent(golem_issuer)
            .instrument(info_span!(SPAN_NAME))
            .await
    }
}

pub struct DbInstallationRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "installation";

impl<DBP: Pool> DbInstallationRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedInstallationRepo<Self>
    where
        Self: InstallationRepo,
    {
        LoggedInstallationRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }

    fn with_rw(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_rw(METRICS_SVC_NAME, api_name)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
#[golem_async_trait::region_simple]
impl InstallationRepo for DbInstallationRepo<PostgresPool> {
    async fn get(&self) -> RepoResult<Option<InstallationRecord>> {
        self.with_ro("get")
            .fetch_optional_as(sqlx::query_as(indoc! { r#"
                SELECT golem_issuer
                FROM installation
                WHERE installation_id = 1
            "#}))
            .await
    }

    async fn store_golem_issuer_if_absent(
        &self,
        golem_issuer: &str,
    ) -> RepoResult<InstallationRecord> {
        // The no-op update makes the statement return the stored row when one exists. A second
        // writer waits for the first one's commit, so every caller gets the winner's value.
        self.with_rw("store_golem_issuer_if_absent")
            .fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    INSERT INTO installation (installation_id, golem_issuer)
                    VALUES (1, $1)
                    ON CONFLICT (installation_id)
                    DO UPDATE SET installation_id = excluded.installation_id
                    RETURNING golem_issuer
                "#})
                .bind(golem_issuer),
            )
            .await
    }
}
