use async_trait::async_trait;
use conditional_trait_gen::{trait_gen, when};
use golem_common::model::AccountId;
use golem_service_base::db::Pool;
use golem_service_base::repo::RepoError;

#[async_trait]
pub trait AccountFuelRepo {
    async fn get(&self, id: &AccountId) -> Result<i64, RepoError>;
    async fn update(&self, id: &AccountId, delta: i64) -> Result<(), RepoError>;
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
    #[when(golem_service_base::db::postgres::PostgresPool -> get)]
    async fn get_postgres(&self, id: &AccountId) -> Result<i64, RepoError> {
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

    #[when(golem_service_base::db::sqlite::SqlitePool -> get)]
    async fn get_sqlite(&self, id: &AccountId) -> Result<i64, RepoError> {
        let query = sqlx::query_as::<_, AccountFuel>(
            "
            SELECT consumed
            FROM account_fuel
            WHERE account_id = $1
              AND month = strftime('%m', 'now')
              AND year = strftime('%Y', 'now')
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
    async fn update_postgres(&self, id: &AccountId, delta: i64) -> Result<(), RepoError> {
        let query = sqlx::query(
            "
            INSERT INTO account_fuel (account_id, consumed, month, year)
            VALUES ($1, $2, EXTRACT(MONTH FROM current_date), EXTRACT(YEAR FROM current_date))
            ON CONFLICT DO UPDATE SET consumed = consumed + $2
            ",
        )
        .bind(&id.value)
        .bind(delta);

        self.db_pool
            .with_rw("account_fuel", "update")
            .execute(query)
            .await?;

        Ok(())
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> update)]
    async fn update_sqlite(&self, id: &AccountId, delta: i64) -> Result<(), RepoError> {
        let query = sqlx::query(
            "
            INSERT INTO account_fuel (account_id, consumed, month, year)
            VALUES ($1, 0, strftime('%m', 'now'), strftime('%Y', 'now'))
            ON CONFLICT DO UPDATE SET consumed = consumed + $2
            ",
        )
        .bind(&id.value)
        .bind(delta);

        self.db_pool
            .with_rw("account_fuel", "update")
            .execute(query)
            .await?;

        Ok(())
    }
}
