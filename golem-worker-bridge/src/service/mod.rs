pub mod worker;
pub mod template;

use std::sync::Arc;
use tracing::error;
use crate::app_config::WorkerBridgeConfig;
use crate::register::{RedisApiRegistry, RegisterApiDefinition};
use crate::worker_request_to_http::{WorkerToHttpResponse, WorkerToHttpResponseDefault};

#[derive(Clone)]
pub struct Services {
    pub worker_service: Arc<dyn worker::WorkerService + Sync + Send>,
    pub definition_service: Arc<dyn RegisterApiDefinition + Sync + Send>,
    pub worker_to_http_service: Arc<dyn WorkerToHttpResponse + Sync + Send>,
    pub template_service: Arc<dyn template::TemplateService + Sync + Send>,
}

impl Services {
    pub async fn new(config: &WorkerBridgeConfig) -> Result<Services, String> {
        let template_service: Arc<dyn template::TemplateService + Sync + Send> = Arc::new(
            template::TemplateServiceDefault::new(&config.template_service)
        );

        let routing_table_service: Arc<
            dyn golem_service_base::routing_table::RoutingTableService + Send + Sync,
        > = Arc::new(
            golem_service_base::routing_table::RoutingTableServiceDefault::new(
                config.routing_table.clone(),
            ),
        );

        let worker_executor_grpc_clients: Arc<
            dyn golem_service_base::worker_executor_clients::WorkerExecutorClients + Sync + Send,
        > = Arc::new(
            golem_service_base::worker_executor_clients::WorkerExecutorClientsDefault::new(
                config.worker_executor_client_cache.max_capacity,
                config.worker_executor_client_cache.time_to_idle,
            ),
        );

        let worker_service: Arc<dyn worker::WorkerService + Sync + Send> =
            Arc::new(worker::WorkerServiceDefault::new(
                worker_executor_grpc_clients.clone(),
                template_service.clone(),
                routing_table_service.clone(),
            ));

        let definition_service: Arc<dyn RegisterApiDefinition + Sync + Send> =
            Arc::new(RedisApiRegistry::new(&config.redis).await.map_err(|e| {
                error!("RedisApiRegistry - init error: {}", e);

                std::io::Error::new(std::io::ErrorKind::Other, "Init error")
            })?);

        let worker_to_http_service: Arc<dyn WorkerToHttpResponse + Sync + Send> =
            Arc::new(WorkerToHttpResponseDefault::new(worker::WorkerServiceDefault::new(
                worker_executor_grpc_clients.clone(),
                template_service.clone(),
                routing_table_service.clone(),
            )));

        Ok(Services {
            worker_service,
            definition_service,
            worker_to_http_service,
            template_service
        })
    }
}

