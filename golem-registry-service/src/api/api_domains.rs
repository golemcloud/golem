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
use golem_common_next::api::api_domain::UpdateApiDomainRequest;
use golem_common_next::model::api_domain::{ApiDomain, ApiDomainId};
use golem_common_next::model::auth::AuthCtx;
use golem_common_next::recorded_http_api_request;
use golem_service_base_next::api_tags::ApiTags;
use golem_service_base_next::model::auth::GolemSecurityScheme;
use poem::web::Path;
use poem_openapi::OpenApi;
use poem_openapi::payload::Json;
use tracing::Instrument;

pub struct ApiDomainsApi {}

#[OpenApi(prefix_path = "/v1/domains", tag = ApiTags::ApiDomain)]
impl ApiDomainsApi {
    /// Get api domain by id
    #[oai(path = "/:domain_id", method = "get", operation_id = "get_domain")]
    async fn get_domain(
        &self,
        domain_id: Path<ApiDomainId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ApiDomain>> {
        let record = recorded_http_api_request!("get_domains", domain_id = domain_id.0.to_string());

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_domain_internal(domain_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_domain_internal(
        &self,
        _domain_id: ApiDomainId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<ApiDomain>> {
        todo!()
    }

    /// Get all revisions of an api-domain
    #[oai(
        path = "/:domain_id/revisions",
        method = "get",
        operation_id = "get_domain_revisions"
    )]
    async fn get_domain_revisions(
        &self,
        domain_id: Path<ApiDomainId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<ApiDomain>>> {
        let record =
            recorded_http_api_request!("get_domain_revisions", domain_id = domain_id.0.to_string());

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_domain_revisions_internal(domain_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_domain_revisions_internal(
        &self,
        _domain_id: ApiDomainId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Page<ApiDomain>>> {
        todo!()
    }

    /// Update an api-domain
    #[oai(path = "/:domain_id", method = "patch", operation_id = "update_domain")]
    async fn update_domain(
        &self,
        domain_id: Path<ApiDomainId>,
        payload: Json<UpdateApiDomainRequest>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ApiDomain>> {
        let record =
            recorded_http_api_request!("update_domain", domain_id = domain_id.0.to_string());

        let auth = AuthCtx::new(token.secret());

        let response = self
            .update_domain_internal(domain_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_domain_internal(
        &self,
        _domain_id: ApiDomainId,
        _payload: UpdateApiDomainRequest,
        _auth: AuthCtx,
    ) -> ApiResult<Json<ApiDomain>> {
        todo!()
    }
}
