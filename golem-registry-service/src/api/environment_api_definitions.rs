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
use golem_common::api::Page;
use golem_common::api::api_definition::{
    CreateHttpApiDefinitionRequest, HttpApiDefinitionResponseView,
};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

pub struct EnvironmentApiDefinitionsApi {
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1/envs",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Environment,
    tag = ApiTags::ApiDefinition
)]
impl EnvironmentApiDefinitionsApi {
    pub fn new(auth_service: Arc<AuthService>) -> Self {
        Self { auth_service }
    }

    /// Create a new api-definition in the environment
    #[oai(
        path = "/:environment_id/api-definitions",
        method = "post",
        operation_id = "create_api_definition"
    )]
    async fn create_api_definition(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: Json<CreateHttpApiDefinitionRequest>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<HttpApiDefinitionResponseView>> {
        let record = recorded_http_api_request!(
            "create_api_definition",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .create_api_definition_internal(environment_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_api_definition_internal(
        &self,
        _environment_id: EnvironmentId,
        _payload: CreateHttpApiDefinitionRequest,
        _auth: AuthCtx,
    ) -> ApiResult<Json<HttpApiDefinitionResponseView>> {
        todo!()
    }

    /// Get all api-definitions in the environment
    #[oai(
        path = "/:environment_id/api-definitions",
        method = "get",
        operation_id = "get_environment_api_definitions"
    )]
    async fn get_environment_api_definitions(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<HttpApiDefinitionResponseView>>> {
        let record = recorded_http_api_request!(
            "get_environment_api_definitions",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_environment_api_definitions_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_api_definitions_internal(
        &self,
        _environment_id: EnvironmentId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Page<HttpApiDefinitionResponseView>>> {
        todo!()
    }

    /// Get api-definition by name
    #[oai(
        path = "/:environment_id/api-definitions/:api_definition_name",
        method = "get",
        operation_id = "get_environment_api_definition"
    )]
    async fn get_environment_api_definition(
        &self,
        environment_id: Path<EnvironmentId>,
        api_definition_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<HttpApiDefinitionResponseView>> {
        let record = recorded_http_api_request!(
            "get_environment_api_definition",
            environment_id = environment_id.0.to_string(),
            api_definition_name = api_definition_name.0
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_environment_api_definition_internal(environment_id.0, api_definition_name.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_api_definition_internal(
        &self,
        _environment_id: EnvironmentId,
        _api_definition_name: String,
        _auth: AuthCtx,
    ) -> ApiResult<Json<HttpApiDefinitionResponseView>> {
        todo!()
    }

    /// Get all api-definitions in a specific deployment
    #[oai(
        path = "/:environment_id/deployments/:deployment_revision_id/api-definitions",
        method = "get",
        operation_id = "get_deployment_api_definitions",
        tag = ApiTags::Deployment
    )]
    async fn get_deployment_api_definitions(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision_id: Path<DeploymentRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<HttpApiDefinitionResponseView>>> {
        let record = recorded_http_api_request!(
            "get_deployment_api_definitions",
            environment_id = environment_id.0.to_string(),
            deployment_revision_id = deployment_revision_id.0.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_deployment_api_definitions_internal(
                environment_id.0,
                deployment_revision_id.0,
                auth,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_deployment_api_definitions_internal(
        &self,
        _environment_id: EnvironmentId,
        _deployment_revision_id: DeploymentRevision,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Page<HttpApiDefinitionResponseView>>> {
        todo!()
    }

    /// Get api-definition in a deployment by name
    #[oai(
        path = "/:environment_id/deployments/:deployment_revision_id/api-definitions/:api_definition_name",
        method = "get",
        operation_id = "get_deployment_api_definition",
        tag = ApiTags::Deployment
    )]
    async fn get_deployment_api_definition(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision_id: Path<DeploymentRevision>,
        api_definition_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<HttpApiDefinitionResponseView>> {
        let record = recorded_http_api_request!(
            "get_deployment_api_definition",
            environment_id = environment_id.0.to_string(),
            deployment_revision_id = deployment_revision_id.0.0,
            api_definition_name = api_definition_name.0
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_deployment_api_definition_internal(
                environment_id.0,
                deployment_revision_id.0,
                api_definition_name.0,
                auth,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_deployment_api_definition_internal(
        &self,
        _environment_id: EnvironmentId,
        _deployment_revision_id: DeploymentRevision,
        _api_definition_name: String,
        _auth: AuthCtx,
    ) -> ApiResult<Json<HttpApiDefinitionResponseView>> {
        todo!()
    }
}
