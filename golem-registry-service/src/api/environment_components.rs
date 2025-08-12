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
use crate::model::component::Component;
use golem_common::api::Page;
use golem_common::model::auth::AuthCtx;
use golem_common::model::component::ComponentName;
use golem_common::model::deployment::DeploymentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::*;
use tracing::Instrument;
use crate::services::component::ComponentService;
use std::sync::Arc;
use golem_common::model::account::AccountId;
use golem_common::model::ComponentType;
use uuid::uuid;

pub struct EnvironmentComponentsApi {
    component_service: Arc<ComponentService>
}

#[OpenApi(prefix_path = "/v1/envs", tag = ApiTags::Environment,  tag = ApiTags::Component)]
impl EnvironmentComponentsApi {
    pub fn new(component_service: Arc<ComponentService>) -> Self {
        Self { component_service }
    }

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
        environment_id: EnvironmentId,
        payload: CreateComponentRequest,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Component>> {
        let data = payload.component.into_vec().await?;
        let files_archive = payload.files.map(|f| f.into_file());

        // TODO
        let account_id: AccountId = AccountId(uuid!("00000000-0000-0000-0000-000000000000"));

        let response = self.component_service.create(
            &environment_id,
            &payload.component_name,
            payload.component_type.unwrap_or(ComponentType::Durable),
            data,
            files_archive,
            payload.file_options.unwrap_or_default().0,
            payload.dynamic_linking.unwrap_or_default().0,
            payload.env.unwrap_or_default().0,
            payload.agent_types.unwrap_or_default().0,
            &account_id
        ).await?;

        Ok(Json(response))
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

    /// Get all components in a specific deployment
    #[oai(
        path = "/:environment_id/deployments/:deployment_id/components",
        method = "get",
        operation_id = "get_deployment_components",
        tag = ApiTags::Deployment
    )]
    async fn get_deployment_components(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_id: Path<DeploymentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<Component>>> {
        let record = recorded_http_api_request!(
            "get_deployment_components",
            environment_id = environment_id.0.to_string(),
            deployment_id = deployment_id.0.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_deployment_components_internal(environment_id.0, deployment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_deployment_components_internal(
        &self,
        _environment_id: EnvironmentId,
        _deployment_id: DeploymentId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Page<Component>>> {
        todo!()
    }

    /// Get component in a deployment by name
    #[oai(
        path = "/:environment_id/deployments/:deployment_id/components/:component_name",
        method = "get",
        operation_id = "get_deployment_component",
        tag = ApiTags::Deployment
    )]
    async fn get_deployment_component(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_id: Path<DeploymentId>,
        component_name: Path<ComponentName>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Component>> {
        let record = recorded_http_api_request!(
            "get_deployment_component",
            deployment_id = deployment_id.0.0.to_string(),
            component_name = component_name.0.to_string()
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_deployment_component_internal(
                environment_id.0,
                deployment_id.0,
                component_name.0,
                auth,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_deployment_component_internal(
        &self,
        _environment_id: EnvironmentId,
        _deployment_id: DeploymentId,
        _component_name: ComponentName,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Component>> {
        todo!()
    }
}
