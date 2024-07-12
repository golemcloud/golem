use golem_common::tracing::init_tracing_with_default_env_filter;
use golem_service_base::config::DbConfig;
use golem_service_base::db;
use tracing::{error, info};

use cloud_worker_service::app::{app, get_openapi_yaml};
use cloud_worker_service::config::{load_or_dump_config, WorkerServiceCloudConfig};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    if std::env::args().any(|arg| arg == "--dump-openapi-yaml") {
        println!("{}", get_openapi_yaml(&WorkerServiceCloudConfig::default()));
        Ok(())
    } else if let Some(config) = load_or_dump_config() {
        init_tracing_with_default_env_filter(&config.base_config.tracing);

        if config.is_local_env() {
            info!("Golem Worker Service starting up (local mode)...");
        } else {
            info!("Golem Worker Service starting up...");
        }

        match config.base_config.db.clone() {
            DbConfig::Postgres(c) => {
                db::postgres_migrate(&c, "./db/migration/postgres")
                    .await
                    .map_err(|e| {
                        error!("DB - init error: {}", &e);
                        std::io::Error::new(
                            std::io::ErrorKind::Other,
                            format!("Init error (pg): {e:?}"),
                        )
                    })?;
            }
            DbConfig::Sqlite(c) => {
                db::sqlite_migrate(&c, "./db/migration/sqlite")
                    .await
                    .map_err(|e| {
                        error!("DB - init error: {}", e);
                        std::io::Error::new(
                            std::io::ErrorKind::Other,
                            format!("Init error (sqlite): {e:?}"),
                        )
                    })?;
            }
        };

        app(&config).await
    } else {
        Ok(())
    }
}
