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
use super::model::CreateComponentRequest;
use golem_common::api::Page;
use golem_common::model::auth::AuthCtx;
use golem_common::model::component::{Component, ComponentName};
use golem_common::model::environment::EnvironmentId;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::*;
use tracing::Instrument;

pub struct EnvironmentComponentsApi {}

#[OpenApi(prefix_path = "/v1/envs", tag = ApiTags::Environment,  tag = ApiTags::Component)]
impl EnvironmentComponentsApi {
    /// Create a new component in the environment
    ///
    /// The request body is encoded as multipart/form-data containing metadata and the WASM binary.
    #[oai(
        path = "/:environment_id/components",
        method = "post",
        operation_id = "post_component"
    )]
    async fn create_component(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: CreateComponentRequest,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Component>> {
        let record = recorded_http_api_request!(
            "create_component",
            environment_id = environment_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .create_component_internal(environment_id.0, payload, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_component_internal(
        &self,
        _environment_id: EnvironmentId,
        _payload: CreateComponentRequest,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Component>> {
        todo!()
    }

    /// Get all components in the environment
    #[oai(
        path = "/:environment_id/components",
        method = "get",
        operation_id = "get_environment_components"
    )]
    async fn get_environment_components(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<Component>>> {
        let record = recorded_http_api_request!(
            "get_environment_components",
            environment_id = environment_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_environment_components_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_components_internal(
        &self,
        _environment_id: EnvironmentId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Page<Component>>> {
        todo!()
    }

    /// Get a component in the environment by name
    #[oai(
        path = "/:environment_id/components/:component_name",
        method = "get",
        operation_id = "get_environment_component"
    )]
    async fn get_environment_component(
        &self,
        environment_id: Path<EnvironmentId>,
        component_name: Path<ComponentName>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Component>> {
        let record = recorded_http_api_request!(
            "get_environment_component",
            environment_id = environment_id.0.to_string(),
            component_name = component_name.0.to_string()
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_environment_component_internal(environment_id.0, component_name.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_component_internal(
        &self,
        _environment_id: EnvironmentId,
        _component_name: ComponentName,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Component>> {
        todo!()
    }
}
