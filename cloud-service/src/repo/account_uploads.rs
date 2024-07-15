use std::ops::Deref;
use std::sync::Arc;

use async_trait::async_trait;
use golem_common::model::AccountId;
use sqlx::{Database, Pool};

use super::RepoError;

#[async_trait]
pub trait AccountUploadsRepo {
    async fn get(&self, id: &AccountId) -> Result<i32, RepoError>;
    async fn update(&self, id: &AccountId, value: i32) -> Result<i32, RepoError>;
}

pub struct DbAccountUploadsRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbAccountUploadsRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[derive(sqlx::FromRow)]
struct AccountUploads {
    counter: i32,
}

#[async_trait]
impl AccountUploadsRepo for DbAccountUploadsRepo<sqlx::Postgres> {
    async fn get(&self, id: &AccountId) -> Result<i32, RepoError> {
        let result = sqlx::query_as::<_, AccountUploads>(
            "
            select counter
            from account_uploads
            where account_id = $1
                and month = extract(month from get_current_date())
                and year = extract(year from get_current_date())
            ",
        )
        .bind(id.value.clone())
        .fetch_optional(self.db_pool.deref())
        .await?;

        Ok(result.map(|r| r.counter).unwrap_or(0))
    }

    async fn update(&self, id: &AccountId, value: i32) -> Result<i32, RepoError> {
        let mut transaction = self.db_pool.begin().await?;

        sqlx::query(
            "
            insert into account_uploads (account_id, counter, month, year)
            values ($1, 0, 1, 2000)
            on conflict do nothing
        ",
        )
        .bind(id.value.clone())
        .execute(&mut *transaction)
        .await?;

        sqlx::query("
            update account_uploads
            set counter = case
                when month = extract(month from get_current_date()) and year = extract(year from get_current_date())
                    then counter + $2
                else $2
                end,
            month = extract(month from get_current_date()),
            year = extract(year from get_current_date())
            where account_id = $1
        ")
        .bind(id.value.clone())
        .bind(value)
        .execute(&mut *transaction)
        .await?;

        // Why don't we use get function?
        let new_counter = sqlx::query_as::<_, AccountUploads>(
            "
            select counter
            from account_uploads
            where account_id = $1
            ",
        )
        .bind(id.value.clone())
        .fetch_optional(&mut *transaction)
        .await?;

        transaction.commit().await?;

        Ok(new_counter.map(|r| r.counter).unwrap_or(0))
    }
}

#[async_trait]
impl AccountUploadsRepo for DbAccountUploadsRepo<sqlx::Sqlite> {
    async fn get(&self, id: &AccountId) -> Result<i32, RepoError> {
        let result = sqlx::query_as::<_, AccountUploads>(
            "
            select counter
            from account_uploads
            where account_id = $1
                and month = extract(month from get_current_date())
                and year = extract(year from get_current_date())
            ",
        )
        .bind(id.value.clone())
        .fetch_optional(self.db_pool.deref())
        .await?;

        Ok(result.map(|r| r.counter).unwrap_or(0))
    }

    async fn update(&self, id: &AccountId, value: i32) -> Result<i32, RepoError> {
        sqlx::query("
            with upsert as (
                update account_uploads
                set counter = case
                when month = extract(month from get_current_date()) and year = extract(year from get_current_date())
                    then counter + $2
                else $2
                end,
                month = extract(month from get_current_date()),
                year = extract(year from get_current_date())
                where account_id = $1
                returning *
            )
            insert into account_uploads (account_id, counter, month, year)
            select $1, $2, extract(month from get_current_date()), extract(year from get_current_date())
            where not exists (select * from upsert)
        ")
            .bind(id.value.clone())
            .bind(value)
            .execute(self.db_pool.deref())
            .await?;

        // Why don't we use get function?
        let new_counter = sqlx::query_as::<_, AccountUploads>(
            "
            select counter
            from account_uploads
            where account_id = $1
            ",
        )
        .bind(id.value.clone())
        .fetch_optional(self.db_pool.deref())
        .await?;

        Ok(new_counter.map(|r| r.counter).unwrap_or(0))
    }
}
