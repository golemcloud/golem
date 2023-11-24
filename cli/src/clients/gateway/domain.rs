use async_trait::async_trait;
use golem_gateway_client::apis::api_domain_api::{
    v1_api_domains_delete, v1_api_domains_get, v1_api_domains_put,
};
use golem_gateway_client::apis::configuration::Configuration;
use golem_gateway_client::models::{ApiDomain, DomainRequest};
use tracing::info;

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

pub struct DomainClientLive {
    pub configuration: Configuration,
}

#[async_trait]
impl DomainClient for DomainClientLive {
    async fn get(&self, project_id: ProjectId) -> Result<Vec<ApiDomain>, GolemError> {
        info!(
            "Calling v1_api_domains_get for project_id {project_id:?} on base url {}",
            self.configuration.base_path
        );
        Ok(v1_api_domains_get(&self.configuration, &project_id.0.to_string()).await?)
    }

    async fn update(
        &self,
        project_id: ProjectId,
        domain_name: String,
    ) -> Result<ApiDomain, GolemError> {
        info!("Calling v1_api_domains_get for project_id {project_id:?}, domain_name {domain_name} on base url {}", self.configuration.base_path);
        Ok(v1_api_domains_put(
            &self.configuration,
            DomainRequest {
                project_id: project_id.0,
                domain_name,
            },
        )
        .await?)
    }

    async fn delete(&self, project_id: ProjectId, domain_name: &str) -> Result<String, GolemError> {
        info!("Calling v1_api_domains_get for project_id {project_id:?}, domain_name {domain_name} on base url {}", self.configuration.base_path);
        Ok(
            v1_api_domains_delete(&self.configuration, &project_id.0.to_string(), domain_name)
                .await?,
        )
    }
}
