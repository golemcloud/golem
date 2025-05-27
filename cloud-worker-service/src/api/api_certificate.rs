use std::result::Result;
use std::sync::Arc;

use cloud_common::auth::{CloudAuthCtx, GolemSecurityScheme};

use crate::api::common::{ApiEndpointError, ApiTags};
use crate::model::{Certificate, CertificateId, CertificateRequest};
use crate::service::api_certificate::CertificateService;
use golem_common::model::ProjectId;
use golem_common::recorded_http_api_request;
use poem_openapi::param::Query;
use poem_openapi::payload::Json;
use poem_openapi::*;
use tracing::Instrument;

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

    /// Creates a new certificate
    ///
    /// A certificate is associated with a given Golem project and domain, and consists of
    /// a key pair.
    ///
    /// The created certificate will be associated with a certificate ID returned by this endpoint.
    #[oai(path = "/", method = "post", operation_id = "create_certificate")]
    async fn create(
        &self,
        payload: Json<CertificateRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Certificate>, ApiEndpointError> {
        let token = token.secret();
        let record = recorded_http_api_request!(
            "create_certificate",
            domain_name = payload.0.domain_name.to_string(),
            project_id = payload.0.project_id.to_string()
        );
        let response = self
            .certificate_service
            .create(&payload.0, &CloudAuthCtx::new(token))
            .instrument(record.span.clone())
            .await
            .map(Json)
            .map_err(|err| err.into());

        record.result(response)
    }

    /// Gets one or all certificates for a given project
    ///
    /// If `certificate-id` is not set, it returns all certificates associated with the project.
    /// If `certificate-id` is set, it returns a single certificate if it exists.
    #[oai(path = "/", method = "get", operation_id = "get_certificates")]
    async fn get(
        &self,
        #[oai(name = "project-id")] project_id_query: Query<ProjectId>,
        #[oai(name = "certificate-id")] certificate_id_query: Query<Option<CertificateId>>,
        security: GolemSecurityScheme,
    ) -> Result<Json<Vec<Certificate>>, ApiEndpointError> {
        let token = security.secret();
        let record = recorded_http_api_request!(
            "get_certificates",
            certificate_id = certificate_id_query.0.as_ref().map(|id| id.to_string()),
            project_id = project_id_query.0.to_string()
        );
        let response = self
            .certificate_service
            .get(
                project_id_query.0.clone(),
                certificate_id_query.0,
                &CloudAuthCtx::new(token),
            )
            .instrument(record.span.clone())
            .await
            .map(Json)
            .map_err(|err| err.into());

        record.result(response)
    }

    /// Deletes a certificate
    ///
    /// Deletes the certificate associated with the given certificate ID and project ID.
    #[oai(path = "/", method = "delete", operation_id = "delete_certificate")]
    async fn delete(
        &self,
        #[oai(name = "project-id")] project_id_query: Query<ProjectId>,
        #[oai(name = "certificate-id")] certificate_id_query: Query<CertificateId>,
        security: GolemSecurityScheme,
    ) -> Result<Json<String>, ApiEndpointError> {
        let token = security.secret();
        let record = recorded_http_api_request!(
            "delete_certificate",
            certificate_id = certificate_id_query.0.to_string(),
            project_id = project_id_query.0.to_string()
        );
        let response = self
            .certificate_service
            .delete(
                &project_id_query.0,
                &certificate_id_query.0,
                &CloudAuthCtx::new(token),
            )
            .instrument(record.span.clone())
            .await
            .map(|_| Json("ok".to_string()))
            .map_err(|err| err.into());
        record.result(response)
    }
}
