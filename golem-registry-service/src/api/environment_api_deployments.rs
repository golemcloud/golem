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
use super::model::{CreateComponentRequest, UpdateComponentRequest};
use golem_common_next::model::component::{Component, ComponentName};
use golem_common_next::recorded_http_api_request;
use golem_service_base_next::api_tags::ApiTags;
use golem_service_base_next::model::auth::GolemSecurityScheme;
use poem_openapi::payload::{Binary, Json};
use poem_openapi::*;
use tracing::Instrument;
use poem_openapi::param::Path;
use golem_common_next::model::EnvironmentId;
use golem_common_next::model::auth::AuthCtx;
use golem_common_next::api::Page;
use poem::Body;

pub struct EnvironmentApiDeploymentsApi {}

#[OpenApi(prefix_path = "/v1/envs/:environment_id/api-deployments", tag = ApiTags::Component)]
impl EnvironmentApiDeploymentsApi {
    /// Get all api-definitions in the environment
    #[oai(
        path = "/",
        method = "get",
        operation_id = "get_api_definitions"
    )]
    async fn get_api_definitions(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<Component>>> {
        let record = recorded_http_api_request!(
            "get_components",
            environment_id = environment_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_components_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_components_internal(
        &self,
        _environment_id: EnvironmentId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Page<Component>>> {
        todo!()
    }
}
