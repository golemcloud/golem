use crate::api::common::{ApiEndpointError, ApiTags};
use crate::service::auth::AuthService;
use cloud_common::auth::{CloudAuthCtx, CloudNamespace, GolemSecurityScheme};
use cloud_common::model::ProjectAction;
use futures_util::future::try_join_all;
use golem_common::json_yaml::JsonOrYaml;
use golem_common::model::ProjectId;
use golem_common::{recorded_http_api_request, safe};
use golem_worker_service_base::api::HttpApiDefinitionRequest;
use golem_worker_service_base::api::HttpApiDefinitionResponseData;
use golem_worker_service_base::gateway_api_definition::http::HttpApiDefinitionRequest as CoreHttpApiDefinitionRequest;
use golem_worker_service_base::gateway_api_definition::http::OpenApiHttpApiDefinition;
use golem_worker_service_base::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use golem_worker_service_base::service::gateway::api_definition::ApiDefinitionService;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::result::Result;
use std::sync::Arc;
use tracing::{error, Instrument};

pub struct ApiDefinitionApi {
    definition_service: Arc<dyn ApiDefinitionService<CloudAuthCtx, CloudNamespace> + Send + Sync>,
    auth_service: Arc<dyn AuthService + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/api/definitions", tag = ApiTags::ApiDefinition)]
impl ApiDefinitionApi {
    pub fn new(
        definition_service: Arc<
            dyn ApiDefinitionService<CloudAuthCtx, CloudNamespace> + Send + Sync,
        >,
        auth_service: Arc<dyn AuthService + Sync + Send>,
    ) -> Self {
        Self {
            definition_service,
            auth_service,
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
        openapi: JsonOrYaml<OpenApiHttpApiDefinition>,
        token: GolemSecurityScheme,
    ) -> Result<Json<HttpApiDefinitionResponseData>, ApiEndpointError> {
        let record =
            recorded_http_api_request!("import_open_api", project_id = project_id.0.to_string(),);

        let response = self
            .create_or_update_open_api_internal(project_id.0, openapi.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_or_update_open_api_internal(
        &self,
        project_id: ProjectId,
        openapi: OpenApiHttpApiDefinition,
        token: GolemSecurityScheme,
    ) -> Result<Json<HttpApiDefinitionResponseData>, ApiEndpointError> {
        let auth_ctx = CloudAuthCtx::new(token.secret());
        let namespace = self
            .auth_service
            .authorize_project_action(&project_id, ProjectAction::CreateApiDefinition, &auth_ctx)
            .await?;

        let conversion_context = self
            .definition_service
            .conversion_context(&namespace, &auth_ctx);

        let definition = openapi
            .to_http_api_definition_request(&conversion_context)
            .await
            .map_err(|e| {
                error!("Invalid Spec {}", e);
                ApiEndpointError::bad_request(safe(e))
            })?;

        let result = self
            .definition_service
            .create(&definition, &namespace, &auth_ctx)
            .await?;

        HttpApiDefinitionResponseData::from_compiled_http_api_definition(
            result,
            &conversion_context,
        )
        .await
        .map_err(|err| ApiEndpointError::internal(safe(err)))
        .map(Json)
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
        payload: JsonOrYaml<HttpApiDefinitionRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<HttpApiDefinitionResponseData>, ApiEndpointError> {
        let record = recorded_http_api_request!(
            "create_definition",
            api_definition_id = payload.0.id.to_string(),
            version = payload.0.version.to_string(),
            draft = payload.0.draft.to_string(),
            project_id = project_id.0.to_string()
        );

        let response = self
            .create_internal(project_id.0, payload.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_internal(
        &self,
        project_id: ProjectId,
        payload: HttpApiDefinitionRequest,
        token: GolemSecurityScheme,
    ) -> Result<Json<HttpApiDefinitionResponseData>, ApiEndpointError> {
        let token = token.secret();
        let auth_ctx = CloudAuthCtx::new(token);
        let namespace = self
            .auth_service
            .authorize_project_action(&project_id, ProjectAction::CreateApiDefinition, &auth_ctx)
            .await?;

        let conversion_context = self
            .definition_service
            .conversion_context(&namespace, &auth_ctx);

        let definition: CoreHttpApiDefinitionRequest = payload
            .clone()
            .into_core(&conversion_context)
            .await
            .map_err(|err| ApiEndpointError::bad_request(safe(err)))?;

        let result = self
            .definition_service
            .create(&definition, &namespace, &auth_ctx)
            .await?;

        HttpApiDefinitionResponseData::from_compiled_http_api_definition(
            result,
            &conversion_context,
        )
        .await
        .map_err(|err| ApiEndpointError::internal(safe(err)))
        .map(Json)
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
        payload: JsonOrYaml<HttpApiDefinitionRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<HttpApiDefinitionResponseData>, ApiEndpointError> {
        let record = recorded_http_api_request!(
            "update_definition",
            api_definition_id = id.0.to_string(),
            version = version.0.to_string(),
            draft = payload.0.draft.to_string(),
            project_id = project_id.0.to_string()
        );

        let response = self
            .update_internal(project_id.0, id.0, version.0, payload.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_internal(
        &self,
        project_id: ProjectId,
        id: ApiDefinitionId,
        version: ApiVersion,
        payload: HttpApiDefinitionRequest,
        token: GolemSecurityScheme,
    ) -> Result<Json<HttpApiDefinitionResponseData>, ApiEndpointError> {
        let token = token.secret();
        let auth_ctx = CloudAuthCtx::new(token);
        let namespace = self
            .auth_service
            .authorize_project_action(&project_id, ProjectAction::UpdateApiDefinition, &auth_ctx)
            .await?;

        let conversion_context = self
            .definition_service
            .conversion_context(&namespace, &auth_ctx);

        let definition: CoreHttpApiDefinitionRequest = payload
            .clone()
            .into_core(&conversion_context)
            .await
            .map_err(|err| ApiEndpointError::bad_request(safe(err)))?;

        if id != definition.id {
            Err(ApiEndpointError::bad_request(safe(
                "Unmatched url and body ids.".to_string(),
            )))
        } else if version != definition.version {
            Err(ApiEndpointError::bad_request(safe(
                "Unmatched url and body versions.".to_string(),
            )))
        } else {
            let result = self
                .definition_service
                .update(&definition, &namespace, &auth_ctx)
                .await?;

            HttpApiDefinitionResponseData::from_compiled_http_api_definition(
                result,
                &conversion_context,
            )
            .await
            .map_err(|err| ApiEndpointError::internal(safe(err)))
            .map(Json)
        }
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
    ) -> Result<Json<HttpApiDefinitionResponseData>, ApiEndpointError> {
        let record = recorded_http_api_request!(
            "get_definition",
            api_definition_id = id.0.to_string(),
            version = version.0.to_string(),
            project_id = project_id.0.to_string()
        );

        let response = self
            .get_internal(project_id.0, id.0, version.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_internal(
        &self,
        project_id: ProjectId,
        id: ApiDefinitionId,
        version: ApiVersion,
        token: GolemSecurityScheme,
    ) -> Result<Json<HttpApiDefinitionResponseData>, ApiEndpointError> {
        let token = token.secret();
        let auth_ctx = CloudAuthCtx::new(token);
        let namespace = self
            .auth_service
            .authorize_project_action(&project_id, ProjectAction::ViewApiDefinition, &auth_ctx)
            .await?;

        let conversion_context = self
            .definition_service
            .conversion_context(&namespace, &auth_ctx);

        let data = self
            .definition_service
            .get(&id, &version, &namespace, &auth_ctx)
            .await?;

        let data = data.ok_or(ApiEndpointError::not_found(safe(format!(
            "Can't find api definition with id {id}, and version {version} in project {project_id}"
        ))))?;

        HttpApiDefinitionResponseData::from_compiled_http_api_definition(data, &conversion_context)
            .await
            .map_err(|err| ApiEndpointError::internal(safe(err)))
            .map(Json)
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
    ) -> Result<Json<Vec<HttpApiDefinitionResponseData>>, ApiEndpointError> {
        let record = recorded_http_api_request!(
            "list_definitions",
            api_definition_id = api_definition_id_query.0.as_ref().map(|id| id.to_string()),
            project_id = project_id.0.to_string()
        );

        let response = self
            .list_internal(project_id.0, api_definition_id_query.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_internal(
        &self,
        project_id: ProjectId,
        api_definition_id: Option<ApiDefinitionId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<HttpApiDefinitionResponseData>>, ApiEndpointError> {
        let token = token.secret();
        let auth_ctx = CloudAuthCtx::new(token);
        let namespace = self
            .auth_service
            .authorize_project_action(&project_id, ProjectAction::ViewApiDefinition, &auth_ctx)
            .await?;

        let data = if let Some(api_definition_id) = api_definition_id {
            self.definition_service
                .get_all_versions(&api_definition_id, &namespace, &auth_ctx)
                .await?
        } else {
            self.definition_service
                .get_all(&namespace, &auth_ctx)
                .await?
        };

        let conversion_context = self
            .definition_service
            .conversion_context(&namespace, &auth_ctx);

        let converted = data.into_iter().map(|d| {
            HttpApiDefinitionResponseData::from_compiled_http_api_definition(d, &conversion_context)
        });

        let values = try_join_all(converted).await.map_err(|e| {
            error!("Failed to convert to response data {}", e);
            ApiEndpointError::internal(safe(e))
        })?;

        Ok(Json(values))
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

        let auth_ctx = CloudAuthCtx::new(token);
        let namespace = self
            .auth_service
            .authorize_project_action(&project_id.0, ProjectAction::DeleteApiDefinition, &auth_ctx)
            .await?;

        let response = self
            .definition_service
            .delete(&id.0, &version.0, &namespace, &auth_ctx)
            .instrument(record.span.clone())
            .await
            .map(|_| Json("API definition not found".to_string()))
            .map_err(|err| err.into());

        record.result(response)
    }
}
