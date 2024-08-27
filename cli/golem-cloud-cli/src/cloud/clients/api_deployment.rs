use async_trait::async_trait;

use golem_cli::clients::api_deployment::ApiDeploymentClient;
use golem_cli::cloud::ProjectId;
use golem_cloud_client::model::{ApiDefinitionInfo, ApiSite};
use tracing::info;

use crate::cloud::clients::errors::CloudGolemError;
use crate::cloud::model::ToCli;
use golem_cli::model::{ApiDefinitionId, ApiDefinitionIdWithVersion, ApiDeployment, GolemError};

#[derive(Clone)]
pub struct ApiDeploymentClientLive<C: golem_cloud_client::api::ApiDeploymentClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_cloud_client::api::ApiDeploymentClient + Sync + Send> ApiDeploymentClient
    for ApiDeploymentClientLive<C>
{
    type ProjectContext = ProjectId;

    async fn deploy(
        &self,
        definitions: Vec<ApiDefinitionIdWithVersion>,
        host: &str,
        subdomain: Option<String>,
        project: &Self::ProjectContext,
    ) -> Result<ApiDeployment, GolemError> {
        info!(
            "Deploying definitions to host {host} {}",
            subdomain
                .clone()
                .map_or("".to_string(), |s| format!("subdomain {}", s))
        );

        let api_definition_infos = definitions
            .iter()
            .map(|d| ApiDefinitionInfo {
                id: d.id.0.clone(),
                version: d.version.0.clone(),
            })
            .collect::<Vec<_>>();

        let deployment = golem_cloud_client::model::ApiDeploymentRequest {
            api_definitions: api_definition_infos,
            project_id: project.0,
            site: ApiSite {
                host: host.to_string(),
                subdomain,
            },
        };

        Ok(self
            .client
            .deploy(&deployment)
            .await
            .map_err(CloudGolemError::from)?
            .to_cli())
    }

    async fn list(
        &self,
        api_definition_id: &ApiDefinitionId,
        project: &Self::ProjectContext,
    ) -> Result<Vec<ApiDeployment>, GolemError> {
        info!("List api deployments with definition {api_definition_id}");

        let deployments = self
            .client
            .list_deployments(&project.0, &api_definition_id.0)
            .await
            .map_err(CloudGolemError::from)?;

        Ok(deployments.into_iter().map(|d| d.to_cli()).collect())
    }

    async fn get(&self, site: &str) -> Result<ApiDeployment, GolemError> {
        info!("Getting api deployment for site {site}");

        Ok(self
            .client
            .get_deployment(site)
            .await
            .map_err(CloudGolemError::from)?
            .to_cli())
    }

    async fn delete(&self, site: &str) -> Result<String, GolemError> {
        info!("Deleting api deployment for site {site}");

        Ok(self
            .client
            .delete_deployment(site)
            .await
            .map_err(CloudGolemError::from)?)
    }
}
