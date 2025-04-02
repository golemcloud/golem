// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
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
use tracing::{debug, error, info};

#[derive(Clone, Debug)]
pub struct SqlitePool {
    pool: sqlx::SqlitePool,
}

impl SqlitePool {
    pub async fn new(pool: sqlx::SqlitePool) -> Result<Self, anyhow::Error> {
        Ok(Self { pool })
    }

    pub async fn configured(config: &DbSqliteConfig) -> Result<Self, anyhow::Error> {
        let pool = SqlitePoolOptions::new()
            .max_connections(config.max_connections)
            .connect_with(config.connect_options())
            .await?;

        SqlitePool::new(pool).await
    }

    pub async fn execute<'a>(
        &self,
        query: Query<'a, Sqlite, SqliteArguments<'a>>,
    ) -> Result<SqliteQueryResult, RepoError> {
        Ok(query.execute(&self.pool).await?)
    }

    pub fn with(&self, svc_name: &'static str, api_name: &'static str) -> SqliteLabelledApi {
        SqliteLabelledApi {
            svc_name,
            api_name,
            pool: self.pool.clone(),
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

    async fn execute<'a>(
        &self,
        query: Query<'a, Sqlite, SqliteArguments<'a>>,
    ) -> Result<Self::QueryResult, RepoError> {
        Ok(self.execute(query).await?)
    }

    fn with(&self, svc_name: &'static str, api_name: &'static str) -> Self::LabelledApi {
        self.with(svc_name, api_name)
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
        self.execute(query).await
    }

    async fn fetch_optional<'a, A>(
        &mut self,
        query: Query<'a, Self::Db, A>,
    ) -> Result<Option<Self::Row>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
    {
        self.fetch_optional(query).await
    }

    async fn fetch_optional_as<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Self::Db, O, A>,
    ) -> Result<Option<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, Self::Row>,
    {
        self.fetch_optional_as(query_as).await
    }

    async fn fetch_all<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Self::Db, O, A>,
    ) -> Result<Vec<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, Self::Row>,
    {
        self.fetch_all(query_as).await
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
                debug!(
                    svc_name = self.svc_name,
                    api_name = self.api_name,
                    duration = end.duration_since(start).as_millis(),
                    "DB query executed successfully"
                );
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
        self.execute(query).await
    }

    async fn fetch_optional<'a, A>(
        &mut self,
        query: Query<'a, Self::Db, A>,
    ) -> Result<Option<Self::Row>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
    {
        self.fetch_optional(query).await
    }

    async fn fetch_optional_as<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Self::Db, O, A>,
    ) -> Result<Option<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, Self::Row>,
    {
        self.fetch_optional_as(query_as).await
    }

    async fn fetch_all<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Self::Db, O, A>,
    ) -> Result<Vec<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, Self::Row>,
    {
        self.fetch_all(query_as).await
    }
}

#[async_trait]
impl super::LabelledPoolApi for SqliteLabelledApi {
    type LabelledTransaction = SqliteLabelledTransaction;

    async fn begin(&self) -> Result<Self::LabelledTransaction, RepoError> {
        self.begin().await
    }

    async fn commit(&self, tx: Self::LabelledTransaction) -> Result<(), RepoError> {
        self.commit(tx).await
    }

    async fn rollback(&self, tx: Self::LabelledTransaction) -> Result<(), RepoError> {
        self.rollback(tx).await
    }
}

pub async fn migrate(
    config: &DbSqliteConfig,
    migrations: impl MigrationSource<'_>,
) -> Result<(), anyhow::Error> {
    info!("DB migration: sqlite://{}", config.database);
    let mut conn = SqliteConnection::connect_with(&config.connect_options()).await?;
    let migrator = sqlx::migrate::Migrator::new(migrations).await?;
    migrator.run(&mut conn).await?;
    let _ = conn.close().await;
    Ok(())
}
