use std::result::Result;
use std::sync::Arc;

use crate::api::common::{ApiEndpointError, ApiTags};
use crate::service::api_definition::ApiDefinitionService;
use crate::service::api_domain::RegisterDomainRoute;
use crate::service::auth::{CloudAuthCtx, CloudNamespace};
use cloud_common::auth::GolemSecurityScheme;
use golem_common::model::ProjectId;
use golem_worker_service_base::api::HttpApiDefinition;
use golem_worker_service_base::api_definition::http::HttpApiDefinition as CoreHttpApiDefinition;
use golem_worker_service_base::api_definition::{ApiDefinitionId, ApiSiteString, ApiVersion};
use golem_worker_service_base::service::api_deployment::ApiDeploymentService;
use poem_openapi::param::Query;
use poem_openapi::payload::Json;
use poem_openapi::*;
use tracing::info;

pub struct ApiDefinitionApi {
    definition_service: Arc<dyn ApiDefinitionService + Sync + Send>,
    deployment_service: Arc<dyn ApiDeploymentService<CloudNamespace> + Sync + Send>,
    domain_route: Arc<dyn RegisterDomainRoute + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/api/definitions", tag = ApiTags::ApiDefinition)]
impl ApiDefinitionApi {
    pub fn new(
        definition_service: Arc<dyn ApiDefinitionService + Sync + Send>,
        deployment_service: Arc<dyn ApiDeploymentService<CloudNamespace> + Sync + Send>,
        domain_route: Arc<dyn RegisterDomainRoute + Sync + Send>,
    ) -> Self {
        Self {
            definition_service,
            deployment_service,
            domain_route,
        }
    }

    #[oai(path = "/", method = "put")]
    async fn create_or_update(
        &self,
        payload: Json<HttpApiDefinition>,
        #[oai(name = "project-id")] project_id_query: Query<ProjectId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<HttpApiDefinition>, ApiEndpointError> {
        let project_id = &project_id_query.0;
        let token = token.secret();
        let api_definition_id = &payload.id;

        info!(
            "Save API definition - project: {}, id: {}",
            project_id, api_definition_id
        );

        let definition: CoreHttpApiDefinition = payload
            .0
            .clone()
            .try_into()
            .map_err(ApiEndpointError::bad_request)?;

        let _ = self
            .definition_service
            .register(project_id, &definition, &CloudAuthCtx::new(token))
            .await?;

        Ok(Json(payload.0))
    }

    #[oai(path = "/", method = "get")]
    async fn get(
        &self,
        #[oai(name = "project-id")] project_id_query: Query<ProjectId>,
        #[oai(name = "api-definition-id")] api_definition_id_query: Query<Option<ApiDefinitionId>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<HttpApiDefinition>>, ApiEndpointError> {
        let token = token.secret();
        let project_id = project_id_query.0;
        let api_definition_id_optional = api_definition_id_query.0;

        let auth_ctx = CloudAuthCtx::new(token);

        if let Some(api_definition_id) = api_definition_id_optional {
            info!(
                "Get API definition - project: {}, id: {}",
                project_id, api_definition_id
            );

            let (data, _) = self
                .definition_service
                .get_all_versions(&project_id, &api_definition_id, &auth_ctx)
                .await?;

            let values = data
                .into_iter()
                .map(|d| d.try_into().map_err(ApiEndpointError::internal))
                .collect::<Result<Vec<HttpApiDefinition>, ApiEndpointError>>()?;

            Ok(Json(values))
        } else {
            info!("Get API definitions - project: {}", project_id);

            let (data, _) = self
                .definition_service
                .get_all(&project_id, &auth_ctx)
                .await?;

            let values = data
                .into_iter()
                .map(|d| d.try_into().map_err(ApiEndpointError::internal))
                .collect::<Result<Vec<HttpApiDefinition>, ApiEndpointError>>()?;

            Ok(Json(values))
        }
    }

    #[oai(path = "/", method = "delete")]
    async fn delete(
        &self,
        #[oai(name = "project-id")] project_id_query: Query<ProjectId>,
        #[oai(name = "api-definition-id")] api_definition_id_query: Query<ApiDefinitionId>,
        #[oai(name = "version")] version: Query<ApiVersion>,
        token: GolemSecurityScheme,
    ) -> Result<Json<String>, ApiEndpointError> {
        let token = token.secret();
        let project_id = project_id_query.0;
        let api_definition_id = api_definition_id_query.0;
        info!(
            "Delete API definition - project: {}, id: {}",
            project_id, api_definition_id
        );

        let auth_ctx = CloudAuthCtx::new(token);

        let (value, namespace) = self
            .definition_service
            .get(&project_id, &api_definition_id, &version, &auth_ctx)
            .await?;

        match value {
            Some(_) => {
                let deployments = self
                    .deployment_service
                    .get_by_id(&namespace, &api_definition_id)
                    .await?;

                for deployment in deployments {
                    self.domain_route
                        .unregister(&deployment.site.host, &deployment.site.subdomain)
                        .await
                        .map_err(ApiEndpointError::from)?;

                    self.deployment_service
                        .delete(&namespace, &ApiSiteString(deployment.site.to_string()))
                        .await?;
                }

                self.definition_service
                    .delete(&project_id, &api_definition_id, &version, &auth_ctx)
                    .await?;

                Ok(Json("API definition deleted".to_string()))
            }
            None => Err(ApiEndpointError::not_found("API definition not found")),
        }
    }
}
