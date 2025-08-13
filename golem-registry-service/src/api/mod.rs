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

pub mod account_applications;
pub mod account_grants;
pub mod account_tokens;
pub mod accounts;
pub mod api_definitions;
pub mod api_deployments;
pub mod api_domains;
pub mod applications;
pub mod certificates;
pub mod components;
pub mod environment_api_definitions;
pub mod environment_api_deployments;
pub mod environment_api_domains;
pub mod environment_certificates;
pub mod environment_components;
pub mod environment_security_schemes;
pub mod environments;
pub mod error;
pub mod login;
pub mod model;
pub mod plugin_registration;
pub mod security_schemes;
pub mod tokens;

use self::account_applications::AccountApplicationsApi;
use self::account_grants::AccountGrantsApi;
use self::account_tokens::AccountTokensApi;
use self::accounts::AccountsApi;
use self::api_definitions::ApiDefinitionsApi;
use self::api_deployments::ApiDeploymentsApi;
use self::api_domains::ApiDomainsApi;
use self::applications::ApplicationsApi;
use self::certificates::CertificatesApi;
use self::components::ComponentsApi;
use self::environment_api_definitions::EnvironmentApiDefinitionsApi;
use self::environment_api_deployments::EnvironmentApiDeploymentsApi;
use self::environment_api_domains::EnvironmentApiDomainsApi;
use self::environment_certificates::EnvironmentCertificatesApi;
use self::environment_components::EnvironmentComponentsApi;
use self::environment_security_schemes::EnvironmentSecuritySchemesApi;
use self::environments::EnvironmentsApi;
use self::error::ApiError;
use self::login::LoginApi;
use self::plugin_registration::PluginRegistrationApi;
use self::security_schemes::SecuritySchemesApi;
use self::tokens::TokensApi;
use crate::bootstrap::Services;
use golem_service_base::api::HealthcheckApi;
use poem_openapi::OpenApiService;

pub type Apis = (
    HealthcheckApi,
    (
        AccountApplicationsApi,
        AccountGrantsApi,
        AccountTokensApi,
        AccountsApi,
    ),
    ApiDefinitionsApi,
    ApiDeploymentsApi,
    ApiDomainsApi,
    ApplicationsApi,
    CertificatesApi,
    ComponentsApi,
    (
        EnvironmentApiDefinitionsApi,
        EnvironmentApiDeploymentsApi,
        EnvironmentApiDomainsApi,
        EnvironmentCertificatesApi,
        EnvironmentComponentsApi,
        EnvironmentsApi,
        EnvironmentSecuritySchemesApi,
    ),
    LoginApi,
    PluginRegistrationApi,
    SecuritySchemesApi,
    TokensApi,
);

pub fn make_open_api_service(services: &Services) -> OpenApiService<Apis, ()> {
    OpenApiService::new(
        (
            HealthcheckApi,
            (
                AccountApplicationsApi {},
                AccountGrantsApi {},
                AccountTokensApi {},
                AccountsApi {},
            ),
            ApiDefinitionsApi {},
            ApiDeploymentsApi {},
            ApiDomainsApi {},
            ApplicationsApi {},
            CertificatesApi {},
            ComponentsApi::new(services.component_service.clone()),
            (
                EnvironmentApiDefinitionsApi {},
                EnvironmentApiDeploymentsApi {},
                EnvironmentApiDomainsApi {},
                EnvironmentCertificatesApi {},
                EnvironmentComponentsApi::new(services.component_service.clone()),
                EnvironmentsApi {},
                EnvironmentSecuritySchemesApi {},
            ),
            LoginApi {},
            PluginRegistrationApi {},
            SecuritySchemesApi {},
            TokensApi {},
        ),
        "Golem API",
        "1.0",
    )
}

pub type ApiResult<T> = Result<T, ApiError>;
