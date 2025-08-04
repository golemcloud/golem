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
use golem_common::api::Page;
use golem_common::api::api_definition::{CreateSecuritySchemeRequest, SecuritySchemeResponseView};
use golem_common::model::auth::AuthCtx;
use golem_common::model::environment::EnvironmentId;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::OpenApi;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use tracing::Instrument;

pub struct EnvironmentSecuritySchemesApi {}

#[OpenApi(prefix_path = "/v1/envs", tag = ApiTags::Environment, tag = ApiTags::ApiSecurity)]
impl EnvironmentSecuritySchemesApi {
    /// Get all security schemes in the environment
    #[oai(
        path = "/:environment_id/security-schemes",
        method = "get",
        operation_id = "get_environment_security_schemes"
    )]
    async fn get_environment_security_schemes(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<SecuritySchemeResponseView>>> {
        let record = recorded_http_api_request!(
            "get_environment_security_schemes",
            environment_id = environment_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_environment_security_schemes_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_security_schemes_internal(
        &self,
        _environment_id: EnvironmentId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Page<SecuritySchemeResponseView>>> {
        todo!()
    }

    /// Get a security scheme
    ///
    /// Get a security scheme by name
    #[oai(
        path = "/:environment_id/security-schemes/:security_scheme_name",
        method = "get",
        operation_id = "get_environment_security_scheme"
    )]
    async fn get_environment_security_scheme(
        &self,
        environment_id: Path<EnvironmentId>,
        security_scheme_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<SecuritySchemeResponseView>> {
        let record = recorded_http_api_request!(
            "get_environment_security_scheme",
            environment_id = environment_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_environment_security_scheme_internal(
                environment_id.0,
                security_scheme_name.0,
                auth,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_security_scheme_internal(
        &self,
        _environment_id: EnvironmentId,
        _security_scheme_name: String,
        _auth: AuthCtx,
    ) -> ApiResult<Json<SecuritySchemeResponseView>> {
        todo!()
    }

    /// Create a new security scheme in the environment
    #[oai(
        path = "/:environment_id/security-schemes",
        method = "post",
        operation_id = "create_environment_security_scheme"
    )]
    async fn create_environment_security_scheme(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: Json<CreateSecuritySchemeRequest>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<SecuritySchemeResponseView>> {
        let record = recorded_http_api_request!(
            "create_environment_security_scheme",
            environment_id = environment_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .create_environment_security_scheme_internal(environment_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_environment_security_scheme_internal(
        &self,
        _environment_id: EnvironmentId,
        _payload: CreateSecuritySchemeRequest,
        _auth: AuthCtx,
    ) -> ApiResult<Json<SecuritySchemeResponseView>> {
        todo!()
    }
}
