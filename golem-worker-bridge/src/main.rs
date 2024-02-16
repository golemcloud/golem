use golem_worker_bridge::api;
use golem_worker_bridge::api::ApiServices;
use golem_worker_bridge::app_config::WorkerBridgeConfig;
use golem_worker_bridge::register::{RedisApiRegistry, RegisterApiDefinition};
use golem_worker_bridge::worker::WorkerServiceDefault;
use golem_worker_bridge::worker_request_executor::{
    WorkerRequestExecutor, WorkerRequestExecutorDefault,
};
use opentelemetry::global;
use opentelemetry_sdk::metrics::MeterProvider;
use poem::middleware::{OpenTelemetryMetrics, Tracing};
use poem::{EndpointExt, Route};
use std::sync::Arc;
use tracing::error;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let config = WorkerBridgeConfig::default();
    app(&config).await
}

pub async fn app(config: &WorkerBridgeConfig) -> std::io::Result<()> {
    init_tracing_metrics();

    let services: ApiServices = get_api_services(config).await?;

    let api_definition_server = {
        let route = Route::new()
            .nest("/", api::api_definition_routes(services.clone()))
            .with(OpenTelemetryMetrics::new())
            .with(Tracing);

        poem::Server::new(poem::listener::TcpListener::bind((
            "0.0.0.0",
            config.management_port,
        )))
        .name("api")
        .run(route)
    };

    let custom_request_server = {
        let route = api::custom_request_route(services)
            .with(OpenTelemetryMetrics::new())
            .with(Tracing);

        poem::Server::new(poem::listener::TcpListener::bind(("0.0.0.0", config.port)))
            .name("gateway")
            .run(route)
    };

    futures::future::try_join(api_definition_server, custom_request_server).await?;

    Ok(())
}

fn init_tracing_metrics() {
    let prometheus = prometheus::default_registry();
    let exporter = opentelemetry_prometheus::exporter()
        .with_registry(prometheus.clone())
        .build()
        .unwrap();

    global::set_meter_provider(MeterProvider::builder().with_reader(exporter).build());

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_ansi(true)
        .init();
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
