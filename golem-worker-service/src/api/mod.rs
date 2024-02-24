pub mod api_definition_endpoints;
pub mod common;
pub mod custom_request_endpoint;
pub mod healthcheck;
pub mod worker;
pub mod worker_connect;

use poem::endpoint::PrometheusExporter;
use poem::{get, EndpointExt, Route};
use poem_openapi::OpenApiService;
use prometheus::Registry;
use std::ops::Deref;
use std::sync::Arc;

use crate::service::Services;

type ApiServices = (
    worker::WorkerApi,
    api_definition_endpoints::RegisterApiDefinitionApi,
    healthcheck::HealthcheckApi,
);

pub fn combined_routes(prometheus_registry: Arc<Registry>, services: &Services) -> Route {
    let api_service = make_open_api_service(services);

    let ui = api_service.swagger_ui();
    let spec = api_service.spec_endpoint_yaml();
    let metrics = PrometheusExporter::new(prometheus_registry.deref().clone());

    let connect_services = worker_connect::ConnectService::new(services.worker_service.clone());

    Route::new()
        .nest("/", api_service)
        .nest("/docs", ui)
        .nest("/specs", spec)
        .nest("/metrics", metrics)
        .at(
            "/v2/templates/:template_id/workers/:worker_name/connect",
            get(worker_connect::ws.data(connect_services)),
        )
}

pub fn custom_request_route(services: Services) -> Route {
    let custom_request_executor = custom_request_endpoint::CustomRequestApi::new(
        services.worker_to_http_service,
        services.definition_service,
    );

    Route::new().nest("/", custom_request_executor)
}

pub fn make_open_api_service(services: &Services) -> OpenApiService<ApiServices, ()> {
    OpenApiService::new(
        (
            worker::WorkerApi {
                template_service: services.template_service.clone(),
                worker_service: services.worker_service.clone(),
            },
            api_definition_endpoints::RegisterApiDefinitionApi::new(
                services.definition_service.clone(),
            ),
            healthcheck::HealthcheckApi,
        ),
        "Golem API",
        "2.0",
    )
}
