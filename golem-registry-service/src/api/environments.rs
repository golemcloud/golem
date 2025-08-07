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
use golem_common::api::environment::UpdateEnvironmentRequest;
use golem_common::model::auth::AuthCtx;
use golem_common::model::environment::*;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::OpenApi;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use tracing::Instrument;

pub struct EnvironmentsApi {}

#[OpenApi(prefix_path = "/v1/envs", tag = ApiTags::Environment)]
impl EnvironmentsApi {
    /// Get environment by id.
    #[oai(
        path = "/:environment_id",
        method = "get",
        operation_id = "get_environment"
    )]
    pub async fn get_environment(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Environment>> {
        let record = recorded_http_api_request!(
            "get_environment",
            environment_id = environment_id.0.to_string()
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_environment_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_internal(
        &self,
        _environment_id: EnvironmentId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Environment>> {
        todo!()
    }

    /// Update application by id.
    #[oai(
        path = "/:environment_id",
        method = "patch",
        operation_id = "update_environment"
    )]
    pub async fn update_environment(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: Json<UpdateEnvironmentRequest>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Environment>> {
        let record = recorded_http_api_request!(
            "update_environment",
            environment_id = environment_id.0.to_string()
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .update_environment_internal(environment_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_environment_internal(
        &self,
        _application_id: EnvironmentId,
        _payload: UpdateEnvironmentRequest,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Environment>> {
        todo!()
    }
}
