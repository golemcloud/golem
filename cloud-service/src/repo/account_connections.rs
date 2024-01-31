use std::ops::Deref;
use std::sync::Arc;

use async_trait::async_trait;
use golem_common::model::AccountId;
use sqlx::{Database, Pool};

use crate::repo::RepoError;

#[async_trait]
pub trait AccountConnectionsRepo {
    async fn get(&self, account_id: &AccountId) -> Result<i32, RepoError>;

    async fn update(&self, account_id: &AccountId, value: i32) -> Result<i32, RepoError>;
}

pub struct DbAccountConnectionsRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbAccountConnectionsRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[derive(sqlx::FromRow)]
struct AccountConnections {
    counter: i32,
}

#[async_trait]
impl AccountConnectionsRepo for DbAccountConnectionsRepo<sqlx::Postgres> {
    async fn get(&self, account_id: &AccountId) -> Result<i32, RepoError> {
        let result = sqlx::query_as::<_, AccountConnections>(
            "select counter from account_connections where account_id = $1",
        )
        .bind(account_id.value.clone())
        .fetch_optional(self.db_pool.deref())
        .await?;

        Ok(result.map(|r| r.counter).unwrap_or(0))
    }

    async fn update(&self, account_id: &AccountId, value: i32) -> Result<i32, RepoError> {
        let mut transaction = self.db_pool.begin().await?;

        sqlx::query(
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
        .bind(value)
        .execute(&mut *transaction)
        .await?;

        let result = sqlx::query_as::<_, AccountConnections>(
            "select counter from account_connections where account_id = $1",
        )
        .bind(account_id.value.clone())
        .fetch_optional(self.db_pool.deref())
        .await?;

        transaction.commit().await?;

        Ok(result.map(|r| r.counter).unwrap_or(0))
    }
}

#[async_trait]
impl AccountConnectionsRepo for DbAccountConnectionsRepo<sqlx::Sqlite> {
    async fn get(&self, account_id: &AccountId) -> Result<i32, RepoError> {
        let result = sqlx::query_as::<_, AccountConnections>(
            "select counter from account_connections where account_id = $1",
        )
        .bind(account_id.value.clone())
        .fetch_optional(self.db_pool.deref())
        .await?;

        Ok(result.map(|r| r.counter).unwrap_or(0))
    }

    async fn update(&self, account_id: &AccountId, value: i32) -> Result<i32, RepoError> {
        let mut transaction = self.db_pool.begin().await?;

        sqlx::query(
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
        .bind(value)
        .execute(&mut *transaction)
        .await?;

        let result = sqlx::query_as::<_, AccountConnections>(
            "select counter from account_connections where account_id = $1",
        )
        .bind(account_id.value.clone())
        .fetch_optional(self.db_pool.deref())
        .await?;

        transaction.commit().await?;

        Ok(result.map(|r| r.counter).unwrap_or(0))
    }
}
