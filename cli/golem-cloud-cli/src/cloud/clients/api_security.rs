use async_trait::async_trait;

use golem_cli::clients::api_security::ApiSecurityClient;
use golem_cli::cloud::ProjectId;
use golem_cloud_client::model::SecuritySchemeData;
use tracing::info;

use crate::cloud::clients::errors::CloudGolemError;
use crate::cloud::model::to_cli::ToCli;
use golem_cli::model::{ApiSecurityScheme, GolemError};
use golem_client::model::Provider;

#[derive(Clone)]
pub struct ApiSecurityClientLive<C: golem_cloud_client::api::ApiSecurityClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_cloud_client::api::ApiSecurityClient + Sync + Send> ApiSecurityClient
    for ApiSecurityClientLive<C>
{
    type ProjectContext = ProjectId;

    async fn create(
        &self,
        id: String,
        provider_type: Provider,
        client_id: String,
        client_secret: String,
        scopes: Vec<String>,
        redirect_url: String,
        project: &Self::ProjectContext,
    ) -> Result<ApiSecurityScheme, GolemError> {
        info!("Creating security scheme {}", id);

        let provider_type = match provider_type {
            Provider::Google => golem_cloud_client::model::Provider::Google,
            Provider::Facebook => golem_cloud_client::model::Provider::Facebook,
            Provider::Microsoft => golem_cloud_client::model::Provider::Microsoft,
            Provider::Gitlab => golem_cloud_client::model::Provider::Gitlab,
        };
        let result = self
            .client
            .create(
                &project.0,
                &SecuritySchemeData {
                    scheme_identifier: id,
                    provider_type,
                    client_id,
                    client_secret,
                    scopes,
                    redirect_url,
                },
            )
            .await
            .map_err(CloudGolemError::from)?;

        Ok(result.to_cli())
    }

    async fn get(
        &self,
        id: &str,
        project: &Self::ProjectContext,
    ) -> Result<ApiSecurityScheme, GolemError> {
        info!("Getting api security for id {id}");

        Ok(self
            .client
            .get(&project.0, id)
            .await
            .map_err(CloudGolemError::from)?
            .to_cli())
    }
}
