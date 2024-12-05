use std::fmt::Display;

use std::io::Read;

use async_trait::async_trait;

use golem_cloud_client::model::HttpApiDefinitionRequest;

use golem_cli::clients::api_definition::ApiDefinitionClient;
use golem_cli::cloud::ProjectId;
use tokio::fs::read_to_string;
use tracing::info;

use crate::cloud::clients::errors::CloudGolemError;
use golem_cli::model::{
    decode_api_definition, ApiDefinitionFileFormat, ApiDefinitionId, ApiDefinitionVersion,
    GolemError, PathBufOrStdin,
};

#[derive(Clone)]
pub struct ApiDefinitionClientLive<C: golem_cloud_client::api::ApiDefinitionClient + Sync + Send> {
    pub client: C,
}

#[derive(Debug, Copy, Clone)]
enum Action {
    Create,
    Update,
    Import,
}

impl Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = match self {
            Action::Create => "Creating",
            Action::Update => "Updating",
            Action::Import => "Importing",
        };
        write!(f, "{}", str)
    }
}

async fn create_or_update_api_definition<
    C: golem_cloud_client::api::ApiDefinitionClient + Sync + Send,
>(
    action: Action,
    client: &C,
    path: PathBufOrStdin,
    project_id: &ProjectId,
    format: &ApiDefinitionFileFormat,
) -> Result<golem_client::model::HttpApiDefinitionResponseData, GolemError> {
    info!("{action} api definition from {path:?}");

    let definition_str: String = match path {
        PathBufOrStdin::Path(path) => read_to_string(path)
            .await
            .map_err(|e| GolemError(format!("Failed to read from file: {e:?}")))?,
        PathBufOrStdin::Stdin => {
            let mut content = String::new();

            let _ = std::io::stdin()
                .read_to_string(&mut content)
                .map_err(|e| GolemError(format!("Failed to read stdin: {e:?}")))?;

            content
        }
    };

    let res = match action {
        Action::Import => {
            let value = decode_api_definition(&definition_str, format)?;
            client
                .import_open_api_json(&project_id.0, &value)
                .await
                .map_err(CloudGolemError::from)?
        }
        Action::Create => {
            let value: HttpApiDefinitionRequest = decode_api_definition(&definition_str, format)?;

            client
                .create_definition_json(&project_id.0, &value)
                .await
                .map_err(CloudGolemError::from)?
        }
        Action::Update => {
            let value: HttpApiDefinitionRequest = decode_api_definition(&definition_str, format)?;

            client
                .update_definition_json(&project_id.0, &value.id, &value.version, &value)
                .await
                .map_err(CloudGolemError::from)?
        }
    };

    Ok(res)
}

#[async_trait]
impl<C: golem_cloud_client::api::ApiDefinitionClient + Sync + Send> ApiDefinitionClient
    for ApiDefinitionClientLive<C>
{
    type ProjectContext = ProjectId;

    async fn list(
        &self,
        id: Option<&ApiDefinitionId>,
        project: &Self::ProjectContext,
    ) -> Result<Vec<golem_client::model::HttpApiDefinitionResponseData>, GolemError> {
        info!("Getting api definitions");

        let definitions = self
            .client
            .list_definitions(&project.0, id.map(|id| id.0.as_str()))
            .await
            .map_err(CloudGolemError::from)?;

        Ok(definitions)
    }

    async fn get(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
        project: &Self::ProjectContext,
    ) -> Result<golem_client::model::HttpApiDefinitionResponseData, GolemError> {
        info!("Getting api definition for {}/{}", id.0, version.0);

        let definition = self
            .client
            .get_definition(&project.0, id.0.as_str(), version.0.as_str())
            .await
            .map_err(CloudGolemError::from)?;

        Ok(definition)
    }

    async fn create(
        &self,
        path: PathBufOrStdin,
        project: &Self::ProjectContext,
        format: &ApiDefinitionFileFormat,
    ) -> Result<golem_client::model::HttpApiDefinitionResponseData, GolemError> {
        create_or_update_api_definition(Action::Create, &self.client, path, project, format).await
    }

    async fn update(
        &self,
        path: PathBufOrStdin,
        project: &Self::ProjectContext,
        format: &ApiDefinitionFileFormat,
    ) -> Result<golem_client::model::HttpApiDefinitionResponseData, GolemError> {
        create_or_update_api_definition(Action::Update, &self.client, path, project, format).await
    }

    async fn import(
        &self,
        path: PathBufOrStdin,
        project: &Self::ProjectContext,
        format: &ApiDefinitionFileFormat,
    ) -> Result<golem_client::model::HttpApiDefinitionResponseData, GolemError> {
        create_or_update_api_definition(Action::Import, &self.client, path, project, format).await
    }

    async fn delete(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
        project: &Self::ProjectContext,
    ) -> Result<String, GolemError> {
        info!("Deleting api definition for {}/{}", id.0, version.0);
        Ok(self
            .client
            .delete_definition(&project.0, id.0.as_str(), version.0.as_str())
            .await
            .map_err(CloudGolemError::from)?)
    }
}
