use async_trait::async_trait;
use golem_gateway_client::model::ApiDeployment;

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

pub struct DeploymentClientLive<C: golem_gateway_client::api::ApiDeploymentClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_gateway_client::api::ApiDeploymentClient + Sync + Send> DeploymentClient
    for DeploymentClientLive<C>
{
    async fn get(
        &self,
        project_id: ProjectId,
        api_definition_id: &str,
    ) -> Result<Vec<ApiDeployment>, GolemError> {
        Ok(self.client.get(&project_id.0, api_definition_id).await?)
    }

    async fn update(&self, api_deployment: ApiDeployment) -> Result<ApiDeployment, GolemError> {
        Ok(self.client.put(&api_deployment).await?)
    }

    async fn delete(
        &self,
        project_id: ProjectId,
        api_definition_id: &str,
        site: &str,
    ) -> Result<String, GolemError> {
        Ok(self
            .client
            .delete(&project_id.0, api_definition_id, site)
            .await?)
    }
}
