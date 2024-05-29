use std::result::Result;
use std::sync::Arc;

use cloud_common::auth::GolemSecurityScheme;

use golem_common::model::ProjectId;
use poem_openapi::param::Query;
use poem_openapi::payload::Json;
use poem_openapi::*;

use crate::api::common::{ApiEndpointError, ApiTags};
use crate::model::{Certificate, CertificateId, CertificateRequest};
use crate::service::api_certificate::CertificateService;
use crate::service::auth::CloudAuthCtx;

pub struct ApiCertificateApi {
    certificate_service: Arc<dyn CertificateService + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/api/certificates", tag = ApiTags::ApiCertificate)]
impl ApiCertificateApi {
    pub fn new(certificate_service: Arc<dyn CertificateService + Sync + Send>) -> Self {
        Self {
            certificate_service,
        }
    }

    #[oai(path = "/", method = "post")]
    async fn create(
        &self,
        payload: Json<CertificateRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Certificate>, ApiEndpointError> {
        let token = token.secret();
        let certificate = self
            .certificate_service
            .create(&payload.0, &CloudAuthCtx::new(token))
            .await?;

        Ok(Json(certificate))
    }

    #[oai(path = "/", method = "get")]
    async fn get(
        &self,
        #[oai(name = "project-id")] project_id_query: Query<ProjectId>,
        #[oai(name = "certificate-id")] certificate_id_query: Query<Option<CertificateId>>,
        security: GolemSecurityScheme,
    ) -> Result<Json<Vec<Certificate>>, ApiEndpointError> {
        let token = security.secret();
        let project_id = project_id_query.0;
        let certificate_id_optional = certificate_id_query.0;
        let values = self
            .certificate_service
            .get(
                project_id.clone(),
                certificate_id_optional,
                &CloudAuthCtx::new(token),
            )
            .await?;

        Ok(Json(values))
    }

    #[oai(path = "/", method = "delete")]
    async fn delete(
        &self,
        #[oai(name = "project-id")] project_id_query: Query<ProjectId>,
        #[oai(name = "certificate-id")] certificate_id_query: Query<CertificateId>,
        security: GolemSecurityScheme,
    ) -> Result<Json<String>, ApiEndpointError> {
        let token = security.secret();
        let project_id = project_id_query.0;
        let certificate_id = certificate_id_query.0;

        self.certificate_service
            .delete(&project_id, &certificate_id, &CloudAuthCtx::new(token))
            .await?;
        Ok(Json("ok".to_string()))
    }
}
