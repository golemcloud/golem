use std::error::Error;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Connection, Pool, Sqlite, SqliteConnection};
use tracing::info;

use crate::config::DbSqliteConfig;

impl From<&DbSqliteConfig> for SqliteConnectOptions {
    fn from(config: &DbSqliteConfig) -> Self {
        SqliteConnectOptions::new()
            .filename(std::path::Path::new(config.database.as_str()))
            .create_if_missing(true)
    }
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
