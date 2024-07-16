use std::sync::Arc;

use async_trait::async_trait;
use golem_common::model::AccountId;
use sqlx::{Database, Pool};

use super::RepoError;

#[async_trait]
pub trait AccountFuelRepo {
    async fn get(&self, id: &AccountId) -> Result<i64, RepoError>;
    async fn update(&self, id: &AccountId, delta: i64) -> Result<i64, RepoError>;
}

#[derive(sqlx::FromRow)]
struct AccountFuel {
    consumed: i64,
}

pub struct DbAccountFuelRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbAccountFuelRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[async_trait]
impl AccountFuelRepo for DbAccountFuelRepo<sqlx::Postgres> {
    async fn get(&self, id: &AccountId) -> Result<i64, RepoError> {
        let result = sqlx::query_as::<_, AccountFuel>(
            "
            SELECT consumed
            FROM account_fuel
            WHERE account_id = $1
              AND month = EXTRACT(MONTH FROM current_date)
              AND year = EXTRACT(YEAR FROM current_date)
            ",
        )
        .bind(id.value.clone())
        .fetch_optional(self.db_pool.as_ref())
        .await?;

        Ok(result.map(|r| r.consumed).unwrap_or(0))
    }

    async fn update(&self, id: &AccountId, delta: i64) -> Result<i64, RepoError> {
        let mut transaction = self.db_pool.begin().await?;

        sqlx::query(
            "
            INSERT INTO account_fuel (account_id, consumed, month, year)
            VALUES ($1, 0, 1, 2000)
            ON CONFLICT DO NOTHING
            ",
        )
        .bind(id.value.clone())
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "
            UPDATE account_fuel
            SET consumed = CASE
                WHEN month = EXTRACT(MONTH FROM current_date) AND year = EXTRACT(YEAR FROM current_date)
                THEN consumed + $2
                ELSE $2
            END,
            month = EXTRACT(MONTH FROM current_date),
            year = EXTRACT(YEAR FROM current_date)
            WHERE account_id = $1
            ",
        )
        .bind(id.value.clone())
        .bind(delta)
        .execute(&mut *transaction)
        .await?;

        // Should we use get?
        let updated_fuel = sqlx::query_as::<_, AccountFuel>(
            "SELECT consumed FROM account_fuel WHERE account_id = $1",
        )
        .bind(id.value.clone())
        .fetch_one(&mut *transaction)
        .await?;

        transaction.commit().await?;

        Ok(updated_fuel.consumed)
    }
}

#[async_trait]
impl AccountFuelRepo for DbAccountFuelRepo<sqlx::Sqlite> {
    async fn get(&self, id: &AccountId) -> Result<i64, RepoError> {
        let result = sqlx::query_as::<_, AccountFuel>(
            "
            SELECT consumed
            FROM account_fuel
            WHERE account_id = $1
              AND month = EXTRACT(MONTH FROM current_date)
              AND year = EXTRACT(YEAR FROM current_date)
            ",
        )
        .bind(id.value.clone())
        .fetch_optional(self.db_pool.as_ref())
        .await?;

        Ok(result.map(|r| r.consumed).unwrap_or(0))
    }

    async fn update(&self, id: &AccountId, delta: i64) -> Result<i64, RepoError> {
        let mut transaction = self.db_pool.begin().await?;

        sqlx::query(
            "
            WITH upsert AS (
                UPDATE account_fuel
                SET consumed = CASE
                    WHEN month = EXTRACT(MONTH FROM current_date) AND year = EXTRACT(YEAR FROM current_date)
                    THEN consumed + $2
                    ELSE $2
                END,
                month = EXTRACT(MONTH FROM current_date),
                year = EXTRACT(YEAR FROM current_date)
                WHERE account_id = $1
                RETURNING *
            )
            INSERT INTO account_fuel (account_id, consumed, month, year)
            SELECT $1, $2, EXTRACT(MONTH FROM current_date), EXTRACT(YEAR FROM current_date)
            WHERE NOT EXISTS (SELECT * FROM upsert)
            ",
        )
            .bind(id.value.clone())
            .bind(delta)
            .execute(&mut *transaction)
            .await?;

        // Should we use get?
        let updated_fuel = sqlx::query_as::<_, AccountFuel>(
            "SELECT consumed FROM account_fuel WHERE account_id = $1",
        )
        .bind(id.value.clone())
        .fetch_one(&mut *transaction)
        .await?;

        transaction.commit().await?;

        Ok(updated_fuel.consumed)
    }
}
