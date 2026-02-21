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

pub mod account_tokens;
pub mod accounts;
pub mod applications;
pub mod components;
pub mod domain_registrations;
pub mod environment_plugin_grants;
pub mod environment_shares;
pub mod environments;
pub mod error;
pub mod http_api_deployments;
pub mod login;
pub mod plugin_registrations;
pub mod reports;
pub mod security_schemes;
pub mod tokens;

use self::account_tokens::AccountTokensApi;
use self::accounts::AccountsApi;
use self::applications::ApplicationsApi;
use self::components::ComponentsApi;
use self::domain_registrations::DomainRegistrationsApi;
use self::environment_plugin_grants::EnvironmentPluginGrantsApi;
use self::environment_shares::EnvironmentSharesApi;
use self::environments::EnvironmentsApi;
use self::error::ApiError;
use self::http_api_deployments::HttpApiDeploymentsApi;
use self::login::LoginApi;
use self::plugin_registrations::PluginRegistrationsApi;
use self::reports::ReportsApi;
use self::security_schemes::SecuritySchemesApi;
use self::tokens::TokensApi;
use crate::bootstrap::Services;
use golem_service_base::api::HealthcheckApi;
use poem_openapi::OpenApiService;

pub type Apis = (
    HealthcheckApi,
    (AccountTokensApi, AccountsApi),
    ApplicationsApi,
    ComponentsApi,
    DomainRegistrationsApi,
    (
        EnvironmentPluginGrantsApi,
        EnvironmentsApi,
        EnvironmentSharesApi,
    ),
    HttpApiDeploymentsApi,
    LoginApi,
    PluginRegistrationsApi,
    ReportsApi,
    SecuritySchemesApi,
    TokensApi,
);

pub fn make_open_api_service(services: &Services) -> OpenApiService<Apis, ()> {
    OpenApiService::new(
        (
            HealthcheckApi,
            (
                AccountTokensApi::new(
                    services.token_service.clone(),
                    services.auth_service.clone(),
                ),
                AccountsApi::new(
                    services.account_service.clone(),
                    services.plan_service.clone(),
                    services.auth_service.clone(),
                    services.plugin_registration_service.clone(),
                ),
            ),
            ApplicationsApi::new(
                services.application_service.clone(),
                services.auth_service.clone(),
            ),
            ComponentsApi::new(
                services.component_service.clone(),
                services.component_write_service.clone(),
                services.auth_service.clone(),
            ),
            DomainRegistrationsApi::new(
                services.domain_registration_service.clone(),
                services.auth_service.clone(),
            ),
            (
                EnvironmentPluginGrantsApi::new(
                    services.environment_plugin_grant_service.clone(),
                    services.auth_service.clone(),
                ),
                EnvironmentsApi::new(
                    services.environment_service.clone(),
                    services.deployment_service.clone(),
                    services.deployment_write_service.clone(),
                    services.auth_service.clone(),
                ),
                EnvironmentSharesApi::new(
                    services.environment_share_service.clone(),
                    services.auth_service.clone(),
                ),
            ),
            HttpApiDeploymentsApi::new(
                services.http_api_deployment_service.clone(),
                services.auth_service.clone(),
            ),
            LoginApi::new(
                services.login_system.clone(),
                services.token_service.clone(),
            ),
            PluginRegistrationsApi::new(
                services.plugin_registration_service.clone(),
                services.auth_service.clone(),
            ),
            ReportsApi::new(
                services.reports_service.clone(),
                services.auth_service.clone(),
            ),
            SecuritySchemesApi::new(
                services.security_scheme_service.clone(),
                services.auth_service.clone(),
            ),
            TokensApi::new(
                services.token_service.clone(),
                services.auth_service.clone(),
            ),
        ),
        "Golem API",
        "1.0",
    )
}

pub type ApiResult<T> = Result<T, ApiError>;
