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
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::http_api_definition::HttpApiDefinitionRevision;
use golem_common::model::http_api_definition::{
    HttpApiDefinition, HttpApiDefinitionCreation, HttpApiDefinitionId, HttpApiDefinitionName,
    HttpApiDefinitionUpdate,
};
use golem_common::model::poem::NoContentResponse;
use golem_common::model::{Page, UntypedJsonBody};
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::*;

pub struct LegacyHttpApiDefinitionsApi;

#[OpenApi(
    prefix_path = "/stubs",
    tag = ApiTags::RegistryService,
    tag = ApiTags::HttpApiDefinition
)]
#[allow(unused_variables)]
impl LegacyHttpApiDefinitionsApi {
    /// Create a new api-definition in the environment
    #[oai(
        path = "/envs/:environment_id/http-api-definitions",
        method = "post",
        operation_id = "create_http_api_definition_legacy",
        tag = ApiTags::Environment,
    )]
    async fn create_http_api_definition(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: Json<HttpApiDefinitionCreation>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<HttpApiDefinition>> {
        unimplemented!()
    }

    /// Get http api definition
    #[oai(
        path = "/http-api-definitions/:http_api_definition_id",
        method = "get",
        operation_id = "get_http_api_definition_legacy"
    )]
    async fn get_http_api_definition(
        &self,
        http_api_definition_id: Path<HttpApiDefinitionId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<HttpApiDefinition>> {
        unimplemented!()
    }

    /// Update http api definition
    #[oai(
        path = "/http-api-definitions/:http_api_definition_id",
        method = "patch",
        operation_id = "update_http_api_definition_legacy"
    )]
    async fn update_http_api_definition(
        &self,
        http_api_definition_id: Path<HttpApiDefinitionId>,
        payload: Json<HttpApiDefinitionUpdate>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<HttpApiDefinition>> {
        unimplemented!()
    }

    /// Delete http api definition
    #[oai(
        path = "/http-api-definitions/:http_api_definition_id",
        method = "delete",
        operation_id = "delete_http_api_definition_legacy"
    )]
    async fn delete_http_api_definition(
        &self,
        http_api_definition_id: Path<HttpApiDefinitionId>,
        current_revision: Query<HttpApiDefinitionRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<NoContentResponse> {
        unimplemented!()
    }

    /// Get a specific http api definition revision
    #[oai(
        path = "/http-api-definitions/:http_api_definition_id/revisions/:revision",
        method = "get",
        operation_id = "get_http_api_definition_revision_legacy"
    )]
    async fn get_http_api_definition_revision(
        &self,
        http_api_definition_id: Path<HttpApiDefinitionId>,
        revision: Path<HttpApiDefinitionRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<HttpApiDefinition>> {
        unimplemented!()
    }

    /// Get http api definition by name in the environment
    #[oai(
        path = "/envs/:environment_id/http-api-definitions/:http_api_definition_name",
        method = "get",
        operation_id = "get_http_api_definition_in_environment_legacy",
        tag = ApiTags::Environment
    )]
    async fn get_http_api_definition_in_environment(
        &self,
        environment_id: Path<EnvironmentId>,
        http_api_definition_name: Path<HttpApiDefinitionName>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<HttpApiDefinition>> {
        unimplemented!()
    }

    /// Get http api definition by name in the deployment
    #[oai(
        path = "/envs/:environment_id/deployments/:deployment_revision/http-api-definitions/:http_api_definition_name",
        method = "get",
        operation_id = "get_http_api_definition_in_deployment_legacy",
        tag = ApiTags::Environment,
        tag = ApiTags::Deployment,
    )]
    async fn get_http_api_definition_in_deployment(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision: Path<DeploymentRevision>,
        http_api_definition_name: Path<HttpApiDefinitionName>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<HttpApiDefinition>> {
        unimplemented!()
    }

    /// Get openapi spec of http api definition in the deployment
    #[oai(
        path = "/envs/:environment_id/deployments/:deployment_revision/http-api-definitions/:http_api_definition_name/openapi",
        method = "get",
        operation_id = "get_openapi_of_http_api_definition_in_deployment_legacy",
        tag = ApiTags::Environment,
        tag = ApiTags::Deployment,
    )]
    async fn get_openapi_of_http_api_definition_in_deployment(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision: Path<DeploymentRevision>,
        http_api_definition_name: Path<HttpApiDefinitionName>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<UntypedJsonBody>> {
        unimplemented!()
        // let record = recorded_http_api_request!(
        //     "get_openapi_of_http_api_definition_in_deployment",
        //     environment_id = environment_id.0.to_string(),
        //     deployment_revision = deployment_revision.0.to_string(),
        //     http_api_definition_name = http_api_definition_name.0.to_string(),
        // );

        // let auth = self.auth_service.authenticate_token(token.secret()).await?;

        // let response = self
        //     .get_openapi_of_http_api_definition_in_deployment_internal(
        //         environment_id.0,
        //         deployment_revision.0,
        //         http_api_definition_name.0,
        //         auth,
        //     )
        //     .instrument(record.span.clone())
        //     .await;

        // record.result(response)
    }

    // async fn get_openapi_of_http_api_definition_in_deployment_internal(
    //     &self,
    //     environment_id: EnvironmentId,
    //     deployment_revision: DeploymentRevision,
    //     http_api_definition_name: HttpApiDefinitionName,
    //     auth: AuthCtx,
    // ) -> ApiResult<Json<UntypedJsonBody>> {
    //     let open_api_spec = self
    //         .deployed_routes_service
    //         .get_openapi_spec_for_http_api_definition(
    //             environment_id,
    //             deployment_revision,
    //             &http_api_definition_name,
    //             &auth,
    //         )
    //         .await?;

    //     let serialized = serde_json::to_value(open_api_spec.0).map_err(anyhow::Error::from)?;

    //     Ok(Json(UntypedJsonBody(serialized)))
    // }

    /// List http api definitions in the environment
    #[oai(
        path = "/envs/:environment_id/http-api-definitions",
        method = "get",
        operation_id = "list_environment_http_api_definitions_legacy",
        tag = ApiTags::Environment,
    )]
    async fn list_environment_http_api_definitions(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<HttpApiDefinition>>> {
        unimplemented!()
    }

    /// List http api definitions in the deployment
    #[oai(
        path = "/envs/:environment_id/deployments/:deployment_revision/http-api-definitions",
        method = "get",
        operation_id = "list_deployment_http_api_definitions_legacy",
        tag = ApiTags::Environment,
    )]
    async fn list_deployment_http_api_definitions(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision: Path<DeploymentRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<HttpApiDefinition>>> {
        unimplemented!()
    }
}
