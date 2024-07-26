use std::sync::Arc;

use crate::api::common::{ApiEndpointError, ApiTags};
use crate::model::ApiDeployment;
use crate::service::api_domain::RegisterDomainRoute;
use crate::service::auth::{AuthService, CloudAuthCtx, CloudNamespace};
use cloud_common::auth::GolemSecurityScheme;
use golem_common::model::ProjectId;
use golem_common::recorded_http_api_request;
use golem_worker_service_base::api_definition::{ApiDefinitionId, ApiSiteString};
use golem_worker_service_base::service::api_definition::ApiDefinitionIdWithVersion;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::*;

use cloud_common::model::ProjectAction;
use golem_worker_service_base::service::api_deployment::ApiDeploymentService;
use tracing::Instrument;

pub struct ApiDeploymentApi {
    deployment_service: Arc<dyn ApiDeploymentService<CloudNamespace> + Sync + Send>,
    auth_service: Arc<dyn AuthService + Sync + Send>,
    domain_route: Arc<dyn RegisterDomainRoute + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/api/deployments", tag = ApiTags::ApiDeployment)]
impl ApiDeploymentApi {
    pub fn new(
        deployment_service: Arc<dyn ApiDeploymentService<CloudNamespace> + Sync + Send>,
        auth_service: Arc<dyn AuthService + Sync + Send>,
        domain_route: Arc<dyn RegisterDomainRoute + Sync + Send>,
    ) -> Self {
        Self {
            deployment_service,
            auth_service,
            domain_route,
        }
    }

    #[oai(path = "/deploy", method = "post", operation_id = "deploy")]
    async fn create_or_update(
        &self,
        payload: Json<ApiDeployment>,
        token: GolemSecurityScheme,
    ) -> Result<Json<ApiDeployment>, ApiEndpointError> {
        let token = token.secret();
        let project_id = &payload.project_id;
        let record = recorded_http_api_request!(
            "deploy",
            site = payload.0.site.to_string(),
            project_id = payload.0.project_id.to_string()
        );
        let response = {
            let auth_ctx = CloudAuthCtx::new(token);

            let namespace = self
                .auth_service
                .is_authorized(project_id, ProjectAction::ViewApiDefinition, &auth_ctx)
                .instrument(record.span.clone())
                .await?;

            let api_definition_infos = payload
                .api_definitions
                .iter()
                .map(|k| ApiDefinitionIdWithVersion {
                    id: k.id.clone(),
                    version: k.version.clone(),
                })
                .collect::<Vec<ApiDefinitionIdWithVersion>>();

            let payload_deployment = &payload.0;

            let api_deployment = golem_worker_service_base::api_definition::ApiDeployment {
                namespace: namespace.clone(),
                api_definition_keys: api_definition_infos.clone(),
                site: payload_deployment.site.clone(),
            };

            self.deployment_service
                .deploy(&api_deployment)
                .instrument(record.span.clone())
                .await?;

            self.domain_route
                .register(&payload.site.host, payload.site.subdomain.as_deref())
                .instrument(record.span.clone())
                .await?;

            let data = self
                .deployment_service
                .get_by_site(&ApiSiteString(payload.site.to_string()))
                .instrument(record.span.clone())
                .await?;

            let deployment = data
                .map(|d| d.into())
                .ok_or(ApiEndpointError::not_found("API Deployment not found"))?;

            Ok(Json(deployment))
        };

        record.result(response)
    }

    #[oai(path = "/", method = "get", operation_id = "list_deployments")]
    async fn list(
        &self,
        #[oai(name = "project-id")] project_id: Query<ProjectId>,
        #[oai(name = "api-definition-id")] api_definition_id_query: Query<ApiDefinitionId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<ApiDeployment>>, ApiEndpointError> {
        let token = token.secret();
        let project_id = project_id.0;
        let record = recorded_http_api_request!(
            "list_deployments",
            api_definition_id = api_definition_id_query.0.to_string(),
            project_id = project_id.0.to_string()
        );
        let response = {
            let api_definition_id = api_definition_id_query.0;

            let auth_ctx = CloudAuthCtx::new(token);

            let namespace = self
                .auth_service
                .is_authorized(&project_id, ProjectAction::ViewApiDefinition, &auth_ctx)
                .instrument(record.span.clone())
                .await?;

            let api_deployments = self
                .deployment_service
                .get_by_id(&namespace, &api_definition_id)
                .instrument(record.span.clone())
                .await?;

            let values: Vec<ApiDeployment> =
                api_deployments.iter().map(|d| d.clone().into()).collect();

            Ok(Json(values))
        };

        record.result(response)
    }

    #[oai(path = "/:site", method = "get", operation_id = "get_deployment")]
    async fn get(
        &self,
        site: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<ApiDeployment>, ApiEndpointError> {
        let token = token.secret();
        let record = recorded_http_api_request!("get_deployment", site = site.0);
        let response = {
            let site = site.0;
            let auth_ctx = CloudAuthCtx::new(token);

            let api_deployment = self
                .deployment_service
                .get_by_site(&ApiSiteString(site.clone()))
                .instrument(record.span.clone())
                .await?
                .ok_or(ApiEndpointError::not_found("API deployment not found"))?;

            let project_id = &api_deployment.namespace.project_id;

            let _ = self
                .auth_service
                .is_authorized(project_id, ProjectAction::ViewApiDefinition, &auth_ctx)
                .instrument(record.span.clone())
                .await?;

            Ok(Json(api_deployment.into()))
        };

        record.result(response)
    }

    #[oai(path = "/:site", method = "delete", operation_id = "delete_deployment")]
    async fn delete(
        &self,
        site: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<String>, ApiEndpointError> {
        let token = token.secret();
        let record = recorded_http_api_request!("delete_deployment", site = site.0);
        let response = {
            let site = site.0;
            let auth_ctx = CloudAuthCtx::new(token);

            let api_deployment = self
                .deployment_service
                .get_by_site(&ApiSiteString(site.clone()))
                .instrument(record.span.clone())
                .await?
                .ok_or(ApiEndpointError::not_found("API deployment not found"))?;

            let project_id = &api_deployment.namespace.project_id;

            let namespace = self
                .auth_service
                .is_authorized(project_id, ProjectAction::ViewApiDefinition, &auth_ctx)
                .instrument(record.span.clone())
                .await?;

            self.deployment_service
                .delete(&namespace, &ApiSiteString(site))
                .instrument(record.span.clone())
                .await?;

            self.domain_route
                .unregister(
                    &api_deployment.site.host,
                    api_deployment.site.subdomain.as_deref(),
                )
                .instrument(record.span.clone())
                .await?;

            Ok(Json("API deployment deleted".to_string()))
        };

        record.result(response)
    }
}
