use std::sync::Arc;
use golem_worker_bridge::api::ApiServices;
use golem_worker_bridge::api;
use golem_worker_bridge::app_config::WorkerBridgeConfig;
use poem::Route;
use golem_worker_bridge::worker::WorkerServiceDefault;
use golem_worker_bridge::worker_request_executor::{WorkerRequestExecutor, WorkerRequestExecutorDefault};
use tracing::{error};
use golem_worker_bridge::register::{RedisApiRegistry, RegisterApiDefinition};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let config = WorkerBridgeConfig::default();

    app(&config).await
}

pub async fn app(config: &WorkerBridgeConfig) -> std::io::Result<()> {
    let services: ApiServices = get_api_services(config).await?;

    let api_definition_server = poem::Server::new(poem::listener::TcpListener::bind((
        "0.0.0.0",
        config.management_port,
    )))
        .name("api")
        .run(Route::new().nest("/", api::api_definition_routes(services.clone())));

    let custom_request_server =
        poem::Server::new(poem::listener::TcpListener::bind(("0.0.0.0", config.port)))
            .name("gateway")
            .run(api::custom_request_route(services));

    futures::future::try_join(api_definition_server, custom_request_server).await?;

    Ok(())
}


async fn get_api_services(config: &WorkerBridgeConfig) -> Result<ApiServices, std::io::Error> {
    let definition_service: Arc<dyn RegisterApiDefinition + Sync + Send> =
        Arc::new(RedisApiRegistry::new(&config.redis).await.map_err(|e| {
            error!("RedisApiRegistry - init error: {}", e);

            std::io::Error::new(std::io::ErrorKind::Other, "Init error")
        })?);

    let request_executor: Arc<dyn WorkerRequestExecutor + Sync + Send> =
        Arc::new(WorkerRequestExecutorDefault {
            worker_service: WorkerServiceDefault::new(&config.component_service),
        });

    Ok(ApiServices {
        definition_service,
        worker_request_executor: request_executor,
    })
}