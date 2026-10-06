// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
use golem_common::metrics::db::{record_db_failure, record_db_success, record_db_transaction};
use sqlx::migrate::MigrationSource;
use sqlx::query::{Query, QueryAs};
use sqlx::sqlite::{SqliteArguments, SqlitePoolOptions, SqliteQueryResult, SqliteRow};
use sqlx::{Connection, Error, FromRow, IntoArguments, Sqlite, SqliteConnection};
use std::time::Instant;
use tracing::{error, info, warn};

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
            .min_connections(0)
            .max_connections(config.max_connections)
            .connect_with(config.connect_options())
            .await?;
        let write_pool = SqlitePoolOptions::new()
            .min_connections(0)
            .max_connections(1)
            .connect_with(config.connect_options())
            .await?;

        Ok(Self::new(read_pool, write_pool))
    }

    /// Pins a read connection for SQLite incremental BLOB I/O.
    pub(crate) async fn acquire_blob_reader(
        &self,
    ) -> Result<sqlx::pool::PoolConnection<Sqlite>, Error> {
        self.read_pool.acquire().await
    }

    pub fn with_ro(&self, svc_name: &'static str, api_name: &'static str) -> SqliteLabelledApi {
        SqliteLabelledApi {
            svc_name,
            api_name,
            pool: self.read_pool.clone(),
            write_transaction: false,
        }
    }

    pub fn with_rw(&self, svc_name: &'static str, api_name: &'static str) -> SqliteLabelledApi {
        SqliteLabelledApi {
            svc_name,
            api_name,
            pool: self.write_pool.clone(),
            write_transaction: true,
        }
    }
}

#[async_trait]
impl super::Pool for SqlitePool {
    type LabelledApi = SqliteLabelledApi;
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
    svc_name: &'static str,
    api_name: &'static str,
    tx: sqlx::Transaction<'static, Sqlite>,
    start: Instant,
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
        Ok(query.execute(&mut *self.tx).await?)
    }

    async fn fetch_optional<'a, A>(
        &mut self,
        query: Query<'a, Self::Db, A>,
    ) -> Result<Option<Self::Row>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
    {
        Ok(query.fetch_optional(&mut *self.tx).await?)
    }

    async fn fetch_optional_as<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Self::Db, O, A>,
    ) -> Result<Option<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, Self::Row>,
    {
        Ok(query_as.fetch_optional(&mut *self.tx).await?)
    }

    async fn fetch_all<'a, A>(
        &mut self,
        query: Query<'a, Self::Db, A>,
    ) -> Result<Vec<SqliteRow>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
    {
        Ok(query.fetch_all(&mut *self.tx).await?)
    }

    async fn fetch_all_as<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Self::Db, O, A>,
    ) -> Result<Vec<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, Self::Row>,
    {
        Ok(query_as.fetch_all(&mut *self.tx).await?)
    }
}

#[async_trait]
impl super::LabelledPoolTransaction for SqliteLabelledTransaction {
    async fn commit(self) -> Result<(), RepoError> {
        let svc_name = self.svc_name;
        let api_name = self.api_name;
        let start = self.start;
        let result = self.tx.commit().await;
        record_db_transaction("sqlite", svc_name, api_name, start.elapsed());
        SqliteLabelledApi::record(svc_name, api_name, start, result)
    }

    async fn rollback(self) -> Result<(), RepoError> {
        let svc_name = self.svc_name;
        let api_name = self.api_name;
        let start = self.start;
        warn!(
            svc_name = svc_name,
            api_name = api_name,
            "DB transaction rollback",
        );
        golem_common::metrics::db::record_db_rollback("sqlite", svc_name, api_name);
        let result = self.tx.rollback().await;
        record_db_transaction("sqlite", svc_name, api_name, start.elapsed());
        SqliteLabelledApi::record(svc_name, api_name, start, result)
    }
}

pub struct SqliteLabelledApi {
    svc_name: &'static str,
    api_name: &'static str,
    pool: sqlx::SqlitePool,
    write_transaction: bool,
}

impl SqliteLabelledApi {
    fn record<R>(
        svc_name: &'static str,
        api_name: &'static str,
        start: Instant,
        result: Result<R, Error>,
    ) -> Result<R, RepoError> {
        let end = Instant::now();
        match result {
            Ok(result) => {
                record_db_success("sqlite", svc_name, api_name, end.duration_since(start));
                Ok(result)
            }
            Err(err) => {
                error!(
                    svc_name = svc_name,
                    api_name = api_name,
                    duration = end.duration_since(start).as_millis(),
                    error = format!("{err:#}"),
                    "DB query failed",
                );
                record_db_failure("sqlite", svc_name, api_name);
                Err(err.into())
            }
        }
    }

