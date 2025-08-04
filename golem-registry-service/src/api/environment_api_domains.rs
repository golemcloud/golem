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
use golem_common_next::api::Page;
use golem_common_next::api::api_domain::CreateApiDomainRequest;
use golem_common_next::model::api_domain::{ApiDomain, ApiDomainName};
use golem_common_next::model::auth::AuthCtx;
use golem_common_next::model::environment::EnvironmentId;
use golem_common_next::recorded_http_api_request;
use golem_service_base_next::api_tags::ApiTags;
use golem_service_base_next::model::auth::GolemSecurityScheme;
use poem::web::Path;
use poem_openapi::OpenApi;
use poem_openapi::payload::Json;
use tracing::Instrument;

pub struct EnvironmentApiDomainsApi {}

#[OpenApi(prefix_path = "/v1/envs", tag = ApiTags::Environment, tag = ApiTags::ApiDomain)]
impl EnvironmentApiDomainsApi {
    /// Get all API domains
    #[oai(
        path = "/:environment_id/domains",
        method = "get",
        operation_id = "get_domains"
    )]
    async fn get_domains(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<ApiDomain>>> {
        let record = recorded_http_api_request!(
            "get_domains",
            environment_id = environment_id.0.to_string()
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_domains_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_domains_internal(
        &self,
        _environment_id: EnvironmentId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Page<ApiDomain>>> {
        todo!()
    }

    /// Get an api-domain in the environment
    #[oai(
        path = "/:environment_id/domains/:domain",
        method = "get",
        operation_id = "get_environment_domain"
    )]
    async fn get_domain(
        &self,
        environment_id: Path<EnvironmentId>,
        domain: Path<ApiDomainName>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ApiDomain>> {
        let record = recorded_http_api_request!(
            "get_environment_domain",
            environment_id = environment_id.0.to_string()
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_domain_internal(environment_id.0, domain.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_domain_internal(
        &self,
        _environment_id: EnvironmentId,
        _domain: ApiDomainName,
        _auth: AuthCtx,
    ) -> ApiResult<Json<ApiDomain>> {
        todo!()
    }

    /// Create a new api-domain in the environment
    #[oai(
        path = "/:environment_id/domains",
        method = "post",
        operation_id = "create_domain"
    )]
    async fn create_domain(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: Json<CreateApiDomainRequest>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ApiDomain>> {
        let record = recorded_http_api_request!(
            "get_environment_domain",
            environment_id = environment_id.0.to_string()
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .create_domain_internal(environment_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_domain_internal(
        &self,
        _environment_id: EnvironmentId,
        _payload: CreateApiDomainRequest,
        _auth: AuthCtx,
    ) -> ApiResult<Json<ApiDomain>> {
        todo!()
    }
}
