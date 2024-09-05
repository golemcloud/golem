// Copyright 2024 Golem Cloud
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

use std::error::Error;
use std::ops::Deref;
use std::path::Path;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Connection, Executor, PgConnection, Pool, Postgres, Sqlite, SqliteConnection};
use tracing::info;

use golem_common::config::{DbPostgresConfig, DbSqliteConfig};
struct DbPostgresConfigWrapper<'a>(&'a DbPostgresConfig);
impl<'a> Deref for DbPostgresConfigWrapper<'a> {
    type Target = DbPostgresConfig;
    fn deref(&self) -> &Self::Target {
        self.0
    }
}
struct DbSqliteConfigWrapper<'a>(&'a DbSqliteConfig);
impl<'a> Deref for DbSqliteConfigWrapper<'a> {
    type Target = DbSqliteConfig;
    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<'a> From<DbPostgresConfigWrapper<'a>> for PgConnectOptions {
    fn from(config: DbPostgresConfigWrapper) -> Self {
        PgConnectOptions::new()
            .host(config.host.as_str())
            .port(config.port)
            .database(config.database.as_str())
            .username(config.username.as_str())
            .password(config.password.as_str())
    }
}

impl<'a> From<DbSqliteConfigWrapper<'a>> for SqliteConnectOptions {
    fn from(config: DbSqliteConfigWrapper) -> Self {
        SqliteConnectOptions::new()
            .filename(std::path::Path::new(config.database.as_str()))
            .create_if_missing(true)
    }
}

pub async fn create_postgres_pool(
    config: &DbPostgresConfig,
) -> Result<Pool<Postgres>, Box<dyn Error>> {
    let schema = config.schema.clone().unwrap_or("public".to_string());
    info!(
        "DB Pool: postgresql://{}:{}/{}?currentSchema={}",
        config.host, config.port, config.database, schema
    );
    let conn_options = PgConnectOptions::from(DbPostgresConfigWrapper(config));

    PgPoolOptions::new()
        .max_connections(config.max_connections)
        .after_connect(move |conn, _meta| {
            let s = schema.clone();
            Box::pin(async move {
                let sql = format!("SET SCHEMA '{}';", s);
                conn.execute(sqlx::query(&sql)).await?;
                Ok(())
            })
        })
        .connect_with(conn_options)
        .await
        .map_err(|e| e.into())
}

pub async fn postgres_migrate(config: &DbPostgresConfig, path: &str) -> Result<(), Box<dyn Error>> {
    let schema = config.schema.clone().unwrap_or("public".to_string());
    info!(
        "DB migration: postgresql://{}:{}/{}?currentSchema={}, path: {}",
        config.host, config.port, config.database, schema, path
    );
    let conn_options = PgConnectOptions::from(DbPostgresConfigWrapper(config));
    let mut conn = PgConnection::connect_with(&conn_options).await?;
    let sql = format!("CREATE SCHEMA IF NOT EXISTS {};", schema);
    conn.execute(sqlx::query(&sql)).await?;
    let sql = format!("SET SCHEMA '{}';", schema);
    conn.execute(sqlx::query(&sql)).await?;
    // check if schema exists
    let sql = format!(
        "SELECT schema_name FROM information_schema.schemata WHERE schema_name = '{}';",
        schema
    );
    let result = conn.execute(sqlx::query(&sql)).await?;
    if result.rows_affected() == 0 {
        let _ = conn.close().await;
        return Err(format!("DB schema {} do not exists/was not created", schema).into());
    }

    let migrator = sqlx::migrate::Migrator::new(Path::new(path)).await?;
    migrator.run(&mut conn).await?;

    let _ = conn.close().await;
    Ok(())
}

pub async fn create_sqlite_pool(config: &DbSqliteConfig) -> Result<Pool<Sqlite>, Box<dyn Error>> {
    info!("DB Pool: sqlite://{}", config.database);
    let conn_options = SqliteConnectOptions::from(DbSqliteConfigWrapper(config));

    SqlitePoolOptions::new()
        .max_connections(config.max_connections)
        .connect_with(conn_options)
        .await
        .map_err(|e| e.into())
}

pub async fn sqlite_migrate(config: &DbSqliteConfig, path: &str) -> Result<(), Box<dyn Error>> {
    info!("DB migration: sqlite://{}, path: {}", config.database, path);
    let conn_options = SqliteConnectOptions::from(DbSqliteConfigWrapper(config));
    let mut conn = SqliteConnection::connect_with(&conn_options).await?;
    let migrator = sqlx::migrate::Migrator::new(Path::new(path)).await?;
    migrator.run(&mut conn).await?;
    let _ = conn.close().await;
    Ok(())
}
