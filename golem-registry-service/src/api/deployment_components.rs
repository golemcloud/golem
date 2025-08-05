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
use golem_common::model::auth::AuthCtx;
use golem_common::model::component::{Component, ComponentName};
use golem_common::model::deployment::DeploymentId;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::*;
use tracing::Instrument;

pub struct DeploymentComponentsApi {}

#[OpenApi(prefix_path = "/v1/deployments", tag = ApiTags::Deployment, tag = ApiTags::Component)]
impl DeploymentComponentsApi {
    /// Get all components in a specific deployment
    #[oai(
        path = "/:deployment_id/components",
        method = "get",
        operation_id = "get_deployment_components"
    )]
    async fn get_deployment_components(
        &self,
        deployment_id: Path<DeploymentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<Component>>> {
        let record = recorded_http_api_request!(
            "get_deployment_components",
            deployment_id = deployment_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_deployment_components_internal(deployment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_deployment_components_internal(
        &self,
        _deployment_id: DeploymentId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Page<Component>>> {
        todo!()
    }

    /// Get component in a deployment by name
    #[oai(
        path = "/:deployment_id/components/:component_name",
        method = "get",
        operation_id = "get_deployment_component"
    )]
    async fn get_deployment_component(
        &self,
        deployment_id: Path<DeploymentId>,
        component_name: Path<ComponentName>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Component>> {
        let record = recorded_http_api_request!(
            "get_deployment_component",
            deployment_id = deployment_id.0.to_string(),
            component_name = component_name.0.to_string()
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_deployment_component_internal(deployment_id.0, component_name.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_deployment_component_internal(
        &self,
        _deployment_id: DeploymentId,
        _component_name: ComponentName,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Component>> {
        todo!()
    }
}
