// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
use crate::services::auth::AuthService;
use crate::services::resource_definition::ResourceDefinitionService;
use golem_common::model::Page;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::poem::NoContentResponse;
use golem_common::model::quota::{
    ResourceDefinition, ResourceDefinitionCreation, ResourceDefinitionId,
    ResourceDefinitionRevision, ResourceDefinitionUpdate, ResourceName,
};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::OpenApi;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use std::sync::Arc;
use tracing::Instrument;

pub struct ResourceDefinitionsApi {
    resource_definitons_service: Arc<ResourceDefinitionService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Resources
)]
impl ResourceDefinitionsApi {
    pub fn new(
        resource_definitons_service: Arc<ResourceDefinitionService>,
        auth_service: Arc<AuthService>,
    ) -> Self {
        Self {
            resource_definitons_service,
            auth_service,
        }
    }

    /// Create a new resource in the environment
    #[oai(
        path = "/envs/:environment_id/resources",
        method = "post",
        operation_id = "create_resource",
        tag = ApiTags::Environment
    )]
    async fn create_resource(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: Json<ResourceDefinitionCreation>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ResourceDefinition>> {
        let record = recorded_http_api_request!(
            "create_resource",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .create_resource_internal(environment_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_resource_internal(
        &self,
        environment_id: EnvironmentId,
        payload: ResourceDefinitionCreation,
        auth: AuthCtx,
    ) -> ApiResult<Json<ResourceDefinition>> {
        let result = self
            .resource_definitons_service
            .create(environment_id, payload, &auth)
            .await?;

        Ok(Json(result))
    }

    /// Get all resources defined in the environment
    #[oai(
        path = "/envs/:environment_id/resources",
        method = "get",
        operation_id = "get_environment_resources",
        tag = ApiTags::Environment
    )]
    async fn get_environment_resources(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<ResourceDefinition>>> {
        let record = recorded_http_api_request!(
            "get_environment_resources",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_environment_resources_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_resources_internal(
        &self,
        environment_id: EnvironmentId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<ResourceDefinition>>> {
        let values = self
            .resource_definitons_service
            .list_in_environment(environment_id, &auth)
            .await?;

        Ok(Json(Page { values }))
    }

    /// Get a resource in the environment by name
    #[oai(
        path = "/envs/:environment_id/resources/:resource_name",
        method = "get",
        operation_id = "get_environment_resource",
        tag = ApiTags::Environment
    )]
    async fn get_environment_resource(
        &self,
        environment_id: Path<EnvironmentId>,
        resource_name: Path<ResourceName>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ResourceDefinition>> {
        let record = recorded_http_api_request!(
            "get_environment_resource",
            environment_id = environment_id.0.to_string(),
            resource_name = resource_name.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_environment_resource_internal(environment_id.0, resource_name.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_resource_internal(
        &self,
        environment_id: EnvironmentId,
        resource_name: ResourceName,
        auth: AuthCtx,
    ) -> ApiResult<Json<ResourceDefinition>> {
        let result = self
            .resource_definitons_service
            .get_in_environment(environment_id, &resource_name, &auth)
            .await?;

        Ok(Json(result))
    }

    /// Get all resources in a specific deployment
    #[oai(
        path = "/envs/:environment_id/deployments/:deployment_revision/resources",
        method = "get",
        operation_id = "get_deployment_resources",
        tag = ApiTags::Environment,
        tag = ApiTags::Deployment
    )]
    async fn get_deployment_resources(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision: Path<DeploymentRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<ResourceDefinition>>> {
        let record = recorded_http_api_request!(
            "get_deployment_resources",
            environment_id = environment_id.0.to_string(),
            deployment_revision = deployment_revision.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_deployment_resources_internal(environment_id.0, deployment_revision.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_deployment_resources_internal(
        &self,
        _environment_id: EnvironmentId,
        _deployment_revision: DeploymentRevision,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Page<ResourceDefinition>>> {
        unimplemented!()
    }

    /// Get resource in a deployment by name
    #[oai(
        path = "/envs/:environment_id/deployments/:deployment_revision/resources/:resource_name",
        method = "get",
        operation_id = "get_deployment_resource",
        tag = ApiTags::Environment,
        tag = ApiTags::Deployment
    )]
    async fn get_deployment_resource(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision: Path<DeploymentRevision>,
        resource_name: Path<ResourceName>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ResourceDefinition>> {
        let record = recorded_http_api_request!(
            "get_deployment_resource",
            environment_id = environment_id.0.to_string(),
            deployment_revision = deployment_revision.0.to_string(),
            resource_name = resource_name.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_deployment_resource_internal(
                environment_id.0,
                deployment_revision.0,
                resource_name.0,
                auth,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_deployment_resource_internal(
        &self,
        _environment_id: EnvironmentId,
        _deployment_revision: DeploymentRevision,
        _resource_name: ResourceName,
        _auth: AuthCtx,
    ) -> ApiResult<Json<ResourceDefinition>> {
        unimplemented!()
    }

    /// Get a resource by id
    #[oai(
        path = "/resources/:resource_id",
        method = "get",
        operation_id = "get_resource"
    )]
    async fn get_resource(
        &self,
        resource_id: Path<ResourceDefinitionId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ResourceDefinition>> {
        let record =
            recorded_http_api_request!("get_resource", resource_id = resource_id.0.to_string());

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_resource_internal(resource_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_resource_internal(
        &self,
        resource_id: ResourceDefinitionId,
        auth: AuthCtx,
    ) -> ApiResult<Json<ResourceDefinition>> {
        let result = self
            .resource_definitons_service
            .get(resource_id, &auth)
            .await?;

        Ok(Json(result))
    }

    /// Get specific revision of a resource
    #[oai(
        path = "/resources/:resource_id/revisions/:revision",
        method = "get",
        operation_id = "get_resource_revision"
    )]
    async fn get_resource_revision(
        &self,
        resource_id: Path<ResourceDefinitionId>,
        revision: Path<ResourceDefinitionRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ResourceDefinition>> {
        let record = recorded_http_api_request!(
            "get_resource_revision",
            resource_id = resource_id.0.to_string(),
            revision = revision.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_resource_revision_internal(resource_id.0, revision.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_resource_revision_internal(
        &self,
        resource_id: ResourceDefinitionId,
        revision: ResourceDefinitionRevision,
        auth: AuthCtx,
    ) -> ApiResult<Json<ResourceDefinition>> {
        let result = self
            .resource_definitons_service
            .get_revision(resource_id, revision, &auth)
            .await?;

        Ok(Json(result))
    }

    /// Update a resource
    #[oai(
        path = "/resources/:resource_id",
        method = "patch",
        operation_id = "update_resource"
    )]
    async fn update_resource(
        &self,
        resource_id: Path<ResourceDefinitionId>,
        payload: Json<ResourceDefinitionUpdate>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ResourceDefinition>> {
        let record =
            recorded_http_api_request!("update_resource", resource_id = resource_id.0.to_string(),);

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .update_resource_internal(resource_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_resource_internal(
        &self,
        resource_id: ResourceDefinitionId,
        payload: ResourceDefinitionUpdate,
        auth: AuthCtx,
    ) -> ApiResult<Json<ResourceDefinition>> {
        let result = self
            .resource_definitons_service
            .update(resource_id, payload, &auth)
            .await?;

        Ok(Json(result))
    }

    /// Delete a resource
    #[oai(
        path = "/resources/:resource_id",
        method = "delete",
        operation_id = "delete_resource"
    )]
    async fn delete_resource(
        &self,
        resource_id: Path<ResourceDefinitionId>,
        current_revision: Query<ResourceDefinitionRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<NoContentResponse> {
        let record =
            recorded_http_api_request!("delete_resource", resource_id = resource_id.0.to_string(),);

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .delete_resource_internal(resource_id.0, current_revision.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_resource_internal(
        &self,
        resource_id: ResourceDefinitionId,
        current_revision: ResourceDefinitionRevision,
        auth: AuthCtx,
    ) -> ApiResult<NoContentResponse> {
        self.resource_definitons_service
            .delete(resource_id, current_revision, &auth)
            .await?;

        Ok(NoContentResponse::NoContent)
    }
}
