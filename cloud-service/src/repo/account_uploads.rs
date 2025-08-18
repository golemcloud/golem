// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use async_trait::async_trait;
use conditional_trait_gen::{trait_gen, when};
use golem_common::model::AccountId;
use golem_service_base::db::Pool;
use golem_service_base::repo::RepoError;

#[async_trait]
pub trait AccountUploadsRepo: Send + Sync {
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
        let mut transaction = self
            .db_pool
            .with_rw("account_uploads", "update")
            .begin()
            .await?;

        // The logic here is very subtle. account_id is the primary key alone here,
        // if we have a previous month's data the transaction will overwrite it.
        //
        // Ignore the weird 1, 2000 month and year, they will be overwritten by the second query.
        let query = sqlx::query(
            "
            INSERT INTO account_uploads (account_id, counter, month, year)
            VALUES ($1, 0, 1, 2000)
            ON CONFLICT (account_id) DO NOTHING
        ",
        )
        .bind(&id.value);

        transaction.execute(query).await?;

        let query = sqlx::query(
            "
            UPDATE account_uploads
            SET counter = CASE
                WHEN month = EXTRACT(MONTH FROM current_date) AND year = EXTRACT(YEAR FROM current_date)
                THEN counter + $2
                ELSE $2
            END,
            month = EXTRACT(MONTH FROM current_date),
            year = EXTRACT(YEAR FROM current_date)
            WHERE account_id = $1
            ",
        )
        .bind(&id.value)
        .bind(value);

        transaction.execute(query).await?;

        self.db_pool
            .with_rw("account_uploads", "update")
            .commit(transaction)
            .await?;

        Ok(())
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> update)]
    async fn update_sqlite(&self, id: &AccountId, value: i32) -> Result<(), RepoError> {
        let mut transaction = self
            .db_pool
            .with_rw("account_uploads", "update")
            .begin()
            .await?;

        // The logic here is very subtle. account_id is the primary key alone here,
        // if we have a previous month's data the transaction will overwrite it.
        //
        // Ignore the weird 1, 2000 month and year, they will be overwritten by the second query.
        let query = sqlx::query(
            "
            INSERT INTO account_uploads (account_id, counter, month, year)
            VALUES ($1, 0, 1, 2000)
            ON CONFLICT (account_id) DO NOTHING
        ",
        )
        .bind(&id.value);

        transaction.execute(query).await?;

        let query = sqlx::query(
            "
            UPDATE account_uploads
            SET counter = CASE
                WHEN month = strftime('%m', 'now') AND year = strftime('%Y', 'now')
                THEN counter + $2
                ELSE $2
            END,
            month = strftime('%m', 'now'),
            year = strftime('%Y', 'now')
            WHERE account_id = $1
            ",
        )
        .bind(&id.value)
        .bind(value);

        transaction.execute(query).await?;

        self.db_pool
            .with_rw("account_uploads", "update")
            .commit(transaction)
            .await?;

        Ok(())
    }
}
