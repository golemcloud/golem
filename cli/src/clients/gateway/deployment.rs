use async_trait::async_trait;
use golem_gateway_client::apis::api_deployment_api::{
    v1_api_deployments_delete, v1_api_deployments_get, v1_api_deployments_put,
};
use golem_gateway_client::apis::configuration::Configuration;
use golem_gateway_client::models::ApiDeployment;
use tracing::info;

use crate::model::{GolemError, ProjectId};

#[async_trait]
pub trait DeploymentClient {
    async fn get(
        &self,
        project_id: ProjectId,
        api_definition_id: &str,
    ) -> Result<Vec<ApiDeployment>, GolemError>;
    async fn update(&self, api_deployment: ApiDeployment) -> Result<ApiDeployment, GolemError>;
    async fn delete(
        &self,
        project_id: ProjectId,
        api_definition_id: &str,
        site: &str,
    ) -> Result<String, GolemError>;
}

pub struct DeploymentClientLive {
    pub configuration: Configuration,
}

#[async_trait]
impl DeploymentClient for DeploymentClientLive {
    async fn get(
        &self,
        project_id: ProjectId,
        api_definition_id: &str,
    ) -> Result<Vec<ApiDeployment>, GolemError> {
        info!("Calling v1_api_deployments_get for project_id {project_id:?}, api_definition_id {api_definition_id} on base url: {}", self.configuration.base_path);
        Ok(v1_api_deployments_get(
            &self.configuration,
            &project_id.0.to_string(),
            api_definition_id,
        )
        .await?)
    }

    async fn update(&self, api_deployment: ApiDeployment) -> Result<ApiDeployment, GolemError> {
        info!(
            "Calling v1_api_deployments_put on base url: {}",
            self.configuration.base_path
        );
        Ok(v1_api_deployments_put(&self.configuration, api_deployment).await?)
    }

    async fn delete(
        &self,
        project_id: ProjectId,
        api_definition_id: &str,
        site: &str,
    ) -> Result<String, GolemError> {
        info!("Calling v1_api_deployments_delete for project_id {project_id:?}, api_definition_id {api_definition_id}, site {site} on base url: {}", self.configuration.base_path);
        Ok(v1_api_deployments_delete(
            &self.configuration,
            &project_id.0.to_string(),
            api_definition_id,
            site,
        )
        .await?)
    }
}
