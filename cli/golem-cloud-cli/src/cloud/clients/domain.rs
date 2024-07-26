use crate::cloud::clients::errors::CloudGolemError;
use async_trait::async_trait;
use golem_cli::cloud::ProjectId;
use golem_cloud_client::model::{ApiDomain, DomainRequest};

#[async_trait]
pub trait DomainClient {
    async fn get(&self, project_id: ProjectId) -> Result<Vec<ApiDomain>, CloudGolemError>;

    async fn update(
        &self,
        project_id: ProjectId,
        domain_name: String,
    ) -> Result<ApiDomain, CloudGolemError>;

    async fn delete(
        &self,
        project_id: ProjectId,
        domain_name: &str,
    ) -> Result<String, CloudGolemError>;
}

pub struct DomainClientLive<C: golem_cloud_client::api::ApiDomainClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_cloud_client::api::ApiDomainClient + Sync + Send> DomainClient
    for DomainClientLive<C>
{
    async fn get(&self, project_id: ProjectId) -> Result<Vec<ApiDomain>, CloudGolemError> {
        Ok(self.client.get_domains(&project_id.0).await?)
    }

    async fn update(
        &self,
        project_id: ProjectId,
        domain_name: String,
    ) -> Result<ApiDomain, CloudGolemError> {
        Ok(self
            .client
            .create_or_update_domain(&DomainRequest {
                project_id: project_id.0,
                domain_name,
            })
            .await?)
    }

    async fn delete(
        &self,
        project_id: ProjectId,
        domain_name: &str,
    ) -> Result<String, CloudGolemError> {
        Ok(self
            .client
            .delete_domain(&project_id.0, domain_name)
            .await?)
    }
}
