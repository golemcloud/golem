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
use golem_common_next::model::component::{Component, ComponentName};
use golem_common_next::recorded_http_api_request;
use golem_service_base_next::api_tags::ApiTags;
use golem_service_base_next::model::auth::GolemSecurityScheme;
use poem_openapi::payload::Json;
use poem_openapi::*;
use tracing::Instrument;
use poem_openapi::param::Path;
use golem_common_next::model::EnvironmentId;

pub struct EnvironmentComponentsApi {}

#[OpenApi(prefix_path = "/v1/envs/{environment_id}/components", tag = ApiTags::Component)]
impl EnvironmentComponentsApi {
    /// Create a new component
    ///
    /// The request body is encoded as multipart/form-data containing metadata and the WASM binary.
    /// If the component type is not specified, it will be considered as a `Durable` component.
    #[oai(
        path = "/",
        method = "post",
        operation_id = "create_component"
    )]
    async fn create_component(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: CreateComponentRequest,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Component>> {
        let record = recorded_http_api_request!("create_component",);
        let response = self
            .create_component_internal(environment_id.0, payload, token)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn create_component_internal(
        &self,
        _environment_id: EnvironmentId,
        _payload: CreateComponentRequest,
        _token: GolemSecurityScheme,
    ) -> ApiResult<Json<Component>> {
        todo!()
    }

    /// Get a component in an environment by name
    #[oai(
        path = "/:component_name",
        method = "get",
        operation_id = "get_component_by_name"
    )]
    async fn get_component_by_name(
        &self,
        environment_id: Path<EnvironmentId>,
        component_name: Path<ComponentName>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Component>> {
        let record = recorded_http_api_request!("create_component",);
        let response = self
            .get_component_by_name_internal(environment_id.0, component_name.0, token)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn get_component_by_name_internal(
        &self,
        _environment_id: EnvironmentId,
        _component_name: ComponentName,
        _token: GolemSecurityScheme,
    ) -> ApiResult<Json<Component>> {
        todo!()
    }
}
