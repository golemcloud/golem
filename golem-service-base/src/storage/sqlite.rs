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

use bytes::Bytes;
use golem_common::config::DbSqliteConfig;
use golem_common::metrics::db::{record_db_failure, record_db_success};
use sqlx::query::{Query, QueryAs};
use sqlx::sqlite::{
    SqliteArguments, SqliteConnectOptions, SqlitePoolOptions, SqliteQueryResult, SqliteRow,
};
use sqlx::{Error, FromRow, IntoArguments, Sqlite};
use std::path::Path;
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct SqlitePool {
    pool: sqlx::SqlitePool,
}

impl SqlitePool {
    pub async fn new(pool: sqlx::SqlitePool) -> Result<Self, anyhow::Error> {
        Ok(Self { pool })
    }

    pub async fn configured(config: &DbSqliteConfig) -> Result<Self, anyhow::Error> {
        let conn_options = SqliteConnectOptions::new()
            .filename(Path::new(config.database.as_str()))
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(config.max_connections)
            .connect_with(conn_options)
            .await?;

        SqlitePool::new(pool).await
    }

    pub async fn execute<'a>(
        &self,
        query: Query<'a, Sqlite, SqliteArguments<'a>>,
    ) -> Result<SqliteQueryResult, String> {
        query
            .execute(&self.pool)
            .await
            .map_err(|err| err.to_string())
    }

    pub fn with(&self, svc_name: &'static str, api_name: &'static str) -> SqliteLabelledApi {
        SqliteLabelledApi {
            svc_name,
            api_name,
            pool: self.pool.clone(),
        }
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
    ) -> Result<SqliteQueryResult, String> {
        query
            .execute(&mut *self.tx)
            .await
            .map_err(|err| err.to_string())
    }

    async fn commit(self) -> Result<(), Error> {
        self.tx.commit().await
    }
}

pub struct SqliteLabelledApi {
    svc_name: &'static str,
    api_name: &'static str,
    pool: sqlx::SqlitePool,
}

impl SqliteLabelledApi {
    pub async fn execute<'a>(
        &self,
        query: Query<'a, Sqlite, SqliteArguments<'a>>,
    ) -> Result<SqliteQueryResult, String> {
        let start = Instant::now();
        self.record(start, query.execute(&self.pool).await)
    }

    pub async fn fetch_optional<'a, A>(
        &self,
        query: Query<'a, Sqlite, A>,
    ) -> Result<Option<SqliteRow>, String>
    where
        A: 'a + IntoArguments<'a, Sqlite>,
    {
        let start = Instant::now();
        self.record(start, query.fetch_optional(&self.pool).await)
    }

    pub async fn fetch_optional_as<'a, O, A>(
        &self,
        query_as: QueryAs<'a, Sqlite, O, A>,
    ) -> Result<Option<O>, String>
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
    ) -> Result<Vec<O>, String>
    where
        A: 'a + IntoArguments<'a, Sqlite>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, SqliteRow>,
    {
        let start = Instant::now();
        self.record(start, query_as.fetch_all(&self.pool).await)
    }

    pub async fn begin(&self) -> Result<SqliteLabelledTransaction, String> {
        let tx = self.pool.begin().await.map_err(|err| err.to_string())?;
        Ok(SqliteLabelledTransaction {
            tx,
            start: Instant::now(),
        })
    }

    pub async fn commit(&self, tx: SqliteLabelledTransaction) -> Result<(), String> {
        let start = tx.start;
        let result = tx.commit().await;
        self.record(start, result)
    }

    fn record<R>(&self, start: Instant, result: Result<R, Error>) -> Result<R, String> {
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
                record_db_failure("sqlite", self.svc_name, self.api_name);
                Err(err.to_string())
            }
        }
    }
}

#[derive(sqlx::FromRow, Debug)]
pub struct DBValue {
    value: Vec<u8>,
}

impl DBValue {
    pub fn into_bytes(self) -> Bytes {
        Bytes::from(self.value)
    }
}
