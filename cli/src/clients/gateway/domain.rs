use async_trait::async_trait;
use golem_gateway_client::model::ApiDomain;
use golem_gateway_client::model::DomainRequest;

use crate::model::{GolemError, ProjectId};

#[async_trait]
pub trait DomainClient {
    async fn get(&self, project_id: ProjectId) -> Result<Vec<ApiDomain>, GolemError>;

    async fn update(
        &self,
        project_id: ProjectId,
        domain_name: String,
    ) -> Result<ApiDomain, GolemError>;

    async fn delete(&self, project_id: ProjectId, domain_name: &str) -> Result<String, GolemError>;
}

pub struct DomainClientLive<C: golem_gateway_client::api::ApiDomainClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_gateway_client::api::ApiDomainClient + Sync + Send> DomainClient
    for DomainClientLive<C>
{
    async fn get(&self, project_id: ProjectId) -> Result<Vec<ApiDomain>, GolemError> {
        Ok(self.client.get(&project_id.0).await?)
    }

    async fn update(
        &self,
        project_id: ProjectId,
        domain_name: String,
    ) -> Result<ApiDomain, GolemError> {
        Ok(self
            .client
            .put(&DomainRequest {
                project_id: project_id.0,
                domain_name,
            })
            .await?)
    }

    async fn delete(&self, project_id: ProjectId, domain_name: &str) -> Result<String, GolemError> {
        Ok(self.client.delete(&project_id.0, domain_name).await?)
    }
}
