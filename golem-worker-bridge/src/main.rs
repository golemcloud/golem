use golem_worker_bridge::api;
use golem_worker_bridge::api::ApiServices;
use golem_worker_bridge::app_config::WorkerBridgeConfig;
use golem_worker_bridge::register::{RedisApiRegistry, RegisterApiDefinition};
use golem_worker_bridge::worker::WorkerServiceDefault;
use golem_worker_bridge::worker_request_executor::{
    WorkerRequestExecutor, WorkerRequestExecutorDefault,
};
use poem::Route;
use std::sync::Arc;
use openapiv3::OpenAPI;
use tracing::error;
use golem_worker_bridge::api_definition::PathPattern;
use golem_worker_bridge::oas_worker_bridge::sample;

#[tokio::main]
async fn main() -> std::io::Result<()> {
   // let config = WorkerBridgeConfig::default();
    let openapi: OpenAPI = serde_yaml::from_str(sample().as_str()).expect("Could not deserialize input");
    // println!("{:?}", openapi);
    // println!("{:?}", openapi);
    openapi.extensions.iter().for_each(|(key, value)| {
        println!("Key: {}", key);
        println!("Value: {:?}", value);
    });

    openapi.paths.iter().for_each(|(path, path_item)| {
        println!("Path: {}", path);

        match path_item {
            openapiv3::ReferenceOr::Item(item) => {
                println!("extensions: {:?}", item.extensions);
            }
            openapiv3::ReferenceOr::Reference {
                reference,
            } => {
                println!("Reference: {:?}", reference);
            }
        }
        println!("path item is {:?}", path_item);

        let hmm = PathPattern::from(path).unwrap();

        println!("Path Pattern: {:?}", hmm);

        //println!("Path Item: {:?}", path_item);
    });
    Ok(())
   // app(&config).await
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
            worker_service: WorkerServiceDefault::new(&config.template_service),
        });

    Ok(ApiServices {
        definition_service,
        worker_request_executor: request_executor,
    })
}
