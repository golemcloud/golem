use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use golem_common::model::AccountId;
use sqlx::{Database, Pool};

use super::RepoError;
use crate::model::Role;

#[async_trait]
pub trait AccountGrantRepo {
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

pub struct DbAccountGrantRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbAccountGrantRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[async_trait]
impl AccountGrantRepo for DbAccountGrantRepo<sqlx::Postgres> {
    async fn get(&self, account_id: &AccountId) -> Result<Vec<Role>, RepoError> {
        let result = sqlx::query_as::<_, AccountGrantRecord>(
            "
            SELECT account_id, role_id
            FROM account_grants
            WHERE account_id = $1
            ",
        )
        .bind(account_id.value.clone())
        .fetch_all(self.db_pool.as_ref())
        .await?;
        result
            .into_iter()
            .map(|r| Role::from_str(&r.role_id).map_err(|e| RepoError::Internal(e.to_string())))
            .collect()
    }

    async fn add(&self, account_id: &AccountId, role: &Role) -> Result<(), RepoError> {
        sqlx::query(
            "
            INSERT INTO account_grants (account_id, role_id)
            VALUES ($1, $2)
            ON CONFLICT (account_id, role_id) DO NOTHING
            ",
        )
        .bind(account_id.value.clone())
        .bind(role.to_string())
        .execute(self.db_pool.as_ref())
        .await?;

        Ok(())
    }

    async fn remove(&self, account_id: &AccountId, role: &Role) -> Result<(), RepoError> {
        sqlx::query(
            "
            DELETE FROM account_grants
            WHERE account_id = $1 AND role_id = $2
            ",
        )
        .bind(account_id.value.clone())
        .bind(role.to_string())
        .execute(self.db_pool.as_ref())
        .await?;

        Ok(())
    }
}

#[async_trait]
impl AccountGrantRepo for DbAccountGrantRepo<sqlx::Sqlite> {
    async fn get(&self, account_id: &AccountId) -> Result<Vec<Role>, RepoError> {
        let result = sqlx::query_as::<_, AccountGrantRecord>(
            "
            SELECT account_id, role_id
            FROM account_grants
            WHERE account_id = $1
            ",
        )
        .bind(account_id.value.clone())
        .fetch_all(self.db_pool.as_ref())
        .await?;
        result
            .into_iter()
            .map(|r| Role::from_str(&r.role_id).map_err(|e| RepoError::Internal(e.to_string())))
            .collect()
    }

    async fn add(&self, account_id: &AccountId, role: &Role) -> Result<(), RepoError> {
        sqlx::query(
            "
            INSERT INTO account_grants (account_id, role_id)
            VALUES ($1, $2)
            ON CONFLICT (account_id, role_id) DO NOTHING
            ",
        )
        .bind(account_id.value.clone())
        .bind(role.to_string())
        .execute(self.db_pool.as_ref())
        .await?;

        Ok(())
    }

    async fn remove(&self, account_id: &AccountId, role: &Role) -> Result<(), RepoError> {
        sqlx::query(
            "
            DELETE FROM account_grants
            WHERE account_id = $1 AND role_id = $2
            ",
        )
        .bind(account_id.value.clone())
        .bind(role.to_string())
        .execute(self.db_pool.as_ref())
        .await?;

        Ok(())
    }
}
