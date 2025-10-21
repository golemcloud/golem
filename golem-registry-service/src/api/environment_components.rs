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
use crate::model::auth::AuthCtx;
use crate::services::auth::AuthService;
use crate::services::component::{ComponentService, ComponentWriteService};
use golem_common::api::Page;
use golem_common::model::component::ComponentCreation;
use golem_common::model::component::ComponentDto;
use golem_common::model::component::ComponentName;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use golem_service_base::poem::TempFileUpload;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::types::multipart::{JsonField, Upload};
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

pub struct EnvironmentComponentsApi {
    component_service: Arc<ComponentService>,
    component_write_service: Arc<ComponentWriteService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1/envs",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Environment,
    tag = ApiTags::Component
)]
impl EnvironmentComponentsApi {
    pub fn new(
        component_service: Arc<ComponentService>,
        component_write_service: Arc<ComponentWriteService>,
        auth_service: Arc<AuthService>,
    ) -> Self {
        Self {
            component_service,
            component_write_service,
            auth_service,
        }
    }

    /// Create a new component in the environment
    ///
    /// The request body is encoded as multipart/form-data containing metadata and the WASM binary.
    #[oai(
        path = "/:environment_id/components",
        method = "post",
        operation_id = "create_component"
    )]
    async fn create_component(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: CreateComponentRequest,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ComponentDto>> {
        let record = recorded_http_api_request!(
            "create_component",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

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
        auth: AuthCtx,
    ) -> ApiResult<Json<ComponentDto>> {
        let data = payload.component_wasm.into_vec().await?;
        let files_archive = payload.files.map(|f| f.into_file());

        let component: ComponentDto = self
            .component_write_service
            .create(
                &environment_id,
                payload.metadata.0,
                data,
                files_archive,
                &auth,
            )
            .await?
            .into();

        Ok(Json(component))
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
    ) -> ApiResult<Json<Page<ComponentDto>>> {
        let record = recorded_http_api_request!(
            "get_environment_components",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_environment_components_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_components_internal(
        &self,
        environment_id: EnvironmentId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<ComponentDto>>> {
        let components: Vec<ComponentDto> = self
            .component_service
            .list_staged_components(&environment_id, &auth)
            .await?
            .into_iter()
            .map(ComponentDto::from)
            .collect();

        Ok(Json(Page { values: components }))
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
    ) -> ApiResult<Json<ComponentDto>> {
        let record = recorded_http_api_request!(
            "get_environment_component",
            environment_id = environment_id.0.to_string(),
            component_name = component_name.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_environment_component_internal(environment_id.0, component_name.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_component_internal(
        &self,
        environment_id: EnvironmentId,
        component_name: ComponentName,
        auth: AuthCtx,
    ) -> ApiResult<Json<ComponentDto>> {
        let component: ComponentDto = self
            .component_service
            .get_staged_component(environment_id, component_name, &auth)
            .await?
            .into();

        Ok(Json(component))
    }

    /// Get all components in a specific deployment
    #[oai(
        path = "/:environment_id/deployments/:deployment_revision_id/components",
        method = "get",
        operation_id = "get_deployment_components",
        tag = ApiTags::Deployment
    )]
    async fn get_deployment_components(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision_id: Path<DeploymentRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<ComponentDto>>> {
        let record = recorded_http_api_request!(
            "get_deployment_components",
            environment_id = environment_id.0.to_string(),
            deployment_revision_id = deployment_revision_id.0.0,
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_deployment_components_internal(environment_id.0, deployment_revision_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_deployment_components_internal(
        &self,
        environment_id: EnvironmentId,
        deployment_revision_id: DeploymentRevision,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<ComponentDto>>> {
        let components: Vec<ComponentDto> = self
            .component_service
            .list_deployed_components(&environment_id, deployment_revision_id, &auth)
            .await?
            .into_iter()
            .map(ComponentDto::from)
            .collect();

        Ok(Json(Page { values: components }))
    }

    /// Get component in a deployment by name
    #[oai(
        path = "/:environment_id/deployments/:deployment_revision_id/components/:component_name",
        method = "get",
        operation_id = "get_deployment_component",
        tag = ApiTags::Deployment
    )]
    async fn get_deployment_component(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision_id: Path<DeploymentRevision>,
        component_name: Path<ComponentName>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ComponentDto>> {
        let record = recorded_http_api_request!(
            "get_deployment_component",
            environment_id = environment_id.0.to_string(),
            deployment_revision_id = deployment_revision_id.0.0,
            component_name = component_name.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_deployment_component_internal(
                environment_id.0,
                deployment_revision_id.0,
                component_name.0,
                auth,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_deployment_component_internal(
        &self,
        environment_id: EnvironmentId,
        deployment_revision_id: DeploymentRevision,
        component_name: ComponentName,
        auth: AuthCtx,
    ) -> ApiResult<Json<ComponentDto>> {
        let component: ComponentDto = self
            .component_service
            .get_deployed_component(
                environment_id,
                deployment_revision_id,
                component_name,
                &auth,
            )
            .await?
            .into();

        Ok(Json(component))
    }
}

#[derive(Multipart)]
#[oai(rename_all = "camelCase")]
struct CreateComponentRequest {
    metadata: JsonField<ComponentCreation>,
    component_wasm: Upload,
    files: Option<TempFileUpload>,
}
