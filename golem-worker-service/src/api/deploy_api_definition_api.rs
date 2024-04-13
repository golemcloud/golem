use std::sync::Arc;

use crate::api::common::{ApiEndpointError, ApiTags};
use crate::api_definition_service::ApiDefinitionService;
use crate::apispec::{AccountApiDeployment, ApiDeployment};
use crate::auth::CloudAuthCtx;
use crate::deploy::DeployApiDefinition;
use crate::domain_record::RegisterDomainRoute;
use cloud_common::auth::GolemSecurityScheme;
use golem_common::model::ProjectId;
use golem_worker_service_base::api_definition::ApiDefinitionId;
use poem_openapi::param::Query;
use poem_openapi::payload::Json;
use poem_openapi::*;
use tracing::log::error;
use tracing::log::info;

pub struct ApiDeploymentApi {
    definition_service: Arc<dyn ApiDefinitionService + Sync + Send>,
    deployment_service: Arc<dyn DeployApiDefinition + Sync + Send>,
    domain_route: Arc<dyn RegisterDomainRoute + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/api/deployments", tag = ApiTags::ApiDeployment)]
impl ApiDeploymentApi {
    pub fn new(
        definition_service: Arc<dyn ApiDefinitionService + Sync + Send>,
        deployment_service: Arc<dyn DeployApiDefinition + Sync + Send>,
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
        payload: Json<ApiDeployment>,
        token: GolemSecurityScheme,
    ) -> Result<Json<ApiDeployment>, ApiEndpointError> {
        let token = token.secret();
        let project_id = &payload.project_id;
        let version = &payload.version;

        info!(
            "Deploy API definition - project: {}, id: {}, site: {}",
            &payload.project_id, payload.api_definition_id, payload.site
        );

        let auth_ctx = CloudAuthCtx {
            project_id: project_id.clone(),
            token_secret: token.clone(),
        };

        let (api_definition_optional, namespace) = self
            .definition_service
            .get(&payload.api_definition_id, version, &auth_ctx)
            .await?;

        let _api_definition = match api_definition_optional {
            Some(definition) => {
                info!("Deploying API definition: {}", definition.id);
                definition
            }
            None => {
                error!(
                    "Deploy API definition - id: {}, site: {} - definition not found",
                    payload.api_definition_id, payload.site
                );

                return Err(ApiEndpointError::not_found(format!(
                    "API definition {} not found",
                    payload.api_definition_id
                )));
            }
        };

        let definition_account_id = namespace.account_id;

        let current_deployment_optional = self
            .deployment_service
            .get(payload.site.to_string().as_str())
            .await
            .map_err(ApiEndpointError::internal)?;

        if let Some(current_deployment) = current_deployment_optional {
            if current_deployment.account_id != definition_account_id
                || current_deployment.deployment.project_id != payload.project_id
                || current_deployment.deployment.api_definition_id != payload.api_definition_id
            {
                error!(
                    "Deploy API definition - account: {}, project: {}, id: {}, site: {} - site used by another API",
                    definition_account_id,
                    &payload.project_id,
                    payload.api_definition_id,
                    payload.site
                );

                return Err(ApiEndpointError::already_exists(
                    "API site used by another API",
                ));
            }
        }

        self.domain_route
            .register(&payload.site.host, &payload.site.subdomain)
            .await
            .map_err(ApiEndpointError::from)?;

        self.deployment_service
            .deploy(&AccountApiDeployment::new(
                &definition_account_id,
                &payload.0,
            ))
            .await
            .map_err(ApiEndpointError::internal)?;

        let data = self
            .deployment_service
            .get(payload.site.to_string().as_str())
            .await
            .map_err(ApiEndpointError::internal)?;

        let deployment = data
            .map(|d| d.deployment)
            .ok_or(ApiEndpointError::not_found("API Deployment not found"))?;

        Ok(Json(deployment))
    }

    #[oai(path = "/", method = "get")]
    async fn get(
        &self,
        #[oai(name = "project-id")] project_id_query: Query<ProjectId>,
        #[oai(name = "api-definition-id")] api_definition_id_query: Query<ApiDefinitionId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<ApiDeployment>>, ApiEndpointError> {
        let token = token.secret();
        let project_id = project_id_query.0;
        let api_definition_id = api_definition_id_query.0;

        info!("Get API deployments - id: {}", api_definition_id);

        let auth_ctx = CloudAuthCtx {
            project_id: project_id.clone(),
            token_secret: token.clone(),
        };

        let (api_definitions, namespace) = self
            .definition_service
            .get_all_versions(&api_definition_id, &auth_ctx)
            .await?;

        let optional_definition = api_definitions
            .first()
            .map(|_| namespace.account_id.clone());

        match optional_definition {
            Some(account_id) => {
                let data = self
                    .deployment_service
                    .get_by_id(&account_id, &project_id, &api_definition_id)
                    .await
                    .map_err(ApiEndpointError::internal)?;

                let values: Vec<ApiDeployment> =
                    data.iter().map(|d| d.deployment.clone()).collect();

                Ok(Json(values))
            }
            None => Err(ApiEndpointError::not_found("API definition not found")),
        }
    }

    #[oai(path = "/", method = "delete")]
    async fn delete(
        &self,
        #[oai(name = "project-id")] project_id_query: Query<ProjectId>,
        #[oai(name = "api-definition-id")] api_definition_id_query: Query<ApiDefinitionId>,
        #[oai(name = "site")] site_query: Query<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<String>, ApiEndpointError> {
        let token = token.secret();
        let project_id = project_id_query.0;
        let api_definition_id = api_definition_id_query.0;
        let site = site_query.0;

        info!(
            "Delete API deployment - id: {}, site: {}",
            api_definition_id, site
        );

        let auth_ctx = CloudAuthCtx {
            project_id: project_id.clone(),
            token_secret: token.clone(),
        };

        let (api_definitions, namespace) = self
            .definition_service
            .get_all_versions(&api_definition_id, &auth_ctx)
            .await?;

        let optional_definition = api_definitions
            .first()
            .map(|_| namespace.account_id.clone());

        match optional_definition {
            Some(account_id) => {
                let data = self
                    .deployment_service
                    .get(&site)
                    .await
                    .map_err(ApiEndpointError::internal)?;

                if let Some(deployment) = data {
                    if deployment.account_id == account_id
                        && deployment.deployment.project_id == project_id
                        && deployment.deployment.api_definition_id == api_definition_id
                    {
                        self.domain_route
                            .unregister(
                                &deployment.deployment.site.host,
                                &deployment.deployment.site.subdomain,
                            )
                            .await
                            .map_err(ApiEndpointError::from)?;

                        self.deployment_service
                            .delete(&site)
                            .await
                            .map_err(ApiEndpointError::internal)?;

                        Ok(Json("API deployment deleted".to_string()))
                    } else {
                        Err(ApiEndpointError::not_found(
                            "API deployment cannot be deleted",
                        ))
                    }
                } else {
                    Err(ApiEndpointError::not_found("API deployment not found"))
                }
            }
            None => Err(ApiEndpointError::not_found("API definition not found")),
        }
    }
}
