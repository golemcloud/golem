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
use golem_common_next::api::api_definition::SecuritySchemeResponseView;
use golem_common_next::model::api_definition::security_scheme::SecuritySchemeId;
use golem_common_next::model::auth::AuthCtx;
use golem_common_next::recorded_http_api_request;
use golem_service_base_next::api_tags::ApiTags;
use golem_service_base_next::model::auth::GolemSecurityScheme;
use poem_openapi::OpenApi;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use tracing::Instrument;

pub struct SecuritySchemesApi {}

#[OpenApi(prefix_path = "/v1/security-schemes", tag = ApiTags::ApiSecurity)]
impl SecuritySchemesApi {
    /// Get api security scheme by id
    #[oai(
        path = "/:security_scheme_id",
        method = "get",
        operation_id = "get_security_scheme"
    )]
    async fn get_security_scheme(
        &self,
        security_scheme_id: Path<SecuritySchemeId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<SecuritySchemeResponseView>> {
        let record = recorded_http_api_request!(
            "get_security_scheme",
            security_scheme_id = security_scheme_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_security_scheme_internal(security_scheme_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_security_scheme_internal(
        &self,
        _security_scheme_id: SecuritySchemeId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<SecuritySchemeResponseView>> {
        todo!()
    }

    /// Get all revisions of the security scheme
    #[oai(
        path = "/:security_scheme_id/revisions",
        method = "get",
        operation_id = "get_security_scheme_revisions"
    )]
    async fn get_security_scheme_revisions(
        &self,
        security_scheme_id: Path<SecuritySchemeId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<SecuritySchemeResponseView>>> {
        let record = recorded_http_api_request!(
            "get_security_scheme_revisions",
            security_scheme_id = security_scheme_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_security_scheme_revisions_internal(security_scheme_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_security_scheme_revisions_internal(
        &self,
        _security_scheme_id: SecuritySchemeId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Page<SecuritySchemeResponseView>>> {
        todo!()
    }
}
