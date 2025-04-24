use async_trait::async_trait;
use conditional_trait_gen::{trait_gen, when};
use golem_common::model::AccountId;
use golem_service_base::db::Pool;
use golem_service_base::repo::RepoError;

#[async_trait]
pub trait AccountUploadsRepo {
    async fn get(&self, id: &AccountId) -> Result<i32, RepoError>;
    async fn update(&self, id: &AccountId, value: i32) -> Result<i32, RepoError>;
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
        .bind(id.value.clone());

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
        .bind(id.value.clone());

        let result = self
            .db_pool
            .with_ro("account_uploads", "get")
            .fetch_optional_as(query)
            .await?;

        Ok(result.map(|r| r.counter).unwrap_or_default())
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> update)]
    async fn update_postgres(&self, id: &AccountId, value: i32) -> Result<i32, RepoError> {
        let mut transaction = self
            .db_pool
            .with_rw("account_uploads", "update")
            .begin()
            .await?;

        let query = sqlx::query(
            "
            insert into account_uploads (account_id, counter, month, year)
            values ($1, 0, 1, 2000)
            on conflict do nothing
        ",
        )
        .bind(id.value.clone());

        transaction.execute(query).await?;

        let query = sqlx::query("
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
        .bind(value);

        transaction.execute(query).await?;

        // Why don't we use get function?
        let query = sqlx::query_as::<_, AccountUploads>(
            "
            select counter
            from account_uploads
            where account_id = $1
            ",
        )
        .bind(id.value.clone());

        let new_counter = transaction.fetch_optional_as(query).await?;

        self.db_pool
            .with("account_uploads", "update")
            .commit(transaction)
            .await?;

        Ok(new_counter.map(|r| r.counter).unwrap_or_default())
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> update)]
    async fn update_sqlite(&self, id: &AccountId, value: i32) -> Result<i32, RepoError> {
        let mut transaction = self
            .db_pool
            .with_rw("account_uploads", "update")
            .begin()
            .await?;

        let query = sqlx::query("
            UPDATE account_uploads
            SET counter = CASE
                WHEN month = strftime('%m', 'now') AND year = strftime('%Y', 'now')
                THEN counter + $2
                ELSE $2
                END,
                month = strftime('%m', 'now'),
                year = strftime('%Y', 'now')
            WHERE account_id = $1;

            INSERT INTO account_uploads (account_id, counter, month, year)
            SELECT $1, $2, strftime('%m', 'now'), strftime('%Y', 'now')
            WHERE NOT EXISTS (SELECT 1 FROM account_uploads WHERE account_id = $1 AND month = strftime('%m', 'now') AND year = strftime('%Y', 'now'));
        ")
            .bind(id.value.clone())
            .bind(value);

        transaction.execute(query).await?;

        // Why don't we use get function?
        let query = sqlx::query_as::<_, AccountUploads>(
            "
            select counter
            from account_uploads
            where account_id = $1
            ",
        )
        .bind(id.value.clone());

        let new_counter = transaction.fetch_optional_as(query).await?;

        Ok(new_counter.map(|r| r.counter).unwrap_or_default())
    }
}
