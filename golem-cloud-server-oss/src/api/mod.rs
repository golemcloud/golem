use crate::service::Services;
use poem::Route;
use poem::{get, EndpointExt};
use poem_openapi::OpenApiService;
use poem_openapi::Tags;

mod template;
mod worker;

mod worker_connect;

#[derive(Tags)]
enum ApiTags {
    Template,
    Worker,
}

pub fn combined_routes(services: &Services) -> Route {
    let api_service = make_open_api_service(services);

    let ui = api_service.swagger_ui();
    let spec = api_service.spec_endpoint_yaml();

    let connect_services = worker_connect::ConnectService::new(services.worker_service.clone());

    Route::new()
        .nest("/", api_service)
        .nest("/docs", ui)
        .nest("/specs", spec)
        .at(
            "/templates/:template_id/workers/:worker_name/connect",
            get(worker_connect::ws.data(connect_services)),
        )
}

type ApiServices = (template::TemplateApi, worker::WorkerApi);

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
        ),
        "Golem API",
        "2.0",
    )
}
