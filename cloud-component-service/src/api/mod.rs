use crate::service::Services;
use poem::Route;
use poem_openapi::{OpenApiService, Tags};

pub mod component;
pub mod healthcheck;

#[derive(Tags)]
enum ApiTags {
    Component,
    HealthCheck,
}

pub fn combined_routes(services: &Services) -> Route {
    let api_service = make_open_api_service(services);

    let ui = api_service.swagger_ui();
    let spec = api_service.spec_endpoint_yaml();

    Route::new()
        .nest("/", api_service)
        .nest("/docs", ui)
        .nest("/specs", spec)
}

type ApiServices = (component::ComponentApi, healthcheck::HealthcheckApi);

pub fn make_open_api_service(services: &Services) -> OpenApiService<ApiServices, ()> {
    OpenApiService::new(
        (
            component::ComponentApi::new(services.component_service.clone()),
            healthcheck::HealthcheckApi,
        ),
        "Golem API",
        "1.0",
    )
}
