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

use anyhow::anyhow;
use sqlx::migrate::MigrationSource;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Connection, Executor, PgConnection, Pool, Postgres, Sqlite, SqliteConnection};
use std::path::Path;
use tracing::info;

use golem_common::config::{DbPostgresConfig, DbSqliteConfig};

fn create_postgres_options(config: &DbPostgresConfig) -> PgConnectOptions {
    PgConnectOptions::new()
        .host(config.host.as_str())
        .port(config.port)
        .database(config.database.as_str())
        .username(config.username.as_str())
        .password(config.password.as_str())
}

pub async fn create_postgres_pool(
    config: &DbPostgresConfig,
) -> Result<Pool<Postgres>, sqlx::Error> {
    let schema = config.schema.clone().unwrap_or("public".to_string());
    info!(
        "DB Pool: postgresql://{}:{}/{}?currentSchema={}",
        config.host, config.port, config.database, schema
    );

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
        .connect_with(create_postgres_options(config))
        .await
}

pub async fn postgres_migrate(
    config: &DbPostgresConfig,
    migrations: impl MigrationSource<'_>,
) -> Result<(), anyhow::Error> {
    let schema = config.schema.clone().unwrap_or("public".to_string());
    info!(
        "DB migration: postgresql://{}:{}/{}?currentSchema={}",
        config.host, config.port, config.database, schema
    );
    let options = create_postgres_options(config);
    let mut conn = PgConnection::connect_with(&options).await?;
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
        return Err(anyhow!("DB schema {schema} do not exists/was not created"));
    }

    let migrator = sqlx::migrate::Migrator::new(migrations).await?;
    migrator.run(&mut conn).await?;

    let _ = conn.close().await;
    Ok(())
}

fn create_sqlite_options(config: &DbSqliteConfig) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(Path::new(&config.database))
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
}

pub async fn create_sqlite_pool(config: &DbSqliteConfig) -> Result<Pool<Sqlite>, sqlx::Error> {
    info!("DB Pool: sqlite://{}", config.database);

    SqlitePoolOptions::new()
        .max_connections(config.max_connections)
        .connect_with(create_sqlite_options(config))
        .await
}

pub async fn sqlite_migrate(
    config: &DbSqliteConfig,
    migrations: impl MigrationSource<'_>,
) -> Result<(), anyhow::Error> {
    info!("DB migration: sqlite://{}", config.database);
    let mut conn = SqliteConnection::connect_with(&create_sqlite_options(config)).await?;
    let migrator = sqlx::migrate::Migrator::new(migrations).await?;
    migrator.run(&mut conn).await?;
    let _ = conn.close().await;
    Ok(())
}
