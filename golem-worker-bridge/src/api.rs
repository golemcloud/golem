use std::sync::Arc;

use poem::Route;
use poem_openapi::OpenApiService;

use crate::register::RegisterApiDefinition;
use crate::worker_request_executor::WorkerRequestExecutor;

#[derive(Clone)]
pub struct ApiServices {
    pub definition_service: Arc<dyn RegisterApiDefinition + Sync + Send>,
    pub worker_request_executor: Arc<dyn WorkerRequestExecutor + Sync + Send>,
}

pub struct ManagementOpenApiService {
    pub route: Route,
    pub yaml: String,
}

pub fn management_open_api_service(services: ApiServices) -> ManagementOpenApiService {
    let api_service = OpenApiService::new(
        (
            api_definition::ApiDefinitionApi::new(
                services.project_service.clone(),
                services.auth_service.clone(),
                services.definition_service.clone(),
                services.definition_validator,
                services.deployment_service.clone(),
                services.domain_route.clone(),
            )
        ),
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

pub fn management_routes(services: ApiServices) -> Route {
    management_open_api_service(services).route
}


pub fn gateway_routes(services: ApiServices) -> Route {
    let api_handler = api_gateway::ApiGatewayApi::new(
        services.worker_request_executor,
        services.deployment_service,
        services.definition_service,
    );

    Route::new().nest("/", api_handler)
}
