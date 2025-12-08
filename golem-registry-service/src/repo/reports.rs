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

use super::model::reports::{AccountCountsRecord, AccountSummaryRecord};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo::RepoResult;
use indoc::indoc;
use tracing::{Instrument, info_span};

#[async_trait]
pub trait ReportsRepo: Send + Sync {
    async fn list_account_summaries(&self) -> RepoResult<Vec<AccountSummaryRecord>>;

    async fn get_account_counts(&self) -> RepoResult<AccountCountsRecord>;
}

pub struct LoggedReportsRepo<Repo: ReportsRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "reports repository";

impl<Repo: ReportsRepo> LoggedReportsRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }
}

#[async_trait]
impl<Repo: ReportsRepo> ReportsRepo for LoggedReportsRepo<Repo> {
    async fn list_account_summaries(&self) -> RepoResult<Vec<AccountSummaryRecord>> {
        self.repo
            .list_account_summaries()
            .instrument(info_span!(SPAN_NAME))
            .await
    }

    async fn get_account_counts(&self) -> RepoResult<AccountCountsRecord> {
        self.repo
            .get_account_counts()
            .instrument(info_span!(SPAN_NAME))
            .await
    }
}

pub struct DbReportsRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "reports";

impl<DBP: Pool> DbReportsRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self {
            db_pool: db_pool.clone(),
        }
    }

    pub fn logged(db_pool: DBP) -> LoggedReportsRepo<Self>
    where
        Self: ReportsRepo,
    {
        LoggedReportsRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl ReportsRepo for DbReportsRepo<PostgresPool> {
    async fn list_account_summaries(&self) -> RepoResult<Vec<AccountSummaryRecord>> {
        let result = self
            .with_ro("list_account_summaries")
            .fetch_all_as(sqlx::query_as(indoc! { r#"
                    SELECT
                        a.account_id, a.email, a.created_at,
                        r.name,
                        (
                            SELECT CAST(COUNT(1) AS NUMERIC)
                            FROM applications ap
                            JOIN environments e
                                ON e.application_id = ap.application_id
                            JOIN components c
                                ON c.environment_id = e.environment_id
                            WHERE
                                ap.account_id = a.account_id
                                AND ap.deleted_at IS NULL
                                AND e.deleted_at IS NULL
                                AND c.deleted_at IS NULL
                        ) as components_count,
                        COALESCE((
                            SELECT value
                            FROM account_usage_stats aus
                            WHERE
                                aus.account_id = a.account_id
                                AND aus.usage_type = 3 -- TotalWorkerCount
                                AND aus.usage_key = 'total'
                        ), 0) as workers_count
                    FROM accounts a
                    JOIN account_revisions r
                        ON r.account_id = a.account_id
                        AND r.revision_id = a.current_revision_id
                    WHERE
                        a.deleted_at IS NULL
                "# }))
            .await?;

        Ok(result)
    }

    async fn get_account_counts(&self) -> RepoResult<AccountCountsRecord> {
        let result = self
            .with_ro("get_account_counts")
            .fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        (SELECT CAST(COUNT(1) AS NUMERIC) FROM accounts a) as total_accounts,
                        (SELECT CAST(COUNT(1) AS NUMERIC) FROM accounts a WHERE a.deleted_at IS NULL) as total_active_accounts,
                        (SELECT CAST(COUNT(1) AS NUMERIC) FROM accounts a WHERE a.deleted_at IS NOT NULL) as total_deleted_accounts
                "# })
            )
            .await?;

        Ok(result)
    }
}
