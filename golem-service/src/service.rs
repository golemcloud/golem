pub mod template;
pub mod worker;

use golem_service_base::config::TemplateStoreConfig;
use golem_service_base::service::template_object_store;
use std::sync::Arc;

use crate::config::{CloudServiceConfig, DbConfig};
use crate::db;
use crate::repo::template::{DbTemplateRepo, TemplateRepo};

#[derive(Clone)]
pub struct Services {
    pub template_service: Arc<dyn template::TemplateService + Sync + Send>,
    pub worker_service: Arc<dyn worker::WorkerService + Sync + Send>,
}

impl Services {
    pub async fn new(config: &CloudServiceConfig) -> Result<Services, String> {
        let template_repo: Arc<dyn TemplateRepo + Sync + Send> = match config.db.clone() {
            DbConfig::Postgres(c) => {
                let db_pool = db::create_postgres_pool(&c)
                    .await
                    .map_err(|e| e.to_string())?;
                Arc::new(DbTemplateRepo::new(db_pool.clone().into()))
            }
            DbConfig::Sqlite(c) => {
                let db_pool = db::create_sqlite_pool(&c)
                    .await
                    .map_err(|e| e.to_string())?;
                Arc::new(DbTemplateRepo::new(db_pool.clone().into()))
            }
        };

        let object_store: Arc<dyn template_object_store::TemplateObjectStore + Sync + Send> =
            match config.templates.store.clone() {
                TemplateStoreConfig::S3(c) => {
                    Arc::new(template_object_store::AwsS3TemplateObjectStore::new(&c).await)
                }
                TemplateStoreConfig::Local(c) => {
                    Arc::new(template_object_store::FsTemplateObjectStore::new(&c)?)
                }
            };

        let template_service: Arc<dyn template::TemplateService + Sync + Send> = Arc::new(
            template::TemplateServiceDefault::new(template_repo.clone(), object_store.clone()),
        );

        let routing_table_service: Arc<
            dyn golem_service_base::routing_table::RoutingTableService + Send + Sync,
        > = Arc::new(
            golem_service_base::routing_table::RoutingTableServiceDefault::new(
                config.routing_table.clone(),
            ),
        );

        let worker_executor_clients: Arc<
            dyn golem_service_base::worker_executor_clients::WorkerExecutorClients + Sync + Send,
        > = Arc::new(
            golem_service_base::worker_executor_clients::WorkerExecutorClientsDefault::new(),
        );

        let worker_service: Arc<dyn worker::WorkerService + Sync + Send> =
            Arc::new(worker::WorkerServiceDefault::new(
                worker_executor_clients.clone(),
                template_service.clone(),
                routing_table_service.clone(),
            ));

        Ok(Services {
            template_service,
            worker_service,
        })
    }

    pub fn noop() -> Self {
        let template_service: Arc<dyn template::TemplateService + Sync + Send> =
            Arc::new(template::TemplateServiceNoOp::default());

        let worker_service: Arc<dyn worker::WorkerService + Sync + Send> =
            Arc::new(worker::WorkerServiceNoOp::default());

        Services {
            template_service,
            worker_service,
        }
    }
}
