use async_trait::async_trait;
use golem_gateway_client::apis::api_definition_api::{
    v1_api_definitions_delete, v1_api_definitions_get, v1_api_definitions_put,
};
use golem_gateway_client::apis::configuration::Configuration;
use golem_gateway_client::models::ApiDefinition;
use tracing::info;

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

pub struct DefinitionClientLive {
    pub configuration: Configuration,
}

#[async_trait]
impl DefinitionClient for DefinitionClientLive {
    async fn get(
        &self,
        project_id: ProjectId,
        api_definition_id: Option<&str>,
    ) -> Result<Vec<ApiDefinition>, GolemError> {
        info!("Calling v1_api_definitions_get for project_id {project_id:?}, api_definition_id {api_definition_id:?} on base url {}", self.configuration.base_path);
        Ok(v1_api_definitions_get(
            &self.configuration,
            &project_id.0.to_string(),
            api_definition_id,
        )
        .await?)
    }

    async fn update(&self, api_definition: ApiDefinition) -> Result<ApiDefinition, GolemError> {
        info!(
            "Calling v1_api_definitions_put on base url {}",
            self.configuration.base_path
        );
        Ok(v1_api_definitions_put(&self.configuration, api_definition).await?)
    }

    async fn delete(
        &self,
        project_id: ProjectId,
        api_definition_id: &str,
    ) -> Result<String, GolemError> {
        info!("Calling v1_api_definitions_delete for project_id {project_id:?}, api_definition_id {api_definition_id} on base url {}", self.configuration.base_path);
        Ok(v1_api_definitions_delete(
            &self.configuration,
            &project_id.0.to_string(),
            api_definition_id,
        )
        .await?)
    }
}
