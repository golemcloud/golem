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

use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_common::model::AccountId;
use golem_service_base::db::Pool;
use golem_service_base::repo::RepoError;

#[async_trait]
pub trait AccountConnectionsRepo: Send + Sync {
    async fn get(&self, account_id: &AccountId) -> Result<i32, RepoError>;

    async fn update(&self, account_id: &AccountId, value: i32) -> Result<i32, RepoError>;
}

pub struct DbAccountConnectionsRepo<DB: Pool> {
    db_pool: DB,
}

impl<DB: Pool> DbAccountConnectionsRepo<DB> {
    pub fn new(db_pool: DB) -> Self {
        Self { db_pool }
    }
}

#[derive(sqlx::FromRow)]
struct AccountConnections {
    counter: i32,
}

#[trait_gen(golem_service_base::db::postgres::PostgresPool -> golem_service_base::db::postgres::PostgresPool, golem_service_base::db::sqlite::SqlitePool
)]
#[async_trait]
impl AccountConnectionsRepo
    for DbAccountConnectionsRepo<golem_service_base::db::postgres::PostgresPool>
{
    async fn get(&self, account_id: &AccountId) -> Result<i32, RepoError> {
        let query = sqlx::query_as::<_, AccountConnections>(
            "select counter from account_connections where account_id = $1",
        )
        .bind(&account_id.value);

        self.db_pool
            .with_ro("account_connections", "get")
            .fetch_optional_as(query)
            .await
            .map(|r| r.map(|r| r.counter).unwrap_or_default())
    }

    async fn update(&self, account_id: &AccountId, value: i32) -> Result<i32, RepoError> {
        let mut transaction = self
            .db_pool
            .with_rw("account_connections", "update")
            .begin()
            .await?;

        let query = sqlx::query(
            "
          insert into account_connections
            (account_id, counter)
          values
            ($1, $2)
          on conflict (account_id) do update
          set counter = account_connections.counter + $2
        ",
        )
        .bind(account_id.value.clone())
        .bind(value);

        transaction.execute(query).await?;

        let query = sqlx::query_as::<_, AccountConnections>(
            "select counter from account_connections where account_id = $1",
        )
        .bind(account_id.value.clone());

        let result = transaction.fetch_optional_as(query).await?;

        self.db_pool
            .with_rw("account_connections", "update")
            .commit(transaction)
            .await?;

        Ok(result.map(|r| r.counter).unwrap_or_default())
    }
}
