pub mod api_definition_endpoints;
pub mod common;
pub mod custom_request_endpoint;
pub mod worker;
pub mod worker_connect;

use crate::api::api_definition_endpoints::RegisterApiDefinitionApi;
use poem::Route;
use poem_openapi::OpenApiService;

use crate::register::RegisterApiDefinition;
use crate::service::Services;
use crate::worker_request_to_http::WorkerToHttpResponse;

pub struct ManagementOpenApiService {
    pub route: Route,
    pub yaml: String,
}

pub fn management_open_api_service(services: Services) -> ManagementOpenApiService {
    let api_service = OpenApiService::new(
        RegisterApiDefinitionApi::new(services.definition_service.clone()),
        "Golem Api Gateway Management API",
        "1.0",
    );

    let yaml = api_service.spec_yaml();

    let ui = api_service.swagger_ui();
    let spec = api_service.spec_endpoint_yaml();
    let route = Route::new()
        .nest("/", api_service)
        .nest("/v1/api/docs", ui)
        .nest("/v1/api/specs", spec);

    ManagementOpenApiService { route, yaml }
}

pub fn api_definition_routes(services: Services) -> Route {
    management_open_api_service(services).route
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
            template::TemplateApi {
                template_service: services.template_service.clone(),
            },
            worker::WorkerApi {
                template_service: services.template_service.clone(),
                worker_service: services.worker_service.clone(),
            },
            healthcheck::HealthcheckApi,
        ),
        "Golem API",
        "2.0",
    )
}

