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

use super::ApiResult;
use crate::services::auth::AuthService;
use crate::services::domain_registration::DomainRegistrationService;
use golem_common::model::Page;
use golem_common::model::domain_registration::{
    DomainRegistration, DomainRegistrationCreation, DomainRegistrationId,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::poem::NoContentResponse;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::OpenApi;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use std::sync::Arc;
use tracing::Instrument;

pub struct DomainRegistrationsApi {
    domain_registration_service: Arc<DomainRegistrationService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1",
    tag = ApiTags::RegistryService,
    tag = ApiTags::ApiDomain
)]
impl DomainRegistrationsApi {
    pub fn new(
        domain_registration_service: Arc<DomainRegistrationService>,
        auth_service: Arc<AuthService>,
    ) -> Self {
        Self {
            domain_registration_service,
            auth_service,
        }
    }

    /// Create a new domain registration in the environment
    #[oai(
        path = "/envs/:environment_id/domain-registrations",
        method = "post",
        operation_id = "create_domain_registration",
        tag = ApiTags::Environment,
    )]
    pub async fn create_domain_registration(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: Json<DomainRegistrationCreation>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<DomainRegistration>> {
        let record = recorded_http_api_request!(
            "create_domain_registration",
            environment_id = environment_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .create_domain_registration_internal(environment_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    pub async fn create_domain_registration_internal(
        &self,
        environment_id: EnvironmentId,
        payload: DomainRegistrationCreation,
        auth: AuthCtx,
    ) -> ApiResult<Json<DomainRegistration>> {
        let domain_registration = self
            .domain_registration_service
            .create(environment_id, payload, &auth)
            .await?;

        Ok(Json(domain_registration))
    }

    /// List all domain registrations in the environment
    #[oai(
        path = "/envs/:environment_id/domain-registrations",
        method = "get",
        operation_id = "list_environment_domain_registrations",
        tag = ApiTags::Environment,
    )]
    pub async fn list_environment_domain_registrations(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<DomainRegistration>>> {
        let record = recorded_http_api_request!(
            "list_environment_domain_registrations",
            environment_id = environment_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .list_environment_domain_registrations_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    pub async fn list_environment_domain_registrations_internal(
        &self,
        environment_id: EnvironmentId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<DomainRegistration>>> {
        let domain_registrations = self
            .domain_registration_service
            .list_in_environment(environment_id, &auth)
            .await?;
        Ok(Json(Page {
            values: domain_registrations,
        }))
    }

    /// Get domain registration by id
    #[oai(
        path = "/domain-registrations/:domain_registration_id",
        method = "get",
        operation_id = "get_domain_registration"
    )]
    pub async fn get_domain_registration(
        &self,
        domain_registration_id: Path<DomainRegistrationId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<DomainRegistration>> {
        let record = recorded_http_api_request!(
            "get_domain_registration",
            domain_registration_id = domain_registration_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_domain_registration_internal(domain_registration_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_domain_registration_internal(
        &self,
        domain_registration_id: DomainRegistrationId,
        auth: AuthCtx,
    ) -> ApiResult<Json<DomainRegistration>> {
        let domain_registration = self
            .domain_registration_service
            .get_by_id(domain_registration_id, &auth)
            .await?;

        Ok(Json(domain_registration))
    }

    /// Delete domain registration
    #[oai(
        path = "/domain-registrations/:domain_registration_id",
        method = "delete",
        operation_id = "delete_domain_registrations"
    )]
    pub async fn delete_domain_registration(
        &self,
        domain_registration_id: Path<DomainRegistrationId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<NoContentResponse> {
        let record = recorded_http_api_request!(
            "delete_domain_registration",
            domain_registration_id = domain_registration_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .delete_domain_registration_internal(domain_registration_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_domain_registration_internal(
        &self,
        domain_registration_id: DomainRegistrationId,
        auth: AuthCtx,
    ) -> ApiResult<NoContentResponse> {
        self.domain_registration_service
            .delete(domain_registration_id, &auth)
            .await?;

        Ok(NoContentResponse::NoContent)
    }
}
