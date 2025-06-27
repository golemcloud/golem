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
use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::config::DbPostgresConfig;
use golem_common::metrics::db::{record_db_failure, record_db_success};
use sqlx::migrate::MigrationSource;
use sqlx::postgres::{PgArguments, PgPoolOptions, PgQueryResult, PgRow};
use sqlx::query::{Query, QueryAs};
use sqlx::{Connection, Error, Executor, FromRow, IntoArguments, PgConnection, Postgres};
use std::time::Instant;
use tracing::{debug, error, info};

#[derive(Clone, Debug)]
pub struct PostgresPool {
    pool: sqlx::Pool<Postgres>,
}

impl PostgresPool {
    pub async fn new(pool: sqlx::Pool<Postgres>) -> Result<Self, anyhow::Error> {
        Ok(Self { pool })
    }

    pub async fn configured(config: &DbPostgresConfig) -> Result<Self, anyhow::Error> {
        let schema = config.schema.clone().unwrap_or("public".to_string());
        info!(
            "DB Pool: postgresql://{}:{}/{}?currentSchema={}",
            config.host, config.port, config.database, schema
        );

        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .after_connect(move |conn, _meta| {
                let s = schema.clone();
                Box::pin(async move {
                    let sql = format!("SET SCHEMA '{s}';");
                    conn.execute(sqlx::query(&sql)).await?;
                    Ok(())
                })
            })
            .connect_with(config.connect_options())
            .await?;

        PostgresPool::new(pool).await
    }

    pub fn with(&self, svc_name: &'static str, api_name: &'static str) -> PostgresLabelledApi {
        PostgresLabelledApi {
            svc_name,
            api_name,
            pool: self.pool.clone(),
        }
    }
}

#[async_trait]
impl super::Pool for PostgresPool {
    type LabelledApi = PostgresLabelledApi;
    type LabelledTransaction = ();
    type QueryResult = PgQueryResult;
    type Db = Postgres;
    type Args<'a> = PgArguments;

    fn with_ro(&self, svc_name: &'static str, api_name: &'static str) -> Self::LabelledApi {
        self.with(svc_name, api_name)
    }

    fn with_rw(&self, svc_name: &'static str, api_name: &'static str) -> Self::LabelledApi {
        self.with(svc_name, api_name)
    }
}

pub struct PostgresLabelledTransaction {
    tx: sqlx::Transaction<'static, Postgres>,
    start: Instant,
}

impl PostgresLabelledTransaction {
    pub async fn execute(
        &mut self,
        query: Query<'_, Postgres, PgArguments>,
    ) -> Result<PgQueryResult, RepoError> {
        Ok(query.execute(&mut *self.tx).await?)
    }

    pub async fn fetch_optional<'a, A>(
        &mut self,
        query: Query<'a, Postgres, A>,
    ) -> Result<Option<PgRow>, RepoError>
    where
        A: 'a + IntoArguments<'a, Postgres>,
    {
        Ok(query.fetch_optional(&mut *self.tx).await?)
    }

    pub async fn fetch_optional_as<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Postgres, O, A>,
    ) -> Result<Option<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Postgres>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, PgRow>,
    {
        Ok(query_as.fetch_optional(&mut *self.tx).await?)
    }

    pub async fn fetch_all<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Postgres, O, A>,
    ) -> Result<Vec<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Postgres>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, PgRow>,
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
impl super::PoolApi for PostgresLabelledTransaction {
    type QueryResult = PgQueryResult;
    type Row = PgRow;
    type Db = Postgres;
    type Args<'a> = PgArguments;

    async fn execute<'a>(
        &mut self,
        query: Query<'a, Self::Db, PgArguments>,
    ) -> Result<PgQueryResult, RepoError> {
        PostgresLabelledTransaction::execute(self, query).await
    }

    async fn fetch_optional<'a, A>(
        &mut self,
        query: Query<'a, Self::Db, A>,
    ) -> Result<Option<Self::Row>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
    {
        PostgresLabelledTransaction::fetch_optional(self, query).await
    }

    async fn fetch_optional_as<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Self::Db, O, A>,
    ) -> Result<Option<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, Self::Row>,
    {
        PostgresLabelledTransaction::fetch_optional_as(self, query_as).await
    }

    async fn fetch_all<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Self::Db, O, A>,
    ) -> Result<Vec<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, Self::Row>,
    {
        PostgresLabelledTransaction::fetch_all(self, query_as).await
    }
}

#[async_trait]
impl super::LabelledPoolTransaction for PostgresLabelledTransaction {}

pub struct PostgresLabelledApi {
    svc_name: &'static str,
    api_name: &'static str,
    pool: sqlx::Pool<Postgres>,
}

impl PostgresLabelledApi {
    pub async fn execute(
        &self,
        query: Query<'_, Postgres, PgArguments>,
    ) -> Result<PgQueryResult, RepoError> {
        let start = Instant::now();
        self.record(start, query.execute(&self.pool).await)
    }