    fn record_self<R>(&self, start: Instant, result: Result<R, Error>) -> Result<R, RepoError> {
        Self::record(self.svc_name, self.api_name, start, result)
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
        let start = Instant::now();
        self.record_self(start, query.execute(&self.pool).await)
    }

    async fn fetch_optional<'a, A>(
        &mut self,
        query: Query<'a, Self::Db, A>,
    ) -> Result<Option<Self::Row>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
    {
        let start = Instant::now();
        self.record_self(start, query.fetch_optional(&self.pool).await)
    }

    async fn fetch_optional_as<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Self::Db, O, A>,
    ) -> Result<Option<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, Self::Row>,
    {
        let start = Instant::now();
        self.record_self(start, query_as.fetch_optional(&self.pool).await)
    }

    async fn fetch_all<'a, A>(
        &mut self,
        query: Query<'a, Self::Db, A>,
    ) -> Result<Vec<Self::Row>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
    {
        let start = Instant::now();
        self.record_self(start, query.fetch_all(&self.pool).await)
    }

    async fn fetch_all_as<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Self::Db, O, A>,
    ) -> Result<Vec<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, Self::Row>,
    {
        let start = Instant::now();
        self.record_self(start, query_as.fetch_all(&self.pool).await)
    }
}

#[async_trait]
impl super::LabelledPoolApi for SqliteLabelledApi {
    type LabelledTransaction = SqliteLabelledTransaction;

    async fn begin(&self) -> Result<Self::LabelledTransaction, RepoError> {
        let tx = if self.write_transaction {
            // Acquire SQLite's write reservation before any reads in the transaction. A deferred
            // transaction can lose a cross-process lock-upgrade race with SQLITE_BUSY immediately;
            // BEGIN IMMEDIATE instead lets the configured busy timeout wait for the current writer.
            self.pool.begin_with("BEGIN IMMEDIATE").await?
        } else {
            self.pool.begin().await?
        };
        Ok(SqliteLabelledTransaction {
            svc_name: self.svc_name,
            api_name: self.api_name,
            tx,
            start: Instant::now(),
        })
    }
}

pub async fn migrate(
    config: &DbSqliteConfig,
    migrations: impl MigrationSource<'_>,
) -> Result<(), anyhow::Error> {
    info!("DB migration: sqlite://{}", config.database);
    let mut conn = SqliteConnection::connect_with(&config.connect_options()).await?;
    let migrator = sqlx::migrate::Migrator::new(migrations).await?;
    migrator.run_direct(&mut conn).await?;

    let _ = conn.close().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{LabelledPoolApi, LabelledPoolTransaction};
    use std::time::Duration;

    #[test_r::test]
    async fn write_transactions_wait_for_a_writer_from_another_pool() {
        let tempdir = tempfile::tempdir().unwrap();
        let config = DbSqliteConfig {
            database: tempdir
                .path()
                .join("shared.db")
                .to_string_lossy()
                .into_owned(),
            max_connections: 1,
            foreign_keys: false,
        };
        let first = SqlitePool::configured(&config).await.unwrap();
        let second = SqlitePool::configured(&config).await.unwrap();

        let first_tx = first.with_rw("test", "first").begin().await.unwrap();
        let second_tx =
            tokio::spawn(async move { second.with_rw("test", "second").begin().await.unwrap() });

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !second_tx.is_finished(),
            "a second write transaction acquired the shared SQLite file"
        );

        first_tx.commit().await.unwrap();
        let second_tx = tokio::time::timeout(Duration::from_secs(1), second_tx)
            .await
            .expect("second writer did not acquire the released SQLite file")
            .unwrap();
        second_tx.rollback().await.unwrap();
    }

    #[test_r::test]
    async fn read_only_transaction_does_not_wait_for_a_writer() {
        let tempdir = tempfile::tempdir().unwrap();
        let config = DbSqliteConfig {
            database: tempdir
                .path()
                .join("shared.db")
                .to_string_lossy()
                .into_owned(),
            max_connections: 1,
            foreign_keys: false,
        };
        let pool = SqlitePool::configured(&config).await.unwrap();

        let writer = pool.with_rw("test", "writer").begin().await.unwrap();
        let reader = tokio::time::timeout(
            Duration::from_millis(250),
            pool.with_ro("test", "reader").begin(),
        )
        .await
        .expect("a read-only transaction was blocked by an active writer")
        .unwrap();

        reader.rollback().await.unwrap();
        writer.rollback().await.unwrap();
    }
}
