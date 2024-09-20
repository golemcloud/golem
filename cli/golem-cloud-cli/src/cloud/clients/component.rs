use std::io::Read;

use async_trait::async_trait;
use golem_cli::clients::component::ComponentClient;
use golem_cli::cloud::ProjectId;
use golem_cloud_client::model::ComponentQuery;
use tokio::fs::File;
use tracing::info;

use crate::cloud::clients::errors::CloudGolemError;
use crate::cloud::model::to_cli::ToCli;
use crate::cloud::model::to_cloud::ToCloud;
use golem_cli::model::component::Component;
use golem_cli::model::{ComponentName, GolemError, PathBufOrStdin};
use golem_client::model::ComponentType;
use golem_common::uri::oss::urn::ComponentUrn;

#[derive(Debug, Clone)]
pub struct ComponentClientLive<C: golem_cloud_client::api::ComponentClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_cloud_client::api::ComponentClient + Sync + Send> ComponentClient
    for ComponentClientLive<C>
{
    type ProjectContext = ProjectId;

    async fn get_metadata(
        &self,
        component_urn: &ComponentUrn,
        version: u64,
    ) -> Result<Component, GolemError> {
        info!("Getting component version");
        let component = self
            .client
            .get_component_metadata(&component_urn.id.0, &version.to_string())
            .await
            .map_err(CloudGolemError::from)?;
        Ok(component.to_cli())
    }

    async fn get_latest_metadata(
        &self,
        component_urn: &ComponentUrn,
    ) -> Result<Component, GolemError> {
        info!("Getting latest component version");

        let component = self
            .client
            .get_latest_component_metadata(&component_urn.id.0)
            .await
            .map_err(CloudGolemError::from)?;
        Ok(component.to_cli())
    }

    async fn find(
        &self,
        name: Option<ComponentName>,
        project: &Option<Self::ProjectContext>,
    ) -> Result<Vec<Component>, GolemError> {
        info!("Getting components");

        let project_id = project.map(|p| p.0);
        let name = name.map(|n| n.0);

        let components = self
            .client
            .get_components(project_id.as_ref(), name.as_deref())
            .await
            .map_err(CloudGolemError::from)?;
        Ok(components.into_iter().map(|c| c.to_cli()).collect())
    }

    async fn add(
        &self,
        name: ComponentName,
        file: PathBufOrStdin,
        project: &Option<Self::ProjectContext>,
        component_type: ComponentType,
    ) -> Result<Component, GolemError> {
        info!("Adding component {name:?} from {file:?}");

        let query = ComponentQuery {
            project_id: project.map(|ProjectId(id)| id),
            component_name: name.0,
            component_type: Some(component_type.to_cloud()),
        };

        let component = match file {
            PathBufOrStdin::Path(path) => {
                let file = File::open(path)
                    .await
                    .map_err(|e| GolemError(format!("Can't open component file: {e}")))?;

                self.client
                    .create_component(&query, file)
                    .await
                    .map_err(CloudGolemError::from)?
            }
            PathBufOrStdin::Stdin => {
                let mut bytes = Vec::new();

                let _ = std::io::stdin()
                    .read_to_end(&mut bytes) // TODO: steaming request from stdin
                    .map_err(|e| GolemError(format!("Failed to read stdin: {e:?}")))?;

                self.client
                    .create_component(&query, bytes)
                    .await
                    .map_err(CloudGolemError::from)?
            }
        };

        Ok(component.to_cli())
    }

    async fn update(
        &self,
        urn: ComponentUrn,
        file: PathBufOrStdin,
        component_type: Option<ComponentType>,
    ) -> Result<Component, GolemError> {
        info!("Updating component {urn} from {file:?}");

        let component = match file {
            PathBufOrStdin::Path(path) => {
                let file = File::open(path)
                    .await
                    .map_err(|e| GolemError(format!("Can't open component file: {e}")))?;

                self.client
                    .update_component(&urn.id.0, component_type.to_cloud().as_ref(), file)
                    .await
                    .map_err(CloudGolemError::from)?
            }
            PathBufOrStdin::Stdin => {
                let mut bytes = Vec::new();

                let _ = std::io::stdin()
                    .read_to_end(&mut bytes) // TODO: steaming request from stdin
                    .map_err(|e| GolemError(format!("Failed to read stdin: {e:?}")))?;

                self.client
                    .update_component(&urn.id.0, component_type.to_cloud().as_ref(), bytes)
                    .await
                    .map_err(CloudGolemError::from)?
            }
        };

        Ok(component.to_cli())
    }
}
