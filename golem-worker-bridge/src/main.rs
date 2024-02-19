use golem_worker_bridge::api;
use golem_worker_bridge::api::ApiServices;
use golem_worker_bridge::app_config::WorkerBridgeConfig;
use golem_worker_bridge::register::{RedisApiRegistry, RegisterApiDefinition};
use golem_worker_bridge::service::worker::WorkerServiceDefault;
use golem_worker_bridge::worker_request_to_http::{
    WorkerToHttpResponse, WorkerToHttpResponseDefault,
};
use opentelemetry::global;
use opentelemetry_sdk::metrics::MeterProvider;
use poem::middleware::{OpenTelemetryMetrics, Tracing};
use poem::{EndpointExt, Route};
use std::sync::Arc;
use poem::listener::TcpListener;
use prometheus::Registry;
use tracing::error;
use golem_worker_bridge::service::Services;
use golem_worker_bridge::service::template::{TemplateService, TemplateServiceDefault};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let config = WorkerBridgeConfig::default();
    app(&config).await
}

pub async fn app(config: &WorkerBridgeConfig, prometheus_registry: Registry) -> std::io::Result<()> {
    init_tracing_metrics();

    let http_services: Services = Services::new(config).await?;

    let worker_server = tokio::spawn(async move {
        let prometheus_registry = Arc::new(prometheus_registry);
        let app = api::combined_routes(prometheus_registry, &http_services)
            .with(OpenTelemetryMetrics::new())
            .with(Tracing);

        poem::Server::new(TcpListener::bind(format!("0.0.0.0:{}", config.port)))
            .run(app)
            .await
            .expect("HTTP server failed");
    });

    let custom_request_server = {
        let route = api::custom_request_route(http_services)
            .with(OpenTelemetryMetrics::new())
            .with(Tracing);

        poem::Server::new(poem::listener::TcpListener::bind(("0.0.0.0", config.custom_request_port)))
            .name("gateway")
            .run(route)
    };

    futures::future::try_join(worker_server, custom_request_server).await?;

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

    let routing_table_service: Arc<
        dyn golem_service_base::routing_table::RoutingTableService + Send + Sync,
    > = Arc::new(
        golem_service_base::routing_table::RoutingTableServiceDefault::new(
            config.routing_table.clone(),
        ),
    );

    let template_service: Arc<dyn TemplateService + Send + Sync> = Arc::new(
        TemplateServiceDefault::new(&config.template_service)
    );

    let worker_executor_clients: Arc<
        dyn golem_service_base::worker_executor_clients::WorkerExecutorClients + Sync + Send,
    > = Arc::new(
        golem_service_base::worker_executor_clients::WorkerExecutorClientsDefault::new(
            config.worker_executor_client_cache.max_capacity,
            config.worker_executor_client_cache.time_to_idle,
        ),
    );

    let worker_service: Arc<dyn golem_worker_bridge::service::worker::WorkerService + Sync + Send> =
        Arc::new(WorkerServiceDefault::new(
            worker_executor_clients.clone(),
            template_service.clone(),
            routing_table_service.clone(),
        ));

    Ok(ApiServices {
        definition_service,
        worker_request_executor: request_executor,
    })
}
