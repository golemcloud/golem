use std::sync::Arc;

use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::auth::DefaultNamespace;
use golem_worker_service_base::api::ApiDeployment;
use golem_worker_service_base::api::ApiEndpointError;
use golem_worker_service_base::api_definition;
use golem_worker_service_base::api_definition::{ApiDefinitionId, ApiSiteString};
use golem_worker_service_base::service::api_definition::ApiDefinitionIdWithVersion;
use golem_worker_service_base::service::api_deployment::ApiDeploymentService;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::*;
use tracing::Instrument;

pub struct ApiDeploymentApi {
    deployment_service: Arc<dyn ApiDeploymentService<DefaultNamespace> + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/api/deployments", tag = ApiTags::ApiDeployment)]
impl ApiDeploymentApi {
    pub fn new(
        deployment_service: Arc<dyn ApiDeploymentService<DefaultNamespace> + Sync + Send>,
    ) -> Self {
        Self { deployment_service }
    }

    #[oai(path = "/deploy", method = "post", operation_id = "deploy")]
    async fn create_or_update(
        &self,
        payload: Json<ApiDeployment>,
    ) -> Result<Json<ApiDeployment>, ApiEndpointError> {
        let record = recorded_http_api_request!("deploy", site = payload.0.site.to_string());
        let response = {
            let api_definition_infos = payload
                .api_definitions
                .iter()
                .map(|k| ApiDefinitionIdWithVersion {
                    id: k.id.clone(),
                    version: k.version.clone(),
                })
                .collect::<Vec<ApiDefinitionIdWithVersion>>();

            let api_deployment = api_definition::ApiDeployment {
                namespace: DefaultNamespace::default(),
                api_definition_keys: api_definition_infos,
                site: payload.site.clone(),
            };

            self.deployment_service
                .deploy(&api_deployment)
                .instrument(record.span.clone())
                .await?;

            let data = self
                .deployment_service
                .get_by_site(&ApiSiteString::from(&payload.site))
                .instrument(record.span.clone())
                .await?;

            let deployment = data.ok_or(ApiEndpointError::internal(
                "Failed to verify the deployment",
            ))?;

            Ok(Json(deployment.into()))
        };

        record.result(response)
    }

    #[oai(path = "/", method = "get", operation_id = "list_deployments")]
    async fn list(
        &self,
        #[oai(name = "api-definition-id")] api_definition_id_query: Query<ApiDefinitionId>,
    ) -> Result<Json<Vec<ApiDeployment>>, ApiEndpointError> {
        let record = recorded_http_api_request!(
            "list_deployments",
            api_definition_id = api_definition_id_query.0.to_string(),
        );
        let response = {
            let api_definition_id = api_definition_id_query.0;

            let values = self
                .deployment_service
                .get_by_id(&DefaultNamespace::default(), &api_definition_id)
                .await?;

            Ok(Json(values.iter().map(|v| v.clone().into()).collect()))
        };

        record.result(response)
    }

    #[oai(path = "/:site", method = "get", operation_id = "get_deployment")]
    async fn get(&self, site: Path<String>) -> Result<Json<ApiDeployment>, ApiEndpointError> {
        let record = recorded_http_api_request!("get_deployment", site = site.0);
        let response = {
            let site = site.0;

            let value = self
                .deployment_service
                .get_by_site(&ApiSiteString(site))
                .await?
                .ok_or(ApiEndpointError::not_found("Api deployment not found"))?;

            Ok(Json(value.into()))
        };

        record.result(response)
    }

    #[oai(path = "/:site", method = "delete", operation_id = "delete_deployment")]
    async fn delete(&self, site: Path<String>) -> Result<Json<String>, ApiEndpointError> {
        let record = recorded_http_api_request!("delete_deployment", site = site.0);
        let response = {
            let site = site.0;

            self.deployment_service
                .delete(&DefaultNamespace::default(), &ApiSiteString(site))
                .await?;

            Ok(Json("API deployment deleted".to_string()))
        };

        record.result(response)
    }
}
