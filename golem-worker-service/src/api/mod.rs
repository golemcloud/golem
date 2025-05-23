// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

pub mod api_definition;
pub mod api_deployment;
mod security_scheme;
pub mod worker;
use crate::api::worker::WorkerApi;
use crate::service::Services;
use golem_worker_service_base::api::CustomHttpRequestApi;
use golem_worker_service_base::api::HealthcheckApi;
use poem::endpoint::PrometheusExporter;
use poem::Route;
use poem_openapi::OpenApiService;
use prometheus::Registry;

pub type ApiServices = (
    WorkerApi,
    api_definition::RegisterApiDefinitionApi,
    api_deployment::ApiDeploymentApi,
    security_scheme::SecuritySchemeApi,
    HealthcheckApi,
);

pub fn combined_routes(prometheus_registry: Registry, services: &Services) -> Route {
    let api_service = make_open_api_service(services);

    let ui = api_service.swagger_ui();
    let spec = api_service.spec_endpoint_yaml();
    let metrics = PrometheusExporter::new(prometheus_registry.clone());

    Route::new()
        .nest("/", api_service)
        .nest("/docs", ui)
        .nest("/specs", spec)
        .nest("/metrics", metrics)
}

pub fn custom_request_route(services: &Services) -> Route {
    let custom_request_executor = CustomHttpRequestApi::new(
        services.worker_to_http_service.clone(),
        services.http_definition_lookup_service.clone(),
        services.fileserver_binding_handler.clone(),
        services.http_handler_binding_handler.clone(),
        services.gateway_session_store.clone(),
    );

    Route::new().nest("/", custom_request_executor)
}

pub fn make_open_api_service(services: &Services) -> OpenApiService<ApiServices, ()> {
    OpenApiService::new(
        (
            worker::WorkerApi {
                component_service: services.component_service.clone(),
                worker_service: services.worker_service.clone(),
            },
            api_definition::RegisterApiDefinitionApi::new(services.definition_service.clone()),
            api_deployment::ApiDeploymentApi::new(services.deployment_service.clone()),
            security_scheme::SecuritySchemeApi::new(services.security_scheme_service.clone()),
            HealthcheckApi,
        ),
        "Golem API",
        "1.0",
    )
}
