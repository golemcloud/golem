use std::sync::Arc;

use crate::api::common::{ApiEndpointError, ApiTags};
use crate::model::ApiDeployment;
use crate::model::ApiDeploymentRequest;
use crate::service::api_domain::RegisterDomainRoute;
use crate::service::auth::AuthService;
use cloud_common::auth::{CloudAuthCtx, CloudNamespace, GolemSecurityScheme};
use cloud_common::model::ProjectAction;
use golem_common::model::ProjectId;
use golem_common::{recorded_http_api_request, safe, SafeDisplay};
use golem_worker_service_base::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use golem_worker_service_base::gateway_api_deployment;
use golem_worker_service_base::gateway_api_deployment::ApiSiteString;
use golem_worker_service_base::service::gateway::api_definition::ApiDefinitionIdWithVersion;
use golem_worker_service_base::service::gateway::api_deployment::{
    ApiDeploymentError, ApiDeploymentService,
};
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::*;
use tracing::Instrument;

pub struct ApiDeploymentApi {
    deployment_service: Arc<dyn ApiDeploymentService<CloudAuthCtx, CloudNamespace> + Sync + Send>,
    auth_service: Arc<dyn AuthService + Sync + Send>,
    domain_route: Arc<dyn RegisterDomainRoute + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/api/deployments", tag = ApiTags::ApiDeployment)]
impl ApiDeploymentApi {
    pub fn new(
        deployment_service: Arc<
            dyn ApiDeploymentService<CloudAuthCtx, CloudNamespace> + Sync + Send,
        >,
        auth_service: Arc<dyn AuthService + Sync + Send>,
        domain_route: Arc<dyn RegisterDomainRoute + Sync + Send>,
    ) -> Self {
        Self {
            deployment_service,
            auth_service,
            domain_route,
        }
    }

