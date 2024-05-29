use std::sync::Arc;

use cloud_common::auth::GolemSecurityScheme;
use golem_common::model::ProjectId;
use poem_openapi::param::Query;
use poem_openapi::payload::Json;
use poem_openapi::*;

use crate::api::common::{ApiEndpointError, ApiTags};
use crate::model::{ApiDomain, DomainRequest};
use crate::service::api_domain::ApiDomainService;
use crate::service::auth::CloudAuthCtx;

pub struct ApiDomainApi {
    domain_service: Arc<dyn ApiDomainService + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/api/domains", tag = ApiTags::ApiDomain)]
impl ApiDomainApi {
    pub fn new(domain_service: Arc<dyn ApiDomainService + Sync + Send>) -> Self {
        Self { domain_service }
    }

    #[oai(path = "/", method = "put")]
    async fn create_or_update(
        &self,
        payload: Json<DomainRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<ApiDomain>, ApiEndpointError> {
        let token = token.secret();
        let domain = self
            .domain_service
            .create_or_update(&payload.0, &CloudAuthCtx::new(token))
            .await?;
        Ok(Json(domain))
    }

    #[oai(path = "/", method = "get")]
    async fn get(
        &self,
        #[oai(name = "project-id")] project_id_query: Query<ProjectId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<ApiDomain>>, ApiEndpointError> {
        let token = token.secret();
        let project_id = project_id_query.0;
        let values = self
            .domain_service
            .get(&project_id, &CloudAuthCtx::new(token))
            .await?;
        Ok(Json(values))
    }

    #[oai(path = "/", method = "delete")]
    async fn delete(
        &self,
        #[oai(name = "project-id")] project_id_query: Query<ProjectId>,
        #[oai(name = "domain")] domain_query: Query<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<String>, ApiEndpointError> {
        let token = token.secret();
        let project_id = project_id_query.0;
        let domain_name = domain_query.0;
        self.domain_service
            .delete(&project_id, &domain_name, &CloudAuthCtx::new(token))
            .await?;
        Ok(Json("API domain deleted".to_string()))
    }
}
