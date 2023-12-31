pub mod template;
pub mod template_object_store;
pub mod worker;

use std::sync::Arc;

use crate::config::CloudServiceConfig;
use crate::db;
use crate::repo::template::{DbTemplateRepo, TemplateRepo};

#[derive(Clone)]
pub struct Services {
    pub template_service: Arc<dyn template::TemplateService + Sync + Send>,
    pub worker_service: Arc<dyn worker::WorkerService + Sync + Send>,
}

impl Services {
    pub async fn new(config: &CloudServiceConfig) -> Result<Services, String> {
        let db_pool = db::create_sqlite_pool(&config.db)
            .await
            .map_err(|e| e.to_string())?;

        let template_repo: Arc<dyn TemplateRepo + Sync + Send> =
            Arc::new(DbTemplateRepo::new(db_pool.clone().into()));

        let object_store: Arc<dyn template_object_store::TemplateObjectStore + Sync + Send> =
            Arc::new(template_object_store::FsTemplateObjectStore::new(
                &config.templates.store,
            )?);

        let template_service: Arc<dyn template::TemplateService + Sync + Send> = Arc::new(
            template::TemplateServiceDefault::new(template_repo.clone(), object_store.clone()),
        );

        let routing_table_service: Arc<
            dyn golem_cloud_servers_base::routing_table::RoutingTableService + Send + Sync,
        > = Arc::new(
            golem_cloud_servers_base::routing_table::RoutingTableServiceDefault::new(
                config.routing_table.clone(),
            ),
        );

        let worker_executor_clients: Arc<
            dyn golem_cloud_servers_base::worker_executor_clients::WorkerExecutorClients + Sync + Send,
        > = Arc::new(
            golem_cloud_servers_base::worker_executor_clients::WorkerExecutorClientsDefault::default(),
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
