use async_trait::async_trait;
use golem_gateway_client::apis::api_certificate_api::{
    v1_api_certificates_delete, v1_api_certificates_get, v1_api_certificates_post,
};
use golem_gateway_client::apis::configuration::Configuration;
use golem_gateway_client::models::{Certificate, CertificateRequest};
use tracing::info;

use crate::model::{GolemError, ProjectId};

#[async_trait]
pub trait CertificateClient {
    async fn get(
        &self,
        project_id: ProjectId,
        certificate_id: Option<&str>,
    ) -> Result<Vec<Certificate>, GolemError>;

    async fn create(&self, certificate: CertificateRequest) -> Result<Certificate, GolemError>;

    async fn delete(
        &self,
        project_id: ProjectId,
        certificate_id: &str,
    ) -> Result<String, GolemError>;
}

pub struct CertificateClientLive {
    pub configuration: Configuration,
}

#[async_trait]
impl CertificateClient for CertificateClientLive {
    async fn get(
        &self,
        project_id: ProjectId,
        certificate_id: Option<&str>,
    ) -> Result<Vec<Certificate>, GolemError> {
        info!("Calling v1_api_certificates_get for project_id {project_id:?}, certificate_id {certificate_id:?} on base url {}", self.configuration.base_path);
        Ok(v1_api_certificates_get(
            &self.configuration,
            &project_id.0.to_string(),
            certificate_id,
        )
        .await?)
    }

    async fn create(&self, certificate: CertificateRequest) -> Result<Certificate, GolemError> {
        info!(
            "Calling v1_api_certificates_post on base url {}",
            self.configuration.base_path
        );
        Ok(v1_api_certificates_post(&self.configuration, certificate).await?)
    }

    async fn delete(
        &self,
        project_id: ProjectId,
        certificate_id: &str,
    ) -> Result<String, GolemError> {
        info!("Calling v1_api_certificates_delete for project_id {project_id:?}, certificate_id {certificate_id} on base url {}", self.configuration.base_path);
        Ok(v1_api_certificates_delete(
            &self.configuration,
            &project_id.0.to_string(),
            certificate_id,
        )
        .await?)
    }
}
