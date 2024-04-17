use std::sync::Arc;

use golem_worker_service_base::api::ApiEndpointError;
use golem_worker_service_base::api_definition::{ApiDefinitionId, ApiSite};
use poem_openapi::param::Query;
use poem_openapi::payload::Json;
use poem_openapi::*;

use golem_service_base::api_tags::ApiTags;
use golem_worker_service_base::auth::CommonNamespace;
use tracing::log::info;

use golem_worker_service_base::api::ApiDeployment;
use golem_worker_service_base::api_definition;
use golem_worker_service_base::service::api_definition::ApiDefinitionKey;
use golem_worker_service_base::service::api_deployment::ApiDeploymentService;

pub struct ApiDeploymentApi {
    deployment_service: Arc<dyn ApiDeploymentService<CommonNamespace> + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/api/deployments", tag = ApiTags::ApiDeployment)]
impl ApiDeploymentApi {
    pub fn new(
        deployment_service: Arc<dyn ApiDeploymentService<CommonNamespace> + Sync + Send>,
    ) -> Self {
        Self { deployment_service }
    }

    #[oai(path = "/", method = "put")]
    async fn create_or_update(
        &self,
        payload: Json<ApiDeployment>,
    ) -> Result<Json<ApiDeployment>, ApiEndpointError> {
        info!(
            "Deploy API definition - id: {}, site: {}",
            payload.api_definition_id, payload.site
        );

        let api_deployment = api_definition::ApiDeployment {
            api_definition_id: ApiDefinitionKey {
                namespace: CommonNamespace::default(),
                id: payload.api_definition_id.clone(),
                version: payload.version.clone(),
            },
            site: payload.site.clone(),
        };

        self.deployment_service.deploy(&api_deployment).await?;

        let data = self.deployment_service.get_by_host(&payload.site).await?;

        let deployment = data.ok_or(ApiEndpointError::internal(
            "Failed to verify the deployment",
        ))?;

        Ok(Json(deployment.into()))
    }

    #[oai(path = "/", method = "get")]
    async fn get(
        &self,
        #[oai(name = "api-definition-id")] api_definition_id_query: Query<ApiDefinitionId>,
    ) -> Result<Json<Vec<ApiDeployment>>, ApiEndpointError> {
        let api_definition_id = api_definition_id_query.0;

        info!("Get API deployments - id: {}", api_definition_id);

        let values = self
            .deployment_service
            .get_by_id(&CommonNamespace::default(), &api_definition_id)
            .await?;

        Ok(Json(values.iter().map(|v| v.clone().into()).collect()))
    }

    #[oai(path = "/", method = "delete")]
    async fn delete(
        &self,
        #[oai(name = "site")] site_query: Query<String>,
    ) -> Result<Json<String>, ApiEndpointError> {
        let site = site_query.0;

        self.deployment_service
            .delete(&CommonNamespace::default(), &ApiSite(site))
            .await?;

        Ok(Json("API deployment deleted".to_string()))
    }
}
