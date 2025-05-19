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

use crate::repo::RepoError;
use async_trait::async_trait;
use golem_common::config::DbSqliteConfig;
use golem_common::metrics::db::{record_db_failure, record_db_success};
use sqlx::migrate::MigrationSource;
use sqlx::query::{Query, QueryAs};
use sqlx::sqlite::{SqliteArguments, SqlitePoolOptions, SqliteQueryResult, SqliteRow};
use sqlx::{Connection, Error, FromRow, IntoArguments, Sqlite, SqliteConnection};
use std::time::Instant;
use tracing::{error, info};

#[derive(Clone, Debug)]
pub struct SqlitePool {
    read_pool: sqlx::SqlitePool,
    write_pool: sqlx::SqlitePool,
}

impl SqlitePool {
    pub fn new(read_pool: sqlx::SqlitePool, write_pool: sqlx::SqlitePool) -> Self {
        Self {
            read_pool,
            write_pool,
        }
    }

    pub async fn configured(config: &DbSqliteConfig) -> Result<Self, anyhow::Error> {
        let read_pool = SqlitePoolOptions::new()
            .min_connections(config.max_connections)
            .max_connections(config.max_connections)
            .connect_with(config.connect_options())
            .await?;
        let write_pool = SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .connect_with(config.connect_options())
            .await?;

        Ok(Self::new(read_pool, write_pool))
    }

    pub fn with_ro(&self, svc_name: &'static str, api_name: &'static str) -> SqliteLabelledApi {
        SqliteLabelledApi {
            svc_name,
            api_name,
            pool: self.read_pool.clone(),
        }
    }

    pub fn with_rw(&self, svc_name: &'static str, api_name: &'static str) -> SqliteLabelledApi {
        SqliteLabelledApi {
            svc_name,
            api_name,
            pool: self.write_pool.clone(),
        }
    }
}

#[async_trait]
impl super::Pool for SqlitePool {
    type LabelledApi = SqliteLabelledApi;
    type LabelledTransaction = SqliteLabelledTransaction;
    type QueryResult = SqliteQueryResult;
    type Db = Sqlite;
    type Args<'a> = SqliteArguments<'a>;

    fn with_ro(&self, svc_name: &'static str, api_name: &'static str) -> Self::LabelledApi {
        SqlitePool::with_ro(self, svc_name, api_name)
    }

    fn with_rw(&self, svc_name: &'static str, api_name: &'static str) -> Self::LabelledApi {
        SqlitePool::with_rw(self, svc_name, api_name)
    }
}

pub struct SqliteLabelledTransaction {
    tx: sqlx::Transaction<'static, Sqlite>,
    start: Instant,
}

impl SqliteLabelledTransaction {
    pub async fn execute<'a>(
        &mut self,
        query: Query<'a, Sqlite, SqliteArguments<'a>>,
    ) -> Result<SqliteQueryResult, RepoError> {
        Ok(query.execute(&mut *self.tx).await?)
    }

    pub async fn fetch_optional<'a, A>(
        &mut self,
        query: Query<'a, Sqlite, A>,
    ) -> Result<Option<SqliteRow>, RepoError>
    where
        A: 'a + IntoArguments<'a, Sqlite>,
    {
        Ok(query.fetch_optional(&mut *self.tx).await?)
    }

    pub async fn fetch_optional_as<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Sqlite, O, A>,
    ) -> Result<Option<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Sqlite>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, SqliteRow>,
    {
        Ok(query_as.fetch_optional(&mut *self.tx).await?)
    }

    pub async fn fetch_all<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Sqlite, O, A>,
    ) -> Result<Vec<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Sqlite>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, SqliteRow>,
    {
        Ok(query_as.fetch_all(&mut *self.tx).await?)
    }

    async fn commit(self) -> Result<(), Error> {
        self.tx.commit().await
    }

    async fn rollback(self) -> Result<(), Error> {
        self.tx.rollback().await
    }
}

#[async_trait]
impl super::PoolApi for SqliteLabelledTransaction {
    type QueryResult = SqliteQueryResult;
    type Row = SqliteRow;
    type Db = Sqlite;
    type Args<'a> = SqliteArguments<'a>;

    async fn execute<'a>(
        &mut self,
        query: Query<'a, Self::Db, SqliteArguments<'a>>,
    ) -> Result<SqliteQueryResult, RepoError> {
        SqliteLabelledTransaction::execute(self, query).await
    }

    async fn fetch_optional<'a, A>(
        &mut self,
        query: Query<'a, Self::Db, A>,
    ) -> Result<Option<Self::Row>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
    {
        SqliteLabelledTransaction::fetch_optional(self, query).await
    }

    async fn fetch_optional_as<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Self::Db, O, A>,
    ) -> Result<Option<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, Self::Row>,
    {
        SqliteLabelledTransaction::fetch_optional_as(self, query_as).await
    }

    async fn fetch_all<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Self::Db, O, A>,
    ) -> Result<Vec<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, Self::Row>,
    {
        SqliteLabelledTransaction::fetch_all(self, query_as).await
    }
}

