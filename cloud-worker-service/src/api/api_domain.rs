use std::sync::Arc;

use crate::api::common::{ApiEndpointError, ApiTags};
use crate::model::{ApiDomain, DomainRequest};
use crate::service::api_domain::ApiDomainService;
use cloud_common::auth::{CloudAuthCtx, GolemSecurityScheme};
use golem_common::model::ProjectId;
use golem_common::recorded_http_api_request;
use poem_openapi::param::Query;
use poem_openapi::payload::Json;
use poem_openapi::*;
use tracing::Instrument;

pub struct ApiDomainApi {
    domain_service: Arc<dyn ApiDomainService + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/api/domains", tag = ApiTags::ApiDomain)]
impl ApiDomainApi {
    pub fn new(domain_service: Arc<dyn ApiDomainService + Sync + Send>) -> Self {
        Self { domain_service }
    }

    /// Create or update an API domain
    #[oai(path = "/", method = "put", operation_id = "create_or_update_domain")]
    async fn create_or_update(
        &self,
        payload: Json<DomainRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<ApiDomain>, ApiEndpointError> {
        let token = token.secret();
        let record = recorded_http_api_request!(
            "create_or_update_domain",
            domain_name = payload.0.domain_name.to_string(),
            project_id = payload.0.project_id.to_string()
        );
        let response = self
            .domain_service
            .create_or_update(&payload.0, &CloudAuthCtx::new(token))
            .instrument(record.span.clone())
            .await
            .map(Json)
            .map_err(|err| err.into());

        record.result(response)
    }

    /// Get all API domains
    ///
    /// Returns a list of API domains for the given project.
    #[oai(path = "/", method = "get", operation_id = "get_domains")]
    async fn get(
        &self,
        #[oai(name = "project-id")] project_id_query: Query<ProjectId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<ApiDomain>>, ApiEndpointError> {
        let token = token.secret();
        let record =
            recorded_http_api_request!("get_domains", project_id = project_id_query.0.to_string());
        let response = self
            .domain_service
            .get(&project_id_query.0, &CloudAuthCtx::new(token))
            .instrument(record.span.clone())
            .await
            .map(Json)
            .map_err(|err| err.into());

        record.result(response)
    }

    /// Delete an API domain
    #[oai(path = "/", method = "delete", operation_id = "delete_domain")]
    async fn delete(
        &self,
        #[oai(name = "project-id")] project_id_query: Query<ProjectId>,
        #[oai(name = "domain")] domain_query: Query<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<String>, ApiEndpointError> {
        let token = token.secret();
        let record = recorded_http_api_request!(
            "delete_domain",
            domain_name = domain_query.0,
            project_id = project_id_query.0.to_string()
        );
        let response = self
            .domain_service
            .delete(
                &project_id_query.0,
                &domain_query.0,
                &CloudAuthCtx::new(token),
            )
            .instrument(record.span.clone())
            .await
            .map(|_| Json("API domain deleted".to_string()))
            .map_err(|err| err.into());

        record.result(response)
    }
}
