use golem_worker_service_base::api::{CustomHttpRequestApi, HealthcheckApi};

use crate::api::api_certificate::ApiCertificateApi;
use crate::api::api_definition::ApiDefinitionApi;
use crate::api::api_deployment::ApiDeploymentApi;
use crate::api::api_domain::ApiDomainApi;
use crate::api::api_security::SecuritySchemeApi;
use crate::api::worker::WorkerApi;
use crate::service::ApiServices;
use poem::get;
use poem::{EndpointExt, Route};
use poem_openapi::OpenApiService;

mod api_certificate;
mod api_definition;
mod api_deployment;
mod api_domain;
mod api_security;
mod common;
mod worker;
mod worker_connect;

type WorkerServiceApis = (
    HealthcheckApi,
    WorkerApi,
    ApiDefinitionApi,
    ApiDeploymentApi,
    ApiCertificateApi,
    ApiDomainApi,
    SecuritySchemeApi,
);

pub fn make_open_api_service(services: ApiServices) -> OpenApiService<WorkerServiceApis, ()> {
    OpenApiService::new(
        (
            HealthcheckApi,
            WorkerApi::new(
                services.component_service.clone(),
                services.worker_service.clone(),
                services.worker_auth_service.clone(),
            ),
            ApiDefinitionApi::new(services.definition_service.clone()),
            ApiDeploymentApi::new(
                services.deployment_service,
                services.worker_auth_service.clone(),
                services.domain_route,
            ),
            ApiCertificateApi::new(services.certificate_service),
            ApiDomainApi::new(services.domain_service),
            SecuritySchemeApi::new(services.security_scheme_service),
        ),
        "Golem API",
        "1.0",
    )
}

pub fn management_routes(services: ApiServices) -> Route {
    let api_service = make_open_api_service(services.clone());
    let connect_services = worker_connect::ConnectService::new(
        services.worker_service.clone(),
        services.worker_auth_service.clone(),
    );
    let ui = api_service.swagger_ui();
    let spec = api_service.spec_endpoint_yaml();
    Route::new()
        .nest("/", api_service)
        .nest("/v1/api/docs", ui)
        .nest("/v1/api/specs", spec)
        .at(
            "/v1/components/:component_id/workers/:worker_name/connect",
            get(worker_connect::ws).data(connect_services),
        )
}

pub fn custom_http_request_route(services: ApiServices) -> Route {
    let api_handler = CustomHttpRequestApi::new(
        services.worker_request_to_http_service,
        services.http_request_api_definition_lookup_service,
        services.file_server_binding_handler,
        services.gateway_session_store,
    );

    Route::new().nest("/", api_handler)
}