#[async_trait]
impl super::LabelledPoolTransaction for SqliteLabelledTransaction {}

pub struct SqliteLabelledApi {
    svc_name: &'static str,
    api_name: &'static str,
    pool: sqlx::SqlitePool,
}

impl SqliteLabelledApi {
    pub async fn execute<'a>(
        &self,
        query: Query<'a, Sqlite, SqliteArguments<'a>>,
    ) -> Result<SqliteQueryResult, RepoError> {
        let start = Instant::now();
        self.record(start, query.execute(&self.pool).await)
    }

    pub async fn fetch_optional<'a, A>(
        &self,
        query: Query<'a, Sqlite, A>,
    ) -> Result<Option<SqliteRow>, RepoError>
    where
        A: 'a + IntoArguments<'a, Sqlite>,
    {
        let start = Instant::now();
        self.record(start, query.fetch_optional(&self.pool).await)
    }

    pub async fn fetch_optional_as<'a, O, A>(
        &self,
        query_as: QueryAs<'a, Sqlite, O, A>,
    ) -> Result<Option<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Sqlite>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, SqliteRow>,
    {
        let start = Instant::now();
        self.record(start, query_as.fetch_optional(&self.pool).await)
    }

    pub async fn fetch_all<'a, O, A>(
        &self,
        query_as: QueryAs<'a, Sqlite, O, A>,
    ) -> Result<Vec<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Sqlite>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, SqliteRow>,
    {
        let start = Instant::now();
        self.record(start, query_as.fetch_all(&self.pool).await)
    }

    pub async fn begin(&self) -> Result<SqliteLabelledTransaction, RepoError> {
        let tx = self.pool.begin().await?;
        Ok(SqliteLabelledTransaction {
            tx,
            start: Instant::now(),
        })
    }

    pub async fn commit(&self, tx: SqliteLabelledTransaction) -> Result<(), RepoError> {
        let start = tx.start;
        let result = tx.commit().await;
        self.record(start, result)
    }

    pub async fn rollback(&self, tx: SqliteLabelledTransaction) -> Result<(), RepoError> {
        let start = tx.start;
        let result = tx.rollback().await;
        self.record(start, result)
    }

    fn record<R>(&self, start: Instant, result: Result<R, Error>) -> Result<R, RepoError> {
        let end = Instant::now();
        match result {
            Ok(result) => {
                record_db_success(
                    "sqlite",
                    self.svc_name,
                    self.api_name,
                    end.duration_since(start),
                );
                Ok(result)
            }
            Err(err) => {
                error!(
                    svc_name = self.svc_name,
                    api_name = self.api_name,
                    duration = end.duration_since(start).as_millis(),
                    error = format!("{err:#}"),
                    "DB query failed",
                );
                record_db_failure("sqlite", self.svc_name, self.api_name);
                Err(err.into())
            }
        }
    }
}

#[async_trait]
impl super::PoolApi for SqliteLabelledApi {
    type QueryResult = SqliteQueryResult;
    type Row = SqliteRow;
    type Db = Sqlite;
    type Args<'a> = SqliteArguments<'a>;

    async fn execute<'a>(
        &mut self,
        query: Query<'a, Self::Db, Self::Args<'a>>,
    ) -> Result<Self::QueryResult, RepoError> {
        SqliteLabelledApi::execute(self, query).await
    }

    async fn fetch_optional<'a, A>(
        &mut self,
        query: Query<'a, Self::Db, A>,
    ) -> Result<Option<Self::Row>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
    {
        SqliteLabelledApi::fetch_optional(self, query).await
    }

    async fn fetch_optional_as<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Self::Db, O, A>,
    ) -> Result<Option<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, Self::Row>,
    {
        SqliteLabelledApi::fetch_optional_as(self, query_as).await
    }

    async fn fetch_all<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Self::Db, O, A>,
    ) -> Result<Vec<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, Self::Row>,
    {
        SqliteLabelledApi::fetch_all(self, query_as).await
    }
}

#[async_trait]
impl super::LabelledPoolApi for SqliteLabelledApi {
    type LabelledTransaction = SqliteLabelledTransaction;

    async fn begin(&self) -> Result<Self::LabelledTransaction, RepoError> {
        SqliteLabelledApi::begin(self).await
    }

    async fn commit(&self, tx: Self::LabelledTransaction) -> Result<(), RepoError> {
        SqliteLabelledApi::commit(self, tx).await
    }

    async fn rollback(&self, tx: Self::LabelledTransaction) -> Result<(), RepoError> {
        SqliteLabelledApi::rollback(self, tx).await
    }
}

pub async fn migrate(
    config: &DbSqliteConfig,
    migrations: impl MigrationSource<'_>,
) -> Result<(), anyhow::Error> {
    info!("DB migration: sqlite://{}", config.database);
    let mut conn = SqliteConnection::connect_with(&config.connect_options()).await?;
    let migrator = sqlx::migrate::Migrator::new(migrations).await?;
    // See: https://github.com/launchbadge/sqlx/issues/954, Send required for golem-cli
    futures::executor::block_on(migrator.run(&mut conn))?;
    let _ = conn.close().await;
    Ok(())
}
