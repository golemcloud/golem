use async_trait::async_trait;
use conditional_trait_gen::{trait_gen, when};
use golem_common::model::AccountId;
use golem_service_base::db::Pool;
use golem_service_base::repo::RepoError;

#[async_trait]
pub trait AccountUploadsRepo {
    async fn get(&self, id: &AccountId) -> Result<i32, RepoError>;
    async fn update(&self, id: &AccountId, value: i32) -> Result<(), RepoError>;
}

pub struct DbAccountUploadsRepo<DB: Pool> {
    db_pool: DB,
}

impl<DB: Pool> DbAccountUploadsRepo<DB> {
    pub fn new(db_pool: DB) -> Self {
        Self { db_pool }
    }
}

#[derive(sqlx::FromRow)]
struct AccountUploads {
    counter: i32,
}

#[trait_gen(golem_service_base::db::postgres::PostgresPool -> golem_service_base::db::postgres::PostgresPool, golem_service_base::db::sqlite::SqlitePool
)]
#[async_trait]
impl AccountUploadsRepo for DbAccountUploadsRepo<golem_service_base::db::postgres::PostgresPool> {
    #[when(golem_service_base::db::postgres::PostgresPool -> get)]
    async fn get_postgres(&self, id: &AccountId) -> Result<i32, RepoError> {
        let query = sqlx::query_as::<_, AccountUploads>(
            "
            select counter
            from account_uploads
            where account_id = $1
                and month = extract(month from get_current_date())
                and year = extract(year from get_current_date())
            ",
        )
        .bind(&id.value);

        let result = self
            .db_pool
            .with_ro("account_uploads", "get")
            .fetch_optional_as(query)
            .await?;

        Ok(result.map(|r| r.counter).unwrap_or_default())
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get)]
    async fn get_sqlite(&self, id: &AccountId) -> Result<i32, RepoError> {
        let query = sqlx::query_as::<_, AccountUploads>(
            "
            SELECT counter
            FROM account_uploads
            WHERE account_id = $1
              AND month = strftime('%m', 'now')
              AND year = strftime('%Y', 'now')
            ",
        )
        .bind(&id.value);

        let result = self
            .db_pool
            .with_ro("account_uploads", "get")
            .fetch_optional_as(query)
            .await?;

        Ok(result.map(|r| r.counter).unwrap_or_default())
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> update)]
    async fn update_postgres(&self, id: &AccountId, value: i32) -> Result<(), RepoError> {
        let query = sqlx::query(
            "
            insert into account_uploads (account_id, counter, month, year)
            VALUES ($1, $2, EXTRACT(MONTH FROM current_date), EXTRACT(YEAR FROM current_date))
            ON CONFLICT DO UPDATE SET counter = counter + $2
        ",
        )
        .bind(&id.value)
        .bind(value);

        self.db_pool
            .with_rw("account_uploads", "update")
            .execute(query)
            .await?;

        Ok(())
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> update)]
    async fn update_sqlite(&self, id: &AccountId, value: i32) -> Result<(), RepoError> {
        let query = sqlx::query(
            "
            insert into account_uploads (account_id, counter, month, year)
            VALUES ($1, $2, strftime('%m', 'now'), strftime('%Y', 'now'))
            ON CONFLICT DO UPDATE SET counter = counter + $2
        ",
        )
        .bind(&id.value)
        .bind(value);

        self.db_pool
            .with_rw("account_uploads", "update")
            .execute(query)
            .await?;

        Ok(())
    }
}
