use golem_common::{recorded_http_api_request, safe};
use golem_service_base::api_tags::ApiTags;
use golem_service_base::auth::DefaultNamespace;
use golem_worker_service_base::api::{ApiEndpointError, SecuritySchemeData};
use golem_worker_service_base::gateway_security::{SecurityScheme, SecuritySchemeIdentifier};
use golem_worker_service_base::service::gateway::security_scheme::SecuritySchemeService;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::OpenApi;
use std::sync::Arc;

use tracing::Instrument;

pub struct SecuritySchemeApi {
    security_scheme_service: Arc<dyn SecuritySchemeService<DefaultNamespace> + Sync + Send>,
}

impl SecuritySchemeApi {
    pub fn new(
        security_scheme_service: Arc<dyn SecuritySchemeService<DefaultNamespace> + Sync + Send>,
    ) -> Self {
        Self {
            security_scheme_service,
        }
    }
}

#[OpenApi(prefix_path = "/v1/api/security",  tag = ApiTags::ApiSecurity)]
impl SecuritySchemeApi {
    /// Get a security scheme
    ///
    /// Get a security scheme by name
    #[oai(
        path = "/:security_scheme_identifier",
        method = "get",
        operation_id = "get"
    )]
    async fn get(
        &self,
        security_scheme_identifier: Path<String>,
    ) -> Result<Json<SecuritySchemeData>, ApiEndpointError> {
        let record = recorded_http_api_request!(
            "get",
            security_scheme_identifier = security_scheme_identifier.0
        );
        let response = self
            .security_scheme_service
            .get(
                &SecuritySchemeIdentifier::new(security_scheme_identifier.0),
                &DefaultNamespace::default(),
            )
            .instrument(record.span.clone())
            .await
            .map_err(|err| err.into())
            .map(|security_scheme| Json(SecuritySchemeData::from(security_scheme)));

        record.result(response)
    }

    /// Create a security scheme
    #[oai(path = "/", method = "post", operation_id = "create")]
    async fn create(
        &self,
        payload: Json<SecuritySchemeData>,
    ) -> Result<Json<SecuritySchemeData>, ApiEndpointError> {
        let record = recorded_http_api_request!(
            "create",
            security_scheme_identifier = payload.0.scheme_identifier
        );
        let security_scheme = SecurityScheme::try_from(payload.0).map_err(|err| {
            ApiEndpointError::bad_request(safe(format!("Invalid security scheme {}", err)))
        })?;

        let response = self
            .security_scheme_service
            .create(&DefaultNamespace::default(), &security_scheme)
            .instrument(record.span.clone())
            .await
            .map_err(|err| err.into())
            .map(|security_scheme| Json(SecuritySchemeData::from(security_scheme)));

        record.result(response)
    }
}
