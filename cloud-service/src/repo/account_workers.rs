use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_common::model::AccountId;
use golem_service_base::db::Pool;
use golem_service_base::repo::RepoError;

#[async_trait]
pub trait AccountWorkersRepo {
    async fn get(&self, id: &AccountId) -> Result<i32, RepoError>;
    async fn update(&self, id: &AccountId, value: i32) -> Result<i32, RepoError>;
    async fn delete(&self, id: &AccountId) -> Result<(), RepoError>;
}

pub struct DbAccountWorkerRepo<DB: Pool> {
    db_pool: DB,
}

impl<DB: Pool> DbAccountWorkerRepo<DB> {
    pub fn new(db_pool: DB) -> Self {
        Self { db_pool }
    }
}

#[derive(sqlx::FromRow)]
struct AccountWorkers {
    counter: i32,
}

#[trait_gen(golem_service_base::db::postgres::PostgresPool -> golem_service_base::db::postgres::PostgresPool, golem_service_base::db::sqlite::SqlitePool
)]
#[async_trait]
impl AccountWorkersRepo for DbAccountWorkerRepo<golem_service_base::db::postgres::PostgresPool> {
    async fn get(&self, id: &AccountId) -> Result<i32, RepoError> {
        let query = sqlx::query_as::<_, AccountWorkers>(
            "select counter from account_workers where account_id = $1",
        )
        .bind(id.value.clone());

        self.db_pool
            .with_ro("account_workers", "get")
            .fetch_optional_as(query)
            .await
            .map(|r| r.map(|r| r.counter).unwrap_or_default())
    }

    async fn update(&self, id: &AccountId, value: i32) -> Result<i32, RepoError> {
        let mut transaction = self
            .db_pool
            .with_rw("account_workers", "update")
            .begin()
            .await?;

        let query = sqlx::query(
            "
            insert into 
                account_workers (account_id, counter)
                values ($1, $2) 
            on conflict (account_id) do update 
            set counter = account_workers.counter + $2
            ",
        )
        .bind(id.value.clone())
        .bind(value);

        transaction.execute(query).await?;

        let query = sqlx::query_as::<_, AccountWorkers>(
            "select counter from account_workers where account_id = $1",
        )
        .bind(id.value.clone());

        let result = transaction.fetch_optional_as(query).await?;

        self.db_pool
            .with_rw("account_workers", "update")
            .commit(transaction)
            .await?;

        Ok(result.map(|r| r.counter).unwrap_or_default())
    }

    async fn delete(&self, id: &AccountId) -> Result<(), RepoError> {
        let query =
            sqlx::query("delete from account_workers where account_id = $1").bind(id.value.clone());

        self.db_pool
            .with_rw("account_workers", "delete")
            .execute(query)
            .await?;

        Ok(())
    }
}
