use crate::cloud::clients::domain::DomainClient;
use crate::cloud::model::text::api_domain::{ApiDomainAddView, ApiDomainListView};
use crate::cloud::model::ProjectRef;
use crate::cloud::service::project::ProjectService;
use async_trait::async_trait;
use golem_cli::model::{GolemError, GolemResult};
use std::sync::Arc;

#[async_trait]
pub trait DomainService {
    async fn get(&self, project_ref: ProjectRef) -> Result<GolemResult, GolemError>;
    async fn add(
        &self,
        project_ref: ProjectRef,
        domain_name: String,
    ) -> Result<GolemResult, GolemError>;
    async fn delete(
        &self,
        project_ref: ProjectRef,
        domain_name: String,
    ) -> Result<GolemResult, GolemError>;
}

pub struct DomainServiceLive {
    pub client: Box<dyn DomainClient + Send + Sync>,
    pub projects: Arc<dyn ProjectService + Send + Sync>,
}

#[async_trait]
impl DomainService for DomainServiceLive {
    async fn get(&self, project_ref: ProjectRef) -> Result<GolemResult, GolemError> {
        let project_urn = self.projects.resolve_urn_or_default(project_ref).await?;

        let res = self.client.get(project_urn).await?;

        Ok(GolemResult::Ok(Box::new(ApiDomainListView(res))))
    }

    async fn add(
        &self,
        project_ref: ProjectRef,
        domain_name: String,
    ) -> Result<GolemResult, GolemError> {
        let project_urn = self.projects.resolve_urn_or_default(project_ref).await?;

        let res = self.client.update(project_urn, domain_name).await?;

        Ok(GolemResult::Ok(Box::new(ApiDomainAddView(res))))
    }

    async fn delete(
        &self,
        project_ref: ProjectRef,
        domain_name: String,
    ) -> Result<GolemResult, GolemError> {
        let project_urn = self.projects.resolve_urn_or_default(project_ref).await?;
        let res = self.client.delete(project_urn, &domain_name).await?;
        Ok(GolemResult::Str(res))
    }
}
