// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use golem_common::SafeDisplay;
use golem_common::{recorded_http_api_request, safe};
use golem_service_base::api_tags::ApiTags;
use golem_service_base::auth::{DefaultNamespace, EmptyAuthCtx};
use golem_worker_service_base::api::ApiEndpointError;
use golem_worker_service_base::api::{ApiDeployment, ApiDeploymentRequest};
use golem_worker_service_base::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use golem_worker_service_base::gateway_api_deployment;
use golem_worker_service_base::gateway_api_deployment::ApiSiteString;
use golem_worker_service_base::service::gateway::api_definition::ApiDefinitionIdWithVersion;
use golem_worker_service_base::service::gateway::api_deployment::ApiDeploymentError;
use golem_worker_service_base::service::gateway::api_deployment::ApiDeploymentService;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

pub struct ApiDeploymentApi {
    deployment_service: Arc<dyn ApiDeploymentService<EmptyAuthCtx, DefaultNamespace> + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/api/deployments", tag = ApiTags::ApiDeployment)]
impl ApiDeploymentApi {
    pub fn new(
        deployment_service: Arc<
            dyn ApiDeploymentService<EmptyAuthCtx, DefaultNamespace> + Sync + Send,
        >,
    ) -> Self {
        Self { deployment_service }
    }

    /// Creates or updates a deployment
    ///
    /// Deploys a set of API definitions to a site (specific host and subdomain).
    #[oai(path = "/deploy", method = "post", operation_id = "deploy")]
    async fn create_or_update(
        &self,
        payload: Json<ApiDeploymentRequest>,
    ) -> Result<Json<ApiDeployment>, ApiEndpointError> {
        let record = recorded_http_api_request!("deploy", site = payload.0.site.to_string());

        let response = self
            .create_or_update_internal(payload.0)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn create_or_update_internal(
        &self,
        payload: ApiDeploymentRequest,
    ) -> Result<Json<ApiDeployment>, ApiEndpointError> {
        let namespace = DefaultNamespace::default();
        let api_definition_infos = payload
            .api_definitions
            .iter()
            .map(|k| ApiDefinitionIdWithVersion {
                id: k.id.clone(),
                version: k.version.clone(),
            })
            .collect::<Vec<ApiDefinitionIdWithVersion>>();

        let api_deployment = gateway_api_deployment::ApiDeploymentRequest {
            namespace: namespace.clone(),
            api_definition_keys: api_definition_infos,
            site: payload.site.clone(),
        };

        self.deployment_service
            .deploy(&api_deployment, &EmptyAuthCtx::default())
            .await?;

        let data = self
            .deployment_service
            .get_by_site(&namespace, &ApiSiteString::from(&payload.site))
            .await?;

        let deployment = data.ok_or(ApiEndpointError::internal(safe(
            "Failed to verify the deployment".to_string(),
        )))?;

        Ok(Json(deployment.into()))
    }

    /// Get one or more API deployments
    ///
    /// If `api-definition-id` is not set, it lists all API deployments.
    /// If `api-definition-id` is set, returns a single API deployment.
    #[oai(path = "/", method = "get", operation_id = "list_deployments")]
    async fn list(
        &self,
        #[oai(name = "api-definition-id")] api_definition_id_query: Query<Option<ApiDefinitionId>>,
    ) -> Result<Json<Vec<ApiDeployment>>, ApiEndpointError> {
        let record = recorded_http_api_request!(
            "list_deployments",
            api_definition_id = api_definition_id_query
                .0
                .clone()
                .unwrap_or(ApiDefinitionId("".to_string()))
                .to_string(),
        );
        let response = self
            .list_internal(api_definition_id_query.0)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn list_internal(
        &self,
        api_definition_id: Option<ApiDefinitionId>,
    ) -> Result<Json<Vec<ApiDeployment>>, ApiEndpointError> {
        let values = self
            .deployment_service
            .get_by_id(&DefaultNamespace::default(), api_definition_id)
            .await?;

        Ok(Json(values.iter().map(|v| v.clone().into()).collect()))
    }

    /// Get API deployment by site
    ///
    /// Gets an API deployment by the host name (optionally with a subdomain) it is deployed to.
    #[oai(path = "/:site", method = "get", operation_id = "get_deployment")]
    async fn get(&self, site: Path<String>) -> Result<Json<ApiDeployment>, ApiEndpointError> {
        let record = recorded_http_api_request!("get_deployment", site = site.0);

        let response = self
            .get_internal(site.0)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn get_internal(&self, site: String) -> Result<Json<ApiDeployment>, ApiEndpointError> {
        let value = self
            .deployment_service
            .get_by_site(&DefaultNamespace(), &ApiSiteString(site))
            .await?
            .ok_or(ApiEndpointError::not_found(safe(
                "Api deployment not found".to_string(),
            )))?;

        Ok(Json(value.into()))
    }

    /// Delete API deployment by site
    ///
    /// Deletes an API deployment by the host name (optionally with a subdomain) it is deployed to.
    #[oai(path = "/:site", method = "delete", operation_id = "delete_deployment")]
    async fn delete(&self, site: Path<String>) -> Result<Json<String>, ApiEndpointError> {
        let record = recorded_http_api_request!("delete_deployment", site = site.0);

        let response = self
            .delete_internal(site.0)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn delete_internal(&self, site: String) -> Result<Json<String>, ApiEndpointError> {
        self.deployment_service
            .delete(
                &DefaultNamespace::default(),
                &EmptyAuthCtx(),
                &ApiSiteString(site),
            )
            .await?;

        Ok(Json("API deployment deleted".to_string()))
    }

    /// Undeploy a single API definition from a site
    ///
    /// Removes a specific API definition (by id and version) from a site without deleting the entire deployment.
    #[oai(
        path = "/:site/:id/:version",
        method = "delete",
        operation_id = "undeploy_api"
    )]
    async fn undeploy_api(
        &self,
        site: Path<String>,
        id: Path<String>,
        version: Path<String>,
    ) -> Result<Json<String>, ApiEndpointError> {
        let record = recorded_http_api_request!(
            "undeploy_api",
            site = site.0.clone(),
            id = id.0.clone(),
            version = version.0.clone()
        );

        let response = self
            .undeploy_api_internal(site.0, id.0, version.0, &EmptyAuthCtx::default())
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn undeploy_api_internal(
        &self,
        site: String,
        id: String,
        version: String,
        auth_ctx: &EmptyAuthCtx,
    ) -> Result<Json<String>, ApiEndpointError> {
        let namespace = DefaultNamespace::default();
        let api_definition_key = ApiDefinitionIdWithVersion {
            id: ApiDefinitionId(id),
            version: ApiVersion(version),
        };

        // Pass ApiSiteString directly
        let api_site_string = ApiSiteString(site);

        self.deployment_service
            .undeploy(&namespace, api_site_string, api_definition_key, auth_ctx)
            .await
            .map_err(|err| match err {
                ApiDeploymentError::ApiDeploymentNotFound(_, _) => {
                    ApiEndpointError::not_found(safe("Site not found".to_string()))
                }
                ApiDeploymentError::ApiDefinitionNotFound(_, _, _) => {
                    ApiEndpointError::not_found(safe("API definition not found".to_string()))
                }
                _ => ApiEndpointError::internal(safe(err.to_safe_string())),
            })?;

        Ok(Json("API definition undeployed from site".to_string()))
    }
}
