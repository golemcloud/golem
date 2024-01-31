use std::error::Error;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Connection, Executor, PgConnection, Pool, Postgres, Sqlite, SqliteConnection};
use tracing::info;

use crate::config::{DbPostgresConfig, DbSqliteConfig};

impl From<&DbPostgresConfig> for PgConnectOptions {
    fn from(config: &DbPostgresConfig) -> Self {
        PgConnectOptions::new()
            .host(config.host.as_str())
            .port(config.port)
            .database(config.database.as_str())
            .username(config.username.as_str())
            .password(config.password.as_str())
    }
}

impl From<&DbSqliteConfig> for SqliteConnectOptions {
    fn from(config: &DbSqliteConfig) -> Self {
        SqliteConnectOptions::new()
            .filename(std::path::Path::new(config.database.as_str()))
            .create_if_missing(true)
    }
}

pub async fn create_postgres_pool(
    config: &DbPostgresConfig,
    workspace: &str,
) -> Result<Pool<Postgres>, Box<dyn Error>> {
    info!(
        "DB Pool: postgresql://{}:{}/{}?currentSchema={}",
        config.host, config.port, config.database, workspace
    );
    let conn_options = PgConnectOptions::from(config);

    let w = workspace.to_string();

    PgPoolOptions::new()
        .max_connections(config.max_connections)
        .after_connect(move |conn, _meta| {
            let w = w.clone();
            Box::pin(async move {
                let sql = format!("SET SCHEMA '{}';", w);
                conn.execute(sqlx::query(&sql)).await?;
                Ok(())
            })
        })
        .connect_with(conn_options)
        .await
        .map_err(|e| e.into())
}

pub async fn postgres_migrate(
    config: &DbPostgresConfig,
    workspace: &str,
) -> Result<(), Box<dyn Error>> {
    let schema = workspace;
    info!(
        "DB migration: postgresql://{}:{}/{}?currentSchema={}",
        config.host, config.port, config.database, schema
    );
    let conn_options = PgConnectOptions::from(config);
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

    sqlx::migrate!("./db/migration/postgres")
        .run(&mut conn)
        .await?;

    let _ = conn.close().await;
    Ok(())
}

pub async fn create_sqlite_pool(config: &DbSqliteConfig) -> Result<Pool<Sqlite>, Box<dyn Error>> {
    info!("DB Pool: sqlite://{}", config.database);
    let conn_options = SqliteConnectOptions::from(config);

    SqlitePoolOptions::new()
        .max_connections(config.max_connections)
        .connect_with(conn_options)
        .await
        .map_err(|e| e.into())
}

pub async fn sqlite_migrate(config: &DbSqliteConfig) -> Result<(), Box<dyn Error>> {
    info!("DB migration: sqlite://{}", config.database);
    let conn_options = SqliteConnectOptions::from(config);
    let mut conn = SqliteConnection::connect_with(&conn_options).await?;
    sqlx::migrate!("./db/migration/sqlite")
        .run(&mut conn)
        .await?;
    let _ = conn.close().await;
    Ok(())
}
