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

mod api_certificate;
mod api_definition;
mod api_deployment;
mod api_domain;
mod api_security;
pub mod common;
mod custom_http_request;
pub mod dto;
mod worker;

use self::custom_http_request::CustomHttpRequestApi;
use crate::api::api_certificate::ApiCertificateApi;
use crate::api::api_definition::ApiDefinitionApi;
use crate::api::api_deployment::ApiDeploymentApi;
use crate::api::api_domain::ApiDomainApi;
use crate::api::api_security::SecuritySchemeApi;
use crate::api::worker::WorkerApi;
use crate::service::Services;
use golem_service_base::api::HealthcheckApi;
use poem_openapi::OpenApiService;

pub type Apis = (
    HealthcheckApi,
    WorkerApi,
    ApiDefinitionApi,
    ApiDeploymentApi,
    ApiCertificateApi,
    ApiDomainApi,
    SecuritySchemeApi,
);

pub fn make_open_api_service(services: &Services) -> OpenApiService<Apis, ()> {
    OpenApiService::new(
        (
            HealthcheckApi,
            WorkerApi::new(
                services.component_service.clone(),
                services.worker_service.clone(),
                services.worker_auth_service.clone(),
            ),
            ApiDefinitionApi::new(
                services.definition_service.clone(),
                services.worker_auth_service.clone(),
            ),
            ApiDeploymentApi::new(
                services.deployment_service.clone(),
                services.worker_auth_service.clone(),
                services.domain_route.clone(),
            ),
            ApiCertificateApi::new(services.certificate_service.clone()),
            ApiDomainApi::new(services.domain_service.clone()),
            SecuritySchemeApi::new(services.security_scheme_service.clone()),
        ),
        "Golem API",
        "1.0",
    )
}

pub fn custom_http_request_api(services: &Services) -> CustomHttpRequestApi {
    CustomHttpRequestApi::new(
        services.worker_request_to_http_service.clone(),
        services.http_request_api_definition_lookup_service.clone(),
        services.file_server_binding_handler.clone(),
        services.http_handler_binding_handler.clone(),
        services.gateway_session_store.clone(),
    )
}
