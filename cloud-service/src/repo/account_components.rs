use std::ops::Deref;
use std::sync::Arc;

use async_trait::async_trait;
use golem_common::model::AccountId;
use sqlx::{Database, Pool};

use super::RepoError;

#[async_trait]
pub trait AccountComponentsRepo {
    async fn get(&self, id: &AccountId) -> Result<i32, RepoError>;
    async fn update(&self, id: &AccountId, value: i32) -> Result<i32, RepoError>;
    async fn delete(&self, id: &AccountId) -> Result<(), RepoError>;
}

pub struct DbAccountComponentsRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbAccountComponentsRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[derive(sqlx::FromRow)]
struct AccountComponents {
    counter: i32,
}

#[async_trait]
impl AccountComponentsRepo for DbAccountComponentsRepo<sqlx::Postgres> {
    async fn get(&self, id: &AccountId) -> Result<i32, RepoError> {
        sqlx::query_as::<_, AccountComponents>(
            "select counter from account_components where account_id = $1",
        )
        .bind(id.value.clone())
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
        .map(|r| r.map(|r| r.counter).unwrap_or(0))
    }

    async fn update(&self, id: &AccountId, value: i32) -> Result<i32, RepoError> {
        let mut transaction = self.db_pool.begin().await?;

        sqlx::query(
            "
            insert into 
                account_components (account_id, counter)
                values ($1, $2) 
            on conflict (account_id) do update 
            set counter = account_components.counter + $2
            ",
        )
        .bind(id.value.clone())
        .bind(value)
        .execute(&mut *transaction)
        .await?;

        let result = sqlx::query_as::<_, AccountComponents>(
            "select counter from account_components where account_id = $1",
        )
        .bind(id.value.clone())
        .fetch_optional(&mut *transaction)
        .await?;

        transaction.commit().await?;

        Ok(result.map(|r| r.counter).unwrap_or(0))
    }

    async fn delete(&self, id: &AccountId) -> Result<(), RepoError> {
        sqlx::query("delete from account_components where account_id = $1")
            .bind(id.value.clone())
            .execute(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
            .map(|_| ())
    }
}

#[async_trait]
impl AccountComponentsRepo for DbAccountComponentsRepo<sqlx::Sqlite> {
    async fn get(&self, id: &AccountId) -> Result<i32, RepoError> {
        sqlx::query_as::<_, AccountComponents>(
            "select counter from account_components where account_id = $1",
        )
        .bind(id.value.clone())
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
        .map(|r| r.map(|r| r.counter).unwrap_or(0))
    }

    async fn update(&self, id: &AccountId, value: i32) -> Result<i32, RepoError> {
        let mut transaction = self.db_pool.begin().await?;

        sqlx::query(
            "
            insert into
                account_components (account_id, counter)
                values ($1, $2)
            on conflict (account_id) do update
            set counter = account_components.counter + $2
            ",
        )
        .bind(id.value.clone())
        .bind(value)
        .execute(&mut *transaction)
        .await?;

        let result = sqlx::query_as::<_, AccountComponents>(
            "select counter from account_components where account_id = $1",
        )
        .bind(id.value.clone())
        .fetch_optional(&mut *transaction)
        .await?;

        transaction.commit().await?;

        Ok(result.map(|r| r.counter).unwrap_or(0))
    }

    async fn delete(&self, id: &AccountId) -> Result<(), RepoError> {
        sqlx::query("delete from account_components where account_id = $1")
            .bind(id.value.clone())
            .execute(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
            .map(|_| ())
    }
}