    /// Creates or updates a deployment
    ///
    /// Deploys a set of API definitions to a site (specific host and subdomain).
    #[oai(path = "/deploy", method = "post", operation_id = "deploy")]
    async fn create_or_update(
        &self,
        payload: Json<ApiDeploymentRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<ApiDeployment>, ApiEndpointError> {
        let record = recorded_http_api_request!(
            "deploy",
            site = payload.0.site.to_string(),
            project_id = payload.0.project_id.to_string()
        );
        let response = self
            .create_or_update_internal(payload.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_or_update_internal(
        &self,
        payload: ApiDeploymentRequest,
        token: GolemSecurityScheme,
    ) -> Result<Json<ApiDeployment>, ApiEndpointError> {
        let token = token.secret();
        let auth_ctx = CloudAuthCtx::new(token);

        let namespace = self
            .auth_service
            .authorize_project_action(
                &payload.project_id,
                ProjectAction::UpsertApiDeployment,
                &auth_ctx,
            )
            .await?;

        let api_definition_infos = payload
            .api_definitions
            .iter()
            .map(|k| ApiDefinitionIdWithVersion {
                id: k.id.clone(),
                version: k.version.clone(),
            })
            .collect::<Vec<ApiDefinitionIdWithVersion>>();

        let api_deployment = gateway_api_deployment::ApiDeploymentRequest {
            namespace: namespace.clone(),
            api_definition_keys: api_definition_infos.clone(),
            site: payload.site.clone(),
        };

        self.deployment_service
            .deploy(&api_deployment, &auth_ctx)
            .await?;

        self.domain_route
            .register(&payload.site.host, payload.site.subdomain.as_deref())
            .await?;

        let data = self
            .deployment_service
            .get_by_site(&namespace, &ApiSiteString(payload.site.to_string()))
            .await?;

        let deployment = data
            .map(|d| d.into())
            .ok_or(ApiEndpointError::not_found(safe(
                "API Deployment not found".to_string(),
            )))?;

        Ok(Json(deployment))
    }

    /// Get one or more API deployments
    ///
    /// If `api-definition-id` is not set, it lists all API deployments.
    /// If `api-definition-id` is set, returns a single API deployment.
    #[oai(path = "/", method = "get", operation_id = "list_deployments")]
    async fn list(
        &self,
        #[oai(name = "project-id")] project_id: Query<ProjectId>,
        #[oai(name = "api-definition-id")] api_definition_id_query: Query<ApiDefinitionId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<ApiDeployment>>, ApiEndpointError> {
        let project_id = project_id.0;
        let record = recorded_http_api_request!(
            "list_deployments",
            api_definition_id = api_definition_id_query.0.to_string(),
            project_id = project_id.0.to_string()
        );
        let response = self
            .list_internal(project_id, api_definition_id_query.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_internal(
        &self,
        project_id: ProjectId,
        api_definition_id: ApiDefinitionId,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<ApiDeployment>>, ApiEndpointError> {
        let token = token.secret();
        let auth_ctx = CloudAuthCtx::new(token);

        let namespace = self
            .auth_service
            .authorize_project_action(&project_id, ProjectAction::ViewApiDeployment, &auth_ctx)
            .await?;

        let api_deployments = self
            .deployment_service
            .get_by_id(&namespace, Some(api_definition_id))
            .await?;

        let values: Vec<ApiDeployment> = api_deployments.into_iter().map(|d| d.into()).collect();
        Ok(Json(values))
    }

    /// Get API deployment by site
    ///
    /// Gets an API deployment by the host name (optionally with a subdomain) it is deployed to.
    #[oai(path = "/:site", method = "get", operation_id = "get_deployment")]
    async fn get(
        &self,
        #[oai(name = "project-id")] project_id: Query<ProjectId>,
        site: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<ApiDeployment>, ApiEndpointError> {
        let record = recorded_http_api_request!("get_deployment", site = site.0);
        let response = self
            .get_internal(&project_id.0, site.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_internal(
        &self,
        project_id: &ProjectId,
        site: String,
        token: GolemSecurityScheme,
    ) -> Result<Json<ApiDeployment>, ApiEndpointError> {
        let token = token.secret();
        let site = ApiSiteString(site);
        let auth_ctx = CloudAuthCtx::new(token);

        let namespace = self
            .auth_service
            .authorize_project_action(project_id, ProjectAction::ViewApiDeployment, &auth_ctx)
            .await?;

        let api_deployment = self
            .deployment_service
            .get_by_site(&namespace, &site)
            .await?
            .ok_or(ApiEndpointError::not_found(safe(
                "API deployment not found".to_string(),
            )))?;

        Ok(Json(api_deployment.into()))
    }

    /// Delete API deployment by site
    ///
    /// Deletes an API deployment by the host name (optionally with a subdomain) it is deployed to.
    #[oai(path = "/:site", method = "delete", operation_id = "delete_deployment")]
    async fn delete(
        &self,
        #[oai(name = "project-id")] project_id: Query<ProjectId>,
        site: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<String>, ApiEndpointError> {
        let record = recorded_http_api_request!("delete_deployment", site = site.0);

        let auth_ctx = CloudAuthCtx::new(token.secret());

        let namespace = self
            .auth_service
            .authorize_project_action(&project_id.0, ProjectAction::DeleteApiDeployment, &auth_ctx)
            .await?;

        let response = self
            .delete_internal(&namespace, &auth_ctx, site.0)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_internal(
        &self,
        namespace: &CloudNamespace,
        auth_ctx: &CloudAuthCtx,
        site: String,
    ) -> Result<Json<String>, ApiEndpointError> {
        let site = ApiSiteString(site);

        let api_deployment = self
            .deployment_service
            .get_by_site(namespace, &site)
            .await?
            .ok_or(ApiEndpointError::not_found(safe(
                "API deployment not found".to_string(),
            )))?;

        self.deployment_service
            .delete(namespace, auth_ctx, &site)
            .await?;

        self.domain_route
            .unregister(
                &api_deployment.site.host,
                api_deployment.site.subdomain.as_deref(),
            )
            .await?;

        Ok(Json("API deployment deleted".to_string()))
    }

    /// Undeploy a single API definition from a site
    ///
    /// Removes a specific API definition (by id and version) from a site without deleting the entire deployment.
    #[oai(
        path = "/:site/:id/:version",
        method = "delete",
        operation_id = "undeploy_api"
    )]
    async fn undeploy_api(
        &self,
        #[oai(name = "project-id")] project_id: Query<ProjectId>,
        site: Path<String>,
        id: Path<String>,
        version: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<String>, ApiEndpointError> {
        let record = recorded_http_api_request!(
            "undeploy_api",
            site = site.0.clone(),
            id = id.0.clone(),
            version = version.0.clone()
        );

        let auth_ctx = CloudAuthCtx::new(token.secret());

        // TODO: should not use ProjectAction::ViewApiDefinition, rather a deployment action,
        //       for now kept in sync with delete
        let namespace = self
            .auth_service
            .authorize_project_action(&project_id.0, ProjectAction::DeleteApiDeployment, &auth_ctx)
            .await?;

        let response = self
            .undeploy_api_internal(&namespace, &auth_ctx, site.0, id.0, version.0)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn undeploy_api_internal(
        &self,
        namespace: &CloudNamespace,
        auth_ctx: &CloudAuthCtx,
        site: String,
        id: String,
        version: String,
    ) -> Result<Json<String>, ApiEndpointError> {
        let api_definition_key = ApiDefinitionIdWithVersion {
            id: ApiDefinitionId(id),
            version: ApiVersion(version),
        };

        let api_site_string = ApiSiteString(site);

        self.deployment_service
            .undeploy(namespace, api_site_string, api_definition_key, auth_ctx)
            .await
            .map_err(|err| match err {
                ApiDeploymentError::ApiDeploymentNotFound(_, _) => {
                    ApiEndpointError::not_found(safe("Site not found".to_string()))
                }
                ApiDeploymentError::ApiDefinitionNotFound(_, _, _) => {
                    ApiEndpointError::not_found(safe("API definition not found".to_string()))
                }
                _ => ApiEndpointError::internal(safe(err.to_safe_string())),
            })?;

        Ok(Json("API definition undeployed from site".to_string()))
    }
}
