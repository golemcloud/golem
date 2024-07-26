use crate::cloud::clients::errors::CloudGolemError;
use async_trait::async_trait;
use golem_cli::cloud::ProjectId;
use golem_cloud_client::model::{Certificate, CertificateRequest};
use uuid::Uuid;

#[async_trait]
pub trait CertificateClient {
    async fn get(
        &self,
        project_id: ProjectId,
        certificate_id: Option<&Uuid>,
    ) -> Result<Vec<Certificate>, CloudGolemError>;

    async fn create(&self, certificate: CertificateRequest)
        -> Result<Certificate, CloudGolemError>;

    async fn delete(
        &self,
        project_id: ProjectId,
        certificate_id: &Uuid,
    ) -> Result<String, CloudGolemError>;
}

pub struct CertificateClientLive<C: golem_cloud_client::api::ApiCertificateClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_cloud_client::api::ApiCertificateClient + Sync + Send> CertificateClient
    for CertificateClientLive<C>
{
    async fn get(
        &self,
        project_id: ProjectId,
        certificate_id: Option<&Uuid>,
    ) -> Result<Vec<Certificate>, CloudGolemError> {
        Ok(self
            .client
            .get_certificates(&project_id.0, certificate_id)
            .await?)
    }

    async fn create(
        &self,
        certificate: CertificateRequest,
    ) -> Result<Certificate, CloudGolemError> {
        Ok(self.client.create_certificate(&certificate).await?)
    }

    async fn delete(
        &self,
        project_id: ProjectId,
        certificate_id: &Uuid,
    ) -> Result<String, CloudGolemError> {
        Ok(self
            .client
            .delete_certificate(&project_id.0, certificate_id)
            .await?)
    }
}
