use anyhow::anyhow;
use golem_common::config::DbConfig;
use golem_common::tracing::init_tracing_with_default_env_filter;
use golem_service_base::db;
use golem_service_base::migration::{Migrations, MigrationsDir};
use std::path::Path;
use tracing::{error, info};

use cloud_worker_service::app::{app, dump_openapi_yaml};
use cloud_worker_service::config::load_or_dump_config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--dump-openapi-yaml") {
        println!("{}", dump_openapi_yaml().await.map_err(|err| anyhow!(err))?);
        Ok(())
    } else if let Some(config) = load_or_dump_config() {
        init_tracing_with_default_env_filter(&config.base_config.tracing);

        if config.is_local_env() {
            info!("Golem Worker Service starting up (local mode)...");
        } else {
            info!("Golem Worker Service starting up...");
        }

        let migrations = MigrationsDir::new(Path::new("./db/migration").to_path_buf());
        match config.base_config.db.clone() {
            DbConfig::Postgres(c) => {
                db::postgres::migrate(&c, migrations.postgres_migrations())
                    .await
                    .map_err(|e| {
                        error!("DB - init error: {}", &e);
                        std::io::Error::other(format!("Init error (pg): {e:?}"))
                    })?;
            }
            DbConfig::Sqlite(c) => {
                db::sqlite::migrate(&c, migrations.sqlite_migrations())
                    .await
                    .map_err(|e| {
                        error!("DB - init error: {}", e);
                        std::io::Error::other(format!("Init error (sqlite): {e:?}"))
                    })?;
            }
        };

        Ok(app(&config).await?)
    } else {
        Ok(())
    }
}
