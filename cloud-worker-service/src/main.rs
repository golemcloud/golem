use cloud_worker_service::app::{app, get_openapi_yaml};
use cloud_worker_service::config::WorkerServiceCloudConfig;
use golem_service_base::config::DbConfig;
use golem_service_base::db;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    if std::env::args().any(|arg| arg == "--dump-openapi-yaml") {
        println!("{}", get_openapi_yaml(&WorkerServiceCloudConfig::default()));
        Ok(())
    } else {
        let config = WorkerServiceCloudConfig::load();

        if config.base_config.enable_json_log {
            tracing_subscriber::fmt()
                .json()
                .flatten_event(true)
                // .with_span_events(FmtSpan::FULL) // NOTE: enable to see span events
                .with_env_filter(EnvFilter::from_default_env())
                .init();
        } else {
            tracing_subscriber::fmt()
                .with_env_filter(EnvFilter::from_default_env())
                .with_ansi(true)
                .init();
        }

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
    }
}
