use async_trait::async_trait;
use golem_gateway_client::model::ApiDefinition;

use crate::model::{GolemError, ProjectId};

#[async_trait]
pub trait DefinitionClient {
    async fn get(
        &self,
        project_id: ProjectId,
        api_definition_id: Option<&str>,
    ) -> Result<Vec<ApiDefinition>, GolemError>;

    async fn update(&self, api_definition: ApiDefinition) -> Result<ApiDefinition, GolemError>;

    async fn delete(
        &self,
        project_id: ProjectId,
        api_definition_id: &str,
    ) -> Result<String, GolemError>;
}

pub struct DefinitionClientLive<C: golem_gateway_client::api::ApiDefinitionClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_gateway_client::api::ApiDefinitionClient + Sync + Send> DefinitionClient
    for DefinitionClientLive<C>
{
    async fn get(
        &self,
        project_id: ProjectId,
        api_definition_id: Option<&str>,
    ) -> Result<Vec<ApiDefinition>, GolemError> {
        Ok(self.client.get(&project_id.0, api_definition_id).await?)
    }

    async fn update(&self, api_definition: ApiDefinition) -> Result<ApiDefinition, GolemError> {
        Ok(self.client.put(&api_definition).await?)
    }

    async fn delete(
        &self,
        project_id: ProjectId,
        api_definition_id: &str,
    ) -> Result<String, GolemError> {
        Ok(self.client.delete(&project_id.0, api_definition_id).await?)
    }
}
