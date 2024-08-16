use std::result::Result;
use std::sync::Arc;

use crate::api::common::{ApiEndpointError, ApiTags};
use crate::service::api_definition::ApiDefinitionService;
use crate::service::api_domain::RegisterDomainRoute;
use crate::service::auth::{CloudAuthCtx, CloudNamespace};
use cloud_common::auth::GolemSecurityScheme;
use golem_common::model::ProjectId;
use golem_common::recorded_http_api_request;
use golem_worker_service_base::api::HttpApiDefinition;
use golem_worker_service_base::api_definition::http::{
    get_api_definition, HttpApiDefinition as CoreHttpApiDefinition, JsonOpenApiDefinition,
};
use golem_worker_service_base::api_definition::{ApiDefinitionId, ApiSiteString, ApiVersion};
use golem_worker_service_base::service::api_deployment::ApiDeploymentService;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::*;
use tracing::{error, Instrument};

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

    /// Upload an OpenAPI definition
    ///
    /// Uploads an OpenAPI JSON document and either creates a new one or updates an existing Golem
    /// API definition using it.
    #[oai(
        path = "/:project_id/import",
        method = "put",
        operation_id = "import_open_api"
    )]
    async fn create_or_update_open_api(
        &self,
        project_id: Path<ProjectId>,
        Json(openapi): Json<JsonOpenApiDefinition>,
        token: GolemSecurityScheme,
    ) -> Result<Json<HttpApiDefinition>, ApiEndpointError> {
        let project_id = &project_id.0;
        let token = token.secret();

        let record = recorded_http_api_request!("import_open_api",);

        let response = {
            let definition = get_api_definition(openapi.0).map_err(|e| {
                error!("Invalid Spec {}", e);
                ApiEndpointError::bad_request(e)
            })?;

            self.definition_service
                .create(project_id, &definition, &CloudAuthCtx::new(token))
                .instrument(record.span.clone())
                .await?;

            definition
                .try_into()
                .map_err(ApiEndpointError::internal)
                .map(Json)
        };

        record.result(response)
    }

    /// Create a new API definition
    ///
    /// Creates a new API definition described by Golem's API definition JSON document.
    /// If an API definition of the same version already exists, its an error.
    #[oai(
        path = "/:project_id",
        method = "post",
        operation_id = "create_definition"
    )]
    async fn create(
        &self,
        project_id: Path<ProjectId>,
        payload: Json<HttpApiDefinition>,
        token: GolemSecurityScheme,
    ) -> Result<Json<HttpApiDefinition>, ApiEndpointError> {
        let project_id = &project_id.0;
        let token = token.secret();
        let record = recorded_http_api_request!(
            "create_definition",
            api_definition_id = payload.0.id.to_string(),
            version = payload.0.version.to_string(),
            draft = payload.0.draft.to_string(),
            project_id = project_id.0.to_string()
        );

        let response = {
            let definition: CoreHttpApiDefinition = payload
                .0
                .clone()
                .try_into()
                .map_err(ApiEndpointError::bad_request)?;

            let _ = self
                .definition_service
                .create(project_id, &definition, &CloudAuthCtx::new(token))
                .instrument(record.span.clone())
                .await?;
            Ok(Json(payload.0))
        };

        record.result(response)
    }

    /// Update an existing API definition.
    ///
    /// Only draft API definitions can be updated.
    #[oai(
        path = "/:project_id/:id/:version",
        method = "put",
        operation_id = "update_definition"
    )]
    async fn update(
        &self,
        project_id: Path<ProjectId>,
        id: Path<ApiDefinitionId>,
        version: Path<ApiVersion>,
        payload: Json<HttpApiDefinition>,
        token: GolemSecurityScheme,
    ) -> Result<Json<HttpApiDefinition>, ApiEndpointError> {
        let project_id = &project_id.0;
        let token = token.secret();
        let record = recorded_http_api_request!(
            "update_definition",
            api_definition_id = id.0.to_string(),
            version = version.0.to_string(),
            draft = payload.0.draft.to_string(),
            project_id = project_id.0.to_string()
        );

        let response = {
            let definition: CoreHttpApiDefinition = payload
                .0
                .clone()
                .try_into()
                .map_err(ApiEndpointError::bad_request)?;

            if id.0 != definition.id {
                Err(ApiEndpointError::bad_request("Unmatched url and body ids."))
            } else if version.0 != definition.version {
                Err(ApiEndpointError::bad_request(
                    "Unmatched url and body versions.",
                ))
            } else {
                let _ = self
                    .definition_service
                    .update(project_id, &definition, &CloudAuthCtx::new(token))
                    .instrument(record.span.clone())
                    .await?;

                Ok(Json(payload.0))
            }
        };

        record.result(response)
    }

    /// Get an API definition
    ///
    /// An API definition is selected by its API definition ID and version.
    #[oai(
        path = "/:project_id/:id/:version",
        method = "get",
        operation_id = "get_definition"
    )]
    async fn get(
        &self,
        project_id: Path<ProjectId>,
        id: Path<ApiDefinitionId>,
        version: Path<ApiVersion>,
        token: GolemSecurityScheme,
    ) -> Result<Json<HttpApiDefinition>, ApiEndpointError> {
        let token = token.secret();
        let record = recorded_http_api_request!(
            "get_definition",
            api_definition_id = id.0.to_string(),
            version = version.0.to_string(),
            project_id = project_id.0.to_string()
        );

        let response = {
            let project_id = project_id.0;
            let api_definition_id = id.0;
            let version = version.0;

            let auth_ctx = CloudAuthCtx::new(token);

            let (data, _) = self
                .definition_service
                .get(&project_id, &api_definition_id, &version, &auth_ctx)
                .instrument(record.span.clone())
                .await?;

            let data = data.ok_or(ApiEndpointError::not_found(
                format!("Can't find api definition with id {api_definition_id}, and version {version} in project {project_id}")
            ))?;

            Ok(Json(data.try_into().map_err(ApiEndpointError::internal)?))
        };

        record.result(response)
    }

    /// List API definitions
    ///
    /// Lists all API definitions associated with the project.
    #[oai(
        path = "/:project_id",
        method = "get",
        operation_id = "list_definitions"
    )]
    async fn list(
        &self,
        project_id: Path<ProjectId>,
        #[oai(name = "api-definition-id")] api_definition_id_query: Query<Option<ApiDefinitionId>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<HttpApiDefinition>>, ApiEndpointError> {
        let token = token.secret();
        let record = recorded_http_api_request!(
            "list_definitions",
            api_definition_id = api_definition_id_query.0.as_ref().map(|id| id.to_string()),
            project_id = project_id.0.to_string()
        );

        let response = {
            let project_id = project_id.0;
            let api_definition_id_optional = api_definition_id_query.0;

            let auth_ctx = CloudAuthCtx::new(token);

            let (data, _) = if let Some(api_definition_id) = api_definition_id_optional {
                self.definition_service
                    .get_all_versions(&project_id, &api_definition_id, &auth_ctx)
                    .instrument(record.span.clone())
                    .await?
            } else {
                self.definition_service
                    .get_all(&project_id, &auth_ctx)
                    .instrument(record.span.clone())
                    .await?
            };
            let values = data
                .into_iter()
                .map(|d| d.try_into().map_err(ApiEndpointError::internal))
                .collect::<Result<Vec<HttpApiDefinition>, ApiEndpointError>>()?;
            Ok(Json(values))
        };
        record.result(response)
    }

    /// Delete an API definition
    ///
    /// Deletes an API definition by its API definition ID and version.
    #[oai(
        path = "/:project_id/:id/:version",
        method = "delete",
        operation_id = "delete_definition"
    )]
    async fn delete(
        &self,
        project_id: Path<ProjectId>,
        id: Path<ApiDefinitionId>,
        version: Path<ApiVersion>,
        token: GolemSecurityScheme,
    ) -> Result<Json<String>, ApiEndpointError> {
        let token = token.secret();
        let record = recorded_http_api_request!(
            "delete_definition",
            api_definition_id = id.0.to_string(),
            version = version.0.to_string(),
            project_id = project_id.0.to_string()
        );

        let response = {
            let project_id = project_id.0;
            let api_definition_id = id.0;
            let version = version.0;

            let auth_ctx = CloudAuthCtx::new(token);

            let (value, namespace) = self
                .definition_service
                .get(&project_id, &api_definition_id, &version, &auth_ctx)
                .instrument(record.span.clone())
                .await?;

            match value {
                Some(_) => {
                    let deployments = self
                        .deployment_service
                        .get_by_id(&namespace, &api_definition_id)
                        .instrument(record.span.clone())
                        .await?;

                    for deployment in deployments {
                        self.domain_route
                            .unregister(&deployment.site.host, deployment.site.subdomain.as_deref())
                            .instrument(record.span.clone())
                            .await
                            .map_err(ApiEndpointError::from)?;

                        self.deployment_service
                            .delete(&namespace, &ApiSiteString(deployment.site.to_string()))
                            .instrument(record.span.clone())
                            .await?;
                    }

                    self.definition_service
                        .delete(&project_id, &api_definition_id, &version, &auth_ctx)
                        .instrument(record.span.clone())
                        .await?;

                    Ok(Json("API definition deleted".to_string()))
                }
                None => Err(ApiEndpointError::not_found("API definition not found")),
            }
        };
        record.result(response)
    }
}
