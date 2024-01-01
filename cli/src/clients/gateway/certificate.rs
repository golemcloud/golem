use async_trait::async_trait;
use golem_gateway_client::model::Certificate;
use golem_gateway_client::model::CertificateRequest;
use uuid::Uuid;

use crate::model::{GolemError, ProjectId};

#[async_trait]
pub trait CertificateClient {
    async fn get(
        &self,
        project_id: ProjectId,
        certificate_id: Option<&Uuid>,
    ) -> Result<Vec<Certificate>, GolemError>;

    async fn create(&self, certificate: CertificateRequest) -> Result<Certificate, GolemError>;

    async fn delete(
        &self,
        project_id: ProjectId,
        certificate_id: &Uuid,
    ) -> Result<String, GolemError>;
}

pub struct CertificateClientLive<C: golem_gateway_client::api::ApiCertificateClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_gateway_client::api::ApiCertificateClient + Sync + Send> CertificateClient
    for CertificateClientLive<C>
{
    async fn get(
        &self,
        project_id: ProjectId,
        certificate_id: Option<&Uuid>,
    ) -> Result<Vec<Certificate>, GolemError> {
        Ok(self.client.get(&project_id.0, certificate_id).await?)
    }

    async fn create(&self, certificate: CertificateRequest) -> Result<Certificate, GolemError> {
        Ok(self.client.post(&certificate).await?)
    }

    async fn delete(
        &self,
        project_id: ProjectId,
        certificate_id: &Uuid,
    ) -> Result<String, GolemError> {
        Ok(self.client.delete(&project_id.0, certificate_id).await?)
    }
}
