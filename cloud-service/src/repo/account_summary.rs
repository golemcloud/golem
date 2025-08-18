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

use crate::model::AccountSummary;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use conditional_trait_gen::{trait_gen, when};
use golem_common::model::AccountId;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo::RepoError;

#[async_trait]
pub trait AccountSummaryRepo: Send + Sync {
    async fn get(&self, skip: i32, limit: i32) -> Result<Vec<AccountSummary>, RepoError>;
    async fn count(&self) -> Result<u64, RepoError>;
}

#[derive(sqlx::FromRow)]
pub struct AccountSummaryRecord {
    id: String,
    name: String,
    email: String,
    components_count: i64,
    workers_count: i64,
    created_at: DateTime<Utc>,
}

impl From<AccountSummaryRecord> for AccountSummary {
    fn from(value: AccountSummaryRecord) -> Self {
        AccountSummary {
            id: AccountId { value: value.id },
            name: value.name,
            email: value.email,
            component_count: value.components_count,
            worker_count: value.workers_count,
            created_at: value.created_at,
        }
    }
}

pub struct DbAccountSummaryRepo<DB: Pool> {
    db_pool: DB,
}

impl<DB: Pool> DbAccountSummaryRepo<DB> {
    pub fn new(db_pool: DB) -> Self {
        Self { db_pool }
    }
}

#[trait_gen(golem_service_base::db::postgres::PostgresPool -> golem_service_base::db::postgres::PostgresPool, golem_service_base::db::sqlite::SqlitePool
)]
#[async_trait]
impl AccountSummaryRepo for DbAccountSummaryRepo<golem_service_base::db::postgres::PostgresPool> {
    #[when(golem_service_base::db::postgres::PostgresPool -> get)]
    async fn get_postgres(&self, skip: i32, limit: i32) -> Result<Vec<AccountSummary>, RepoError> {
        let query = sqlx::query_as::<_, AccountSummaryRecord>(
          "
          SELECT a.id, a.name, a.email, COALESCE(ac.counter, 0::bigint) AS components_count, COALESCE(aw.counter, 0::bigint) AS workers_count, t.created_at::timestamptz
          FROM accounts a
          JOIN (SELECT min(t.created_at) AS created_at, t.account_id FROM tokens t GROUP BY t.account_id) t ON t.account_id = a.id
          LEFT JOIN project_account pa ON pa.owner_account_id = a.id
          LEFT JOIN account_components ac ON ac.account_id = a.id
          LEFT JOIN account_workers aw ON aw.account_id = a.id
          GROUP BY a.id, a.name, a.email, t.created_at, ac.counter, aw.counter, a.deleted
          ORDER BY t.created_at DESC, a.id DESC
         LIMIT $1
         OFFSET $2
          ",
      )
      .bind(limit)
      .bind(skip);

        let result = self
            .db_pool
            .with_ro("account_summary", "get")
            .fetch_all(query)
            .await?;

        Ok(result.into_iter().map(|r| r.into()).collect())
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get)]
    async fn get_sqlite(&self, skip: i32, limit: i32) -> Result<Vec<AccountSummary>, RepoError> {
        let query = sqlx::query_as::<_, AccountSummaryRecord>(
            "
          SELECT a.id, a.name, a.email, CAST(IFNULL(ac.counter, 0) AS bigint) AS components_count, CAST(IFNULL(aw.counter, 0) AS bigint) AS workers_count, t.created_at
          FROM accounts a
          JOIN (SELECT min(t.created_at) AS created_at, t.account_id FROM tokens t GROUP BY t.account_id) t ON t.account_id = a.id
          LEFT JOIN project_account pa ON pa.owner_account_id = a.id
          LEFT JOIN account_components ac ON ac.account_id = a.id
          LEFT JOIN account_workers aw ON aw.account_id = a.id
          GROUP BY a.id, a.name, a.email, t.created_at, ac.counter, aw.counter, a.deleted
          ORDER BY t.created_at DESC, a.id DESC
         LIMIT $1
         OFFSET $2
          ",
        )
            .bind(limit)
            .bind(skip);

        let result = self
            .db_pool
            .with_ro("account_summary", "get")
            .fetch_all(query)
            .await?;

        Ok(result.into_iter().map(|r| r.into()).collect())
    }

    async fn count(&self) -> Result<u64, RepoError> {
        let query = sqlx::query_as::<_, (i64,)>("SELECT count(*) FROM accounts");

        let result = self
            .db_pool
            .with_ro("account_summary", "get")
            .fetch_one_as(query)
            .await?;

        Ok(result.0 as u64)
    }
}
