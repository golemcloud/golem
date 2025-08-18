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

use crate::model::Account;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_common::model::PlanId;
use golem_service_base::db::Pool;
use golem_service_base::repo::RepoError;
use sqlx::QueryBuilder;
use std::result::Result;
use uuid::Uuid;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct AccountRecord {
    pub id: String,
    pub name: String,
    pub email: String,
    pub plan_id: Uuid,
}

impl From<AccountRecord> for Account {
    fn from(value: AccountRecord) -> Self {
        Self {
            id: value.id.as_str().into(),
            name: value.name,
            email: value.email,
            plan_id: PlanId(value.plan_id),
        }
    }
}

impl From<Account> for AccountRecord {
    fn from(value: Account) -> Self {
        Self {
            id: value.id.value,
            name: value.name,
            email: value.email,
            plan_id: value.plan_id.0,
        }
    }
}

#[async_trait]
pub trait AccountRepo: Send + Sync {
    async fn create(&self, account: &AccountRecord) -> Result<Option<AccountRecord>, RepoError>;

    async fn update(&self, account: &AccountRecord) -> Result<AccountRecord, RepoError>;

    async fn get(&self, account_id: &str) -> Result<Option<AccountRecord>, RepoError>;

    async fn find_all(&self, email: Option<&str>) -> Result<Vec<AccountRecord>, RepoError>;

    async fn find(
        &self,
        accounts: &[String],
        email: Option<&str>,
    ) -> Result<Vec<AccountRecord>, RepoError>;

    async fn get_all(&self) -> Result<Vec<AccountRecord>, RepoError>;

    async fn delete(&self, account_id: &str) -> Result<(), RepoError>;
}

pub struct DbAccountRepo<DB: Pool> {
    db_pool: DB,
}

impl<DB: Pool> DbAccountRepo<DB> {
    pub fn new(db_pool: DB) -> Self {
        Self { db_pool }
    }
}

#[trait_gen(golem_service_base::db::postgres::PostgresPool -> golem_service_base::db::postgres::PostgresPool, golem_service_base::db::sqlite::SqlitePool
)]
#[async_trait]
impl AccountRepo for DbAccountRepo<golem_service_base::db::postgres::PostgresPool> {
    async fn create(&self, account: &AccountRecord) -> Result<Option<AccountRecord>, RepoError> {
        let query = sqlx::query(
            r#"
              INSERT INTO accounts
                (id, name, email, plan_id)
              VALUES
                ($1, $2, $3, $4)
            "#,
        )
        .bind(account.id.clone())
        .bind(account.name.clone())
        .bind(account.email.clone())
        .bind(account.plan_id);

        let result = self
            .db_pool
            .with_rw("account", "create")
            .execute(query)
            .await;

        match result {
            Ok(_) => Ok(Some(account.clone())),
            Err(RepoError::UniqueViolation(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn update(&self, account: &AccountRecord) -> Result<AccountRecord, RepoError> {
        let query = sqlx::query(
            r#"
              INSERT INTO accounts
                (id, name, email, plan_id)
              VALUES
                ($1, $2, $3, $4)
              ON CONFLICT (id) DO UPDATE
              SET name = $2,
                  email = $3,
                  plan_id = $4
              WHERE accounts.deleted = false
            "#,
        )
        .bind(account.id.clone())
        .bind(account.name.clone())
        .bind(account.email.clone())
        .bind(account.plan_id);

        self.db_pool
            .with_rw("account", "update")
            .execute(query)
            .await?;

        Ok(account.clone())
    }

    async fn get(&self, account_id: &str) -> Result<Option<AccountRecord>, RepoError> {
        let query = sqlx::query_as::<_, AccountRecord>(
            "SELECT * FROM accounts WHERE id = $1 AND deleted = false",
        )
        .bind(account_id);

        self.db_pool
            .with_ro("account", "get")
            .fetch_optional_as(query)
            .await
    }

    async fn find_all(&self, email: Option<&str>) -> Result<Vec<AccountRecord>, RepoError> {
        let mut query = QueryBuilder::new("SELECT * FROM accounts a WHERE deleted = false");
        if let Some(email) = email {
            query.push(" AND a.email = ");
            query.push_bind(email);
        };

        self.db_pool
            .with_ro("account", "find_all")
            .fetch_all(query.build_query_as::<AccountRecord>())
            .await
    }

    async fn find(
        &self,
        accounts: &[String],
        email: Option<&str>,
    ) -> Result<Vec<AccountRecord>, RepoError> {
        let mut query = QueryBuilder::new("SELECT * FROM accounts a WHERE id IN (");
        {
            let mut in_list = query.separated(", ");
            for account in accounts {
                in_list.push_bind(account);
            }
            in_list.push_unseparated(") ");
        }
        query.push("AND deleted = false");

        if let Some(email) = email {
            query.push(" AND a.email = ");
            query.push_bind(email);
        };

        self.db_pool
            .with_ro("account", "find")
            .fetch_all(query.build_query_as::<AccountRecord>())
            .await
    }

    async fn get_all(&self) -> Result<Vec<AccountRecord>, RepoError> {
        let query =
            sqlx::query_as::<_, AccountRecord>("SELECT * FROM accounts WHERE deleted = false");

        self.db_pool
            .with_ro("account", "get_all")
            .fetch_all(query)
            .await
    }

    async fn delete(&self, account_id: &str) -> Result<(), RepoError> {
        let query =
            sqlx::query("UPDATE accounts SET deleted = true WHERE id = $1").bind(account_id);

        self.db_pool
            .with_rw("account", "delete")
            .execute(query)
            .await?;

        Ok(())
    }
}
