use async_trait::async_trait;
use conditional_trait_gen::{trait_gen, when};
use futures_util::{future, TryFutureExt};
use golem_common::model::AccountId;
use golem_service_base::db::Pool;
use golem_service_base::repo::RepoError;
use sqlx::Error;

#[async_trait]
pub trait AccountFuelRepo {
    async fn get(&self, id: &AccountId) -> Result<i64, RepoError>;
    async fn update(&self, id: &AccountId, delta: i64) -> Result<i64, RepoError>;
}

#[derive(sqlx::FromRow)]
struct AccountFuel {
    consumed: i64,
}

pub struct DbAccountFuelRepo<DB: Pool> {
    db_pool: DB,
}

impl<DB: Pool> DbAccountFuelRepo<DB> {
    pub fn new(db_pool: DB) -> Self {
        Self { db_pool }
    }
}

#[trait_gen(golem_service_base::db::postgres::PostgresPool -> golem_service_base::db::postgres::PostgresPool, golem_service_base::db::sqlite::SqlitePool
)]
#[async_trait]
impl AccountFuelRepo for DbAccountFuelRepo<golem_service_base::db::postgres::PostgresPool> {
    async fn get(&self, id: &AccountId) -> Result<i64, RepoError> {
        let query = sqlx::query_as::<_, AccountFuel>(
            "
            SELECT consumed
            FROM account_fuel
            WHERE account_id = $1
              AND month = EXTRACT(MONTH FROM current_date)
              AND year = EXTRACT(YEAR FROM current_date)
            ",
        )
        .bind(&id.value);

        self.db_pool
            .with_ro("account_fuel", "get")
            .fetch_optional_as(query)
            .await
            .map(|r| r.map(|r| r.consumed).unwrap_or_default())
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> update)]
    async fn update_postgres(&self, id: &AccountId, delta: i64) -> Result<i64, RepoError> {
        let mut transaction = self
            .db_pool
            .with_rw("account_fuel", "update")
            .begin()
            .await?;

        let query = sqlx::query(
            "
            INSERT INTO account_fuel (account_id, consumed, month, year)
            VALUES ($1, 0, 1, 2000)
            ON CONFLICT DO NOTHING
            ",
        )
        .bind(&id.value);

        transaction.execute(query).await?;

        let query = sqlx::query(
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
        .bind(&id.value)
        .bind(delta);

        transaction.execute(query).await?;

        let query = sqlx::query_as::<_, AccountFuel>(
            "SELECT consumed FROM account_fuel WHERE account_id = $1",
        )
        .bind(&id.value);

        // TODO: use fetch_one_as
        let updated_fuel = transaction
            .fetch_optional_as(query)
            .and_then(|row| match row {
                Some(row) => future::ok(row),
                None => future::err(Error::RowNotFound.into()),
            })
            .await?;

        self.db_pool
            .with_rw("account_fuel", "update")
            .commit(transaction)
            .await?;

        Ok(updated_fuel.consumed)
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> update)]
    async fn update_sqlite(&self, id: &AccountId, delta: i64) -> Result<i64, RepoError> {
        let mut transaction = self
            .db_pool
            .with_rw("account_fuel", "update")
            .begin()
            .await?;

        let query = sqlx::query(
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
            .bind(&id.value)
            .bind(delta);

        transaction.execute(query).await?;

        let query = sqlx::query_as::<_, AccountFuel>(
            "SELECT consumed FROM account_fuel WHERE account_id = $1",
        )
        .bind(&id.value);

        // TODO: use fetch_one_as
        let updated_fuel = transaction
            .fetch_optional_as(query)
            .and_then(|row| match row {
                Some(row) => future::ok(row),
                None => future::err(Error::RowNotFound.into()),
            })
            .await?;

        self.db_pool
            .with_rw("account_fuel", "update")
            .commit(transaction)
            .await?;

        Ok(updated_fuel.consumed)
    }
}
