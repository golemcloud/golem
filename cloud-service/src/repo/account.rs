use std::ops::Deref;
use std::result::Result;
use std::sync::Arc;

use async_trait::async_trait;
use cloud_common::model::PlanId;
use sqlx::{Database, Pool};
use uuid::Uuid;

use crate::model::Account;
use crate::repo::RepoError;

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
pub trait AccountRepo {
    async fn create(&self, account: &AccountRecord) -> Result<Option<AccountRecord>, RepoError>;

    async fn update(&self, account: &AccountRecord) -> Result<AccountRecord, RepoError>;

    async fn get(&self, account_id: &str) -> Result<Option<AccountRecord>, RepoError>;

    async fn get_all(&self) -> Result<Vec<AccountRecord>, RepoError>;

    async fn delete(&self, account_id: &str) -> Result<(), RepoError>;
}

pub struct DbAccountRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbAccountRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[async_trait]
impl AccountRepo for DbAccountRepo<sqlx::Postgres> {
    async fn create(&self, account: &AccountRecord) -> Result<Option<AccountRecord>, RepoError> {
        let result = sqlx::query(
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
        .bind(account.plan_id)
        .execute(self.db_pool.deref())
        .await;

        match result {
            Ok(_) => Ok(Some(account.clone())),
            Err(sqlx::Error::Database(error)) if error.is_unique_violation() => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn update(&self, account: &AccountRecord) -> Result<AccountRecord, RepoError> {
        sqlx::query(
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
        .bind(account.plan_id)
        .execute(self.db_pool.deref())
        .await?;

        Ok(account.clone())
    }

    async fn get(&self, account_id: &str) -> Result<Option<AccountRecord>, RepoError> {
        sqlx::query_as::<_, AccountRecord>(
            "SELECT * FROM accounts WHERE id = $1 AND deleted = false",
        )
        .bind(account_id)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_all(&self) -> Result<Vec<AccountRecord>, RepoError> {
        sqlx::query_as::<_, AccountRecord>("SELECT * FROM accounts WHERE deleted = false")
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn delete(&self, account_id: &str) -> Result<(), RepoError> {
        sqlx::query("UPDATE accounts SET deleted = true WHERE id = $1")
            .bind(account_id)
            .execute(self.db_pool.deref())
            .await?;
        Ok(())
    }
}

#[async_trait]
impl AccountRepo for DbAccountRepo<sqlx::Sqlite> {
    async fn create(&self, account: &AccountRecord) -> Result<Option<AccountRecord>, RepoError> {
        let result = sqlx::query(
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
        .bind(account.plan_id)
        .execute(self.db_pool.deref())
        .await;

        match result {
            Ok(_) => Ok(Some(account.clone())),
            Err(sqlx::Error::Database(error)) if error.is_unique_violation() => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn update(&self, account: &AccountRecord) -> Result<AccountRecord, RepoError> {
        sqlx::query(
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
        .bind(account.plan_id)
        .execute(self.db_pool.deref())
        .await?;

        Ok(account.clone())
    }

    async fn get(&self, account_id: &str) -> Result<Option<AccountRecord>, RepoError> {
        sqlx::query_as::<_, AccountRecord>(
            "SELECT * FROM accounts WHERE id = $1 AND deleted = false",
        )
        .bind(account_id)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_all(&self) -> Result<Vec<AccountRecord>, RepoError> {
        sqlx::query_as::<_, AccountRecord>("SELECT * FROM accounts WHERE deleted = false")
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn delete(&self, account_id: &str) -> Result<(), RepoError> {
        sqlx::query("UPDATE accounts SET deleted = true WHERE id = $1")
            .bind(account_id)
            .execute(self.db_pool.deref())
            .await?;
        Ok(())
    }
}
