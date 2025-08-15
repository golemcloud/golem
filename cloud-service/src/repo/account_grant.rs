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
use golem_common::model::auth::Role;
use golem_common::model::AccountId;
use golem_service_base::db::Pool;
use golem_service_base::repo::RepoError;
use std::str::FromStr;

#[async_trait]
pub trait AccountGrantRepo: Send + Sync {
    async fn get(&self, account_id: &AccountId) -> Result<Vec<Role>, RepoError>;
    async fn add(&self, account_id: &AccountId, role: &Role) -> Result<(), RepoError>;
    async fn remove(&self, account_id: &AccountId, role: &Role) -> Result<(), RepoError>;
}

#[allow(dead_code)]
#[derive(sqlx::FromRow)]
struct AccountGrantRecord {
    account_id: String,
    role_id: String,
}

pub struct DbAccountGrantRepo<DB: Pool> {
    db_pool: DB,
}

impl<DB: Pool> DbAccountGrantRepo<DB> {
    pub fn new(db_pool: DB) -> Self {
        Self { db_pool }
    }
}

#[trait_gen(golem_service_base::db::postgres::PostgresPool -> golem_service_base::db::postgres::PostgresPool, golem_service_base::db::sqlite::SqlitePool
)]
#[async_trait]
impl AccountGrantRepo for DbAccountGrantRepo<golem_service_base::db::postgres::PostgresPool> {
    async fn get(&self, account_id: &AccountId) -> Result<Vec<Role>, RepoError> {
        let query = sqlx::query_as::<_, AccountGrantRecord>(
            "
            SELECT account_id, role_id
            FROM account_grants
            WHERE account_id = $1
            ",
        )
        .bind(account_id.value.clone());

        let result = self
            .db_pool
            .with_ro("account_grants", "get")
            .fetch_all(query)
            .await?;

        result
            .into_iter()
            .map(|r| Role::from_str(&r.role_id).map_err(|e| RepoError::Internal(e.to_string())))
            .collect()
    }

    async fn add(&self, account_id: &AccountId, role: &Role) -> Result<(), RepoError> {
        let query = sqlx::query(
            "
            INSERT INTO account_grants (account_id, role_id)
            VALUES ($1, $2)
            ON CONFLICT (account_id, role_id) DO NOTHING
            ",
        )
        .bind(account_id.value.clone())
        .bind(role.to_string());

        self.db_pool
            .with_rw("account_grants", "add")
            .execute(query)
            .await?;

        Ok(())
    }

    async fn remove(&self, account_id: &AccountId, role: &Role) -> Result<(), RepoError> {
        let query = sqlx::query(
            "
            DELETE FROM account_grants
            WHERE account_id = $1 AND role_id = $2
            ",
        )
        .bind(account_id.value.clone())
        .bind(role.to_string());

        self.db_pool
            .with_rw("account_grants", "remove")
            .execute(query)
            .await?;

        Ok(())
    }
}