    pub async fn fetch_optional<'a, A>(
        &self,
        query: Query<'a, Postgres, A>,
    ) -> Result<Option<PgRow>, RepoError>
    where
        A: 'a + IntoArguments<'a, Postgres>,
    {
        let start = Instant::now();
        self.record(start, query.fetch_optional(&self.pool).await)
    }

    pub async fn fetch_optional_as<'a, O, A>(
        &self,
        query_as: QueryAs<'a, Postgres, O, A>,
    ) -> Result<Option<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Postgres>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, PgRow>,
    {
        let start = Instant::now();
        self.record(start, query_as.fetch_optional(&self.pool).await)
    }

    pub async fn fetch_all<'a, O, A>(
        &self,
        query_as: QueryAs<'a, Postgres, O, A>,
    ) -> Result<Vec<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Postgres>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, PgRow>,
    {
        let start = Instant::now();
        self.record(start, query_as.fetch_all(&self.pool).await)
    }

    pub async fn begin(&self) -> Result<PostgresLabelledTransaction, RepoError> {
        let tx = self.pool.begin().await?;
        Ok(PostgresLabelledTransaction {
            tx,
            start: Instant::now(),
        })
    }

    pub async fn commit(&self, tx: PostgresLabelledTransaction) -> Result<(), RepoError> {
        let start = tx.start;
        let result = tx.commit().await;
        self.record(start, result)
    }

    pub async fn rollback(&self, tx: PostgresLabelledTransaction) -> Result<(), RepoError> {
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
                    "postgres",
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
                record_db_failure("postgres", self.svc_name, self.api_name);
                Err(err.into())
            }
        }
    }
}

#[async_trait]
impl super::PoolApi for PostgresLabelledApi {
    type QueryResult = PgQueryResult;
    type Row = PgRow;
    type Db = Postgres;
    type Args<'a> = PgArguments;

    async fn execute<'a>(
        &mut self,
        query: Query<'a, Self::Db, Self::Args<'a>>,
    ) -> Result<Self::QueryResult, RepoError> {
        PostgresLabelledApi::execute(self, query).await
    }

    async fn fetch_optional<'a, A>(
        &mut self,
        query: Query<'a, Self::Db, A>,
    ) -> Result<Option<Self::Row>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
    {
        PostgresLabelledApi::fetch_optional(self, query).await
    }

    async fn fetch_optional_as<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Self::Db, O, A>,
    ) -> Result<Option<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, Self::Row>,
    {
        PostgresLabelledApi::fetch_optional_as(self, query_as).await
    }

    async fn fetch_all<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Self::Db, O, A>,
    ) -> Result<Vec<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, Self::Row>,
    {
        PostgresLabelledApi::fetch_all(self, query_as).await
    }
}

#[async_trait]
impl super::LabelledPoolApi for PostgresLabelledApi {
    type LabelledTransaction = PostgresLabelledTransaction;

    async fn begin(&self) -> Result<Self::LabelledTransaction, RepoError> {
        PostgresLabelledApi::begin(self).await
    }

    async fn commit(&self, tx: Self::LabelledTransaction) -> Result<(), RepoError> {
        PostgresLabelledApi::commit(self, tx).await
    }

    async fn rollback(&self, tx: Self::LabelledTransaction) -> Result<(), RepoError> {
        PostgresLabelledApi::rollback(self, tx).await
    }
}

pub async fn migrate(
    config: &DbPostgresConfig,
    migrations: impl MigrationSource<'_>,
) -> Result<(), anyhow::Error> {
    let schema = config.schema.clone().unwrap_or("public".to_string());
    info!(
        "DB migration: postgresql://{}:{}/{}?currentSchema={}",
        config.host, config.port, config.database, schema
    );
    let options = config.connect_options();
    let mut conn = PgConnection::connect_with(&options).await?;
    let sql = format!("CREATE SCHEMA IF NOT EXISTS {schema};");
    conn.execute(sqlx::query(&sql)).await?;
    let sql = format!("SET SCHEMA '{schema}';");
    conn.execute(sqlx::query(&sql)).await?;
    // check if schema exists
    let sql = format!(
        "SELECT schema_name FROM information_schema.schemata WHERE schema_name = '{schema}';"
    );
    let result = conn.execute(sqlx::query(&sql)).await?;
    if result.rows_affected() == 0 {
        let _ = conn.close().await;
        return Err(anyhow!("DB schema {schema} do not exists/was not created"));
    }

    let migrator = sqlx::migrate::Migrator::new(migrations).await?;
    // See: https://github.com/launchbadge/sqlx/issues/954, Send required for golem-cli
    futures::executor::block_on(migrator.run(&mut conn))?;

    let _ = conn.close().await;
    Ok(())
}
