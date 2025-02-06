use golem_common::json_yaml::JsonOrYaml;
use golem_common::{recorded_http_api_request, safe};
use golem_service_base::api_tags::ApiTags;
use golem_service_base::auth::{DefaultNamespace, EmptyAuthCtx};
use golem_worker_service_base::api::ApiEndpointError;
use golem_worker_service_base::api::HttpApiDefinitionRequest;
use golem_worker_service_base::api::HttpApiDefinitionResponseData;
use golem_worker_service_base::gateway_api_definition::http::CompiledHttpApiDefinition;
use golem_worker_service_base::gateway_api_definition::http::HttpApiDefinitionRequest as CoreHttpApiDefinitionRequest;
use golem_worker_service_base::gateway_api_definition::http::OpenApiHttpApiDefinitionRequest;
use golem_worker_service_base::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use golem_worker_service_base::service::gateway::api_definition::ApiDefinitionService;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::result::Result;
use std::sync::Arc;
use tracing::{error, Instrument};
use golem_worker_service_base::gateway_api_definition::http::HttpApiDefinition;
use golem_worker_service_base::api::RouteResponseData;
use golem_worker_service_base::gateway_api_definition::HasGolemBindings;
use golem_worker_service_base::service::component::ComponentService;
use async_trait::async_trait;
use futures::future::try_join_all;

/// Local trait for converting HttpApiDefinition to HttpApiDefinitionResponseData
#[async_trait]
pub trait ToHttpApiDefinitionResponseData {
    async fn to_response_data(&self, component_service: &Arc<dyn ComponentService<EmptyAuthCtx> + Send + Sync>) -> Result<HttpApiDefinitionResponseData, String>;
}

#[async_trait]
impl ToHttpApiDefinitionResponseData for HttpApiDefinition {
    async fn to_response_data(&self, component_service: &Arc<dyn ComponentService<EmptyAuthCtx> + Send + Sync>) -> Result<HttpApiDefinitionResponseData, String> {
        let components = self.get_bindings()
            .iter()
            .filter_map(|binding| binding.get_component_id())
            .map(|versioned_id| async move {
                let component = component_service
                    .get_by_version(&versioned_id.component_id, versioned_id.version, &EmptyAuthCtx::default())
                    .await
                    .map_err(|e| format!("Failed to get component metadata: {:?}", e))?;

                Ok(golem_service_base::model::Component {
                    versioned_component_id: golem_service_base::model::VersionedComponentId {
                        component_id: versioned_id.component_id.clone(),
                        version: versioned_id.version,
                    },
                    component_name: component.component_name,
                    component_size: component.component_size,
                    metadata: component.metadata,
                    created_at: component.created_at,
                    component_type: component.component_type,
                    files: component.files,
                    installed_plugins: component.installed_plugins,
                })
            })
            .collect::<Vec<_>>();

        let components = try_join_all(components)
            .await
            .map_err(|e: Box<dyn std::error::Error + Send + Sync>| format!("Failed to get components: {}", e))?;

        let metadata_dictionary = golem_worker_service_base::gateway_api_definition::http::ComponentMetadataDictionary::from_components(&components);

        let routes = self.routes
            .iter()
            .filter(|route| !route.binding.is_security_binding())  // Filter out security bindings
            .map(|route| {
                let compiled_route = golem_worker_service_base::gateway_api_definition::http::CompiledRoute::from_route(
                    route,
                    &metadata_dictionary,
                ).map_err(|e| format!("Failed to compile route: {:?}", e))?;
                RouteResponseData::try_from(compiled_route)
            })
            .collect::<Result<Vec<_>, String>>()?;

        Ok(HttpApiDefinitionResponseData {
            id: self.id.clone(),
            version: self.version.clone(),
            routes,
            draft: self.draft,
            created_at: Some(self.created_at),
        })
    }
}

#[async_trait]
impl<N: Clone + Send + Sync> ToHttpApiDefinitionResponseData for CompiledHttpApiDefinition<N> {
    async fn to_response_data(&self, component_service: &Arc<dyn ComponentService<EmptyAuthCtx> + Send + Sync>) -> Result<HttpApiDefinitionResponseData, String> {
        let http_api_def: HttpApiDefinition = (*self).clone().into();
        http_api_def.to_response_data(component_service).await
    }
}

pub struct RegisterApiDefinitionApi {
    definition_service: Arc<dyn ApiDefinitionService<EmptyAuthCtx, DefaultNamespace> + Sync + Send>,
    component_service: Arc<dyn ComponentService<EmptyAuthCtx> + Send + Sync>,
}

#[OpenApi(prefix_path = "/v1/api/definitions", tag = ApiTags::ApiDefinition)]
impl RegisterApiDefinitionApi {
    pub fn new(
        definition_service: Arc<dyn ApiDefinitionService<EmptyAuthCtx, DefaultNamespace> + Sync + Send>,
        component_service: Arc<dyn ComponentService<EmptyAuthCtx> + Send + Sync>,
    ) -> Self {
        Self { definition_service, component_service }
    }

    /// Upload an OpenAPI definition
    ///
    /// Uploads an OpenAPI JSON document and either creates a new one or updates an existing Golem
    /// API definition using it.
    #[oai(path = "/import", method = "put", operation_id = "import_open_api")]
    async fn create_or_update_open_api(
        &self,
        payload: JsonOrYaml<OpenApiHttpApiDefinitionRequest>,
    ) -> Result<Json<HttpApiDefinitionResponseData>, ApiEndpointError> {
        let record = recorded_http_api_request!("import_open_api",);

        let response = {
            let definition = payload.0.to_http_api_definition_request().map_err(|e| {
                error!("Invalid Spec {}", e);
                ApiEndpointError::bad_request(safe(e))
            })?;

            let result = self
                .create_api(&definition)
                .instrument(record.span.clone())
                .await?;

            let result = result.to_response_data(&self.component_service).await.map_err(|e| {
                error!("Failed to convert to response data {}", e);
                ApiEndpointError::internal(safe(e))
            })?;

            Ok(Json(result))
        };

        record.result(response)
    }

    /// Create a new API definition
    ///
    /// Creates a new API definition described by Golem's API definition JSON document.
    /// If an API definition of the same version already exists, its an error.
    #[oai(path = "/", method = "post", operation_id = "create_definition")]
    async fn create(
        &self,
        payload: JsonOrYaml<HttpApiDefinitionRequest>,
    ) -> Result<Json<HttpApiDefinitionResponseData>, ApiEndpointError> {
        let record = recorded_http_api_request!(
            "create_definition",
            api_definition_id = payload.0.id.to_string(),
            version = payload.0.version.to_string(),
            draft = payload.0.draft.to_string()
        );

        let response = {
            let definition: CoreHttpApiDefinitionRequest = payload
                .0
                .try_into()
                .map_err(|err| ApiEndpointError::bad_request(safe(err)))?;

            let compiled_definition = self
                .create_api(&definition)
                .instrument(record.span.clone())
                .await?;

            let result = compiled_definition.to_response_data(&self.component_service).await.map_err(|e| {
                error!("Failed to convert to response data {}", e);
                ApiEndpointError::internal(safe(e))
            })?;

            Ok(Json(result))
        };

        record.result(response)
    }

    /// Update an existing API definition.
    ///
    /// Only draft API definitions can be updated.
    #[oai(
        path = "/:id/:version",
        method = "put",
        operation_id = "update_definition"
    )]
    async fn update(
        &self,
        id: Path<ApiDefinitionId>,
        version: Path<ApiVersion>,
        payload: JsonOrYaml<HttpApiDefinitionRequest>,
    ) -> Result<Json<HttpApiDefinitionResponseData>, ApiEndpointError> {
        let record = recorded_http_api_request!(
            "update_definition",
            api_definition_id = id.0.to_string(),
            version = version.0.to_string(),
            draft = payload.0.draft.to_string()
        );

        let response = {
            let definition: CoreHttpApiDefinitionRequest = payload
                .0
                .try_into()
                .map_err(|err| ApiEndpointError::bad_request(safe(err)))?;

            if id.0 != definition.id {
                Err(ApiEndpointError::bad_request(safe(
                    "Unmatched url and body ids.".to_string(),
                )))
            } else if version.0 != definition.version {
                Err(ApiEndpointError::bad_request(safe(
                    "Unmatched url and body versions.".to_string(),
                )))
            } else {
                let compiled_definition = self
                    .definition_service
                    .update(
                        &definition,
                        &DefaultNamespace::default(),
                        &EmptyAuthCtx::default(),
                    )
                    .instrument(record.span.clone())
                    .await?;

                let result = compiled_definition.to_response_data(&self.component_service).await.map_err(|e| {
                    error!("Failed to convert to response data {}", e);
                    ApiEndpointError::internal(safe(e))
                })?;

                Ok(Json(result))
            }
        };

        record.result(response)
    }

    /// Get an API definition
    ///
    /// An API definition is selected by its API definition ID and version.
    #[oai(
        path = "/:id/:version",
        method = "get",
        operation_id = "get_definition"
    )]
    async fn get(
        &self,
        id: Path<ApiDefinitionId>,
        version: Path<ApiVersion>,
    ) -> Result<Json<HttpApiDefinitionResponseData>, ApiEndpointError> {
        let record = recorded_http_api_request!(
            "get_definition",
            api_definition_id = id.0.to_string(),
            version = version.0.to_string()
        );

        let response = {
            let api_definition_id = id.0;

            let api_version = version.0;

            let data = self
                .definition_service
                .get(
                    &api_definition_id,
                    &api_version,
                    &DefaultNamespace::default(),
                    &EmptyAuthCtx::default(),
                )
                .instrument(record.span.clone())
                .await?;

            let compiled_definition = data.ok_or(ApiEndpointError::not_found(safe(format!(
                "Can't find api definition with id {api_definition_id}, and version {api_version}"
            ))))?;

            let result = compiled_definition.to_response_data(&self.component_service).await.map_err(|e| {
                error!("Failed to convert to response data {}", e);
                ApiEndpointError::internal(safe(e))
            })?;

            Ok(Json(result))
        };

        record.result(response)
    }

    /// Delete an API definition
    ///
    /// Deletes an API definition by its API definition ID and version.
    #[oai(
        path = "/:id/:version",
        method = "delete",
        operation_id = "delete_definition"
    )]
    async fn delete(
        &self,
        id: Path<ApiDefinitionId>,
        version: Path<ApiVersion>,
    ) -> Result<Json<String>, ApiEndpointError> {
        let record = recorded_http_api_request!(
            "delete_definition",
            api_definition_id = id.0.to_string(),
            version = version.0.to_string()
        );

        let response = {
            let api_definition_id = id.0;
            let api_definition_version = version.0;

            self.definition_service
                .delete(
                    &api_definition_id,
                    &api_definition_version,
                    &DefaultNamespace::default(),
                    &EmptyAuthCtx::default(),
                )
                .instrument(record.span.clone())
                .await?;

            Ok(Json("API definition deleted".to_string()))
        };
        record.result(response)
    }

    /// Get or list API definitions
    ///
    /// If `api_definition_id` is specified, returns a single API definition.
    /// Otherwise lists all API definitions.
    #[oai(path = "/", method = "get", operation_id = "list_definitions")]
    async fn list(
        &self,
        #[oai(name = "api-definition-id")] api_definition_id_query: Query<Option<ApiDefinitionId>>,
    ) -> Result<Json<Vec<HttpApiDefinitionResponseData>>, ApiEndpointError> {
        let record = recorded_http_api_request!(
            "list_definitions",
            api_definition_id = api_definition_id_query.0.as_ref().map(|id| id.to_string()),
        );

        let response = {
            let data = if let Some(id) = api_definition_id_query.0 {
                self.definition_service
                    .get_all_versions(&id, &DefaultNamespace::default(), &EmptyAuthCtx::default())
                    .instrument(record.span.clone())
                    .await?
            } else {
                self.definition_service
                    .get_all(&DefaultNamespace::default(), &EmptyAuthCtx::default())
                    .instrument(record.span.clone())
                    .await?
            };

            let values = futures::future::try_join_all(
                data.into_iter()
                    .map(|http_api_def| {
                        let component_service = self.component_service.clone();
                        async move {
                            http_api_def.to_response_data(&component_service).await
                        }
                    })
            )
            .await
            .map_err(|e| {
                error!("Failed to convert to response data {}", e);
                ApiEndpointError::internal(safe(e))
            })?;

            Ok(Json(values))
        };
        record.result(response)
    }

    /// Export the OpenAPI specification for the API definition.
    ///
    /// Returns the OpenAPI spec (in YAML) for the API definition with the given id and version.
    #[oai(path = "/:id/:version/export", method = "get", operation_id = "export_definition")]
    async fn export(
        &self,
        id: poem_openapi::param::Path<golem_worker_service_base::gateway_api_definition::ApiDefinitionId>,
        version: poem_openapi::param::Path<golem_worker_service_base::gateway_api_definition::ApiVersion>,
    ) -> Result<poem_openapi::payload::PlainText<String>, ApiEndpointError> {
        let record = recorded_http_api_request!(
            "export_definition",
            api_definition_id = id.0.to_string(),
            version = version.0.to_string()
        );

        let data = self.definition_service
            .get(&id.0, &version.0, &DefaultNamespace::default(), &EmptyAuthCtx::default())
            .instrument(record.span.clone())
            .await?;
        
        let compiled_definition = data.ok_or(
            ApiEndpointError::not_found(safe(format!(
                "Can't find API definition with id {} and version {}",
                id.0, version.0
            )))
        )?;
        
        // First convert CompiledHttpApiDefinition to HttpApiDefinition
        let http_api_def: HttpApiDefinition = compiled_definition.into();
        
        // Then convert HttpApiDefinition to base HttpApiDefinitionRequest
        let api_def_request: golem_worker_service_base::gateway_api_definition::http::HttpApiDefinitionRequest = http_api_def.into();
        
        // Finally create OpenApiHttpApiDefinitionRequest
        let openapi_def = golem_worker_service_base::gateway_api_definition::http::OpenApiHttpApiDefinitionRequest::from_http_api_definition_request(&api_def_request)
            .map_err(|e| ApiEndpointError::internal(safe(e)))?;
        
        let yaml = serde_yaml::to_string(&openapi_def.0)
            .map_err(|e| ApiEndpointError::internal(safe(e.to_string())))?;
        
        Ok(poem_openapi::payload::PlainText(yaml))
    }
}

impl RegisterApiDefinitionApi {
    async fn create_api(
        &self,
        definition: &CoreHttpApiDefinitionRequest,
    ) -> Result<CompiledHttpApiDefinition<DefaultNamespace>, ApiEndpointError> {
        let result = self
            .definition_service
            .create(
                definition,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await
            .map_err(|e| {
                error!(
                    "API definition ID: {} - register error: {e:?}",
                    definition.id
                );
                e
            })?;

        Ok(result)
    }
}

#[cfg(test)]
mod test {
    use golem_service_base::migration::{Migrations, MigrationsDir};
    use test_r::test;

    use super::*;
    use crate::service::component::ComponentService;
    use async_trait::async_trait;
    use golem_common::config::DbSqliteConfig;
    use golem_common::model::component_constraint::FunctionConstraintCollection;
    use golem_common::model::ComponentId;
    use golem_service_base::db;
    use golem_service_base::model::Component;
    use golem_worker_service_base::gateway_security::DefaultIdentityProvider;
    use golem_worker_service_base::repo::api_definition::{
        ApiDefinitionRepo, DbApiDefinitionRepo, LoggedApiDefinitionRepo,
    };
    use golem_worker_service_base::repo::api_deployment;
    use golem_worker_service_base::repo::security_scheme::{
        DbSecuritySchemeRepo, LoggedSecuritySchemeRepo, SecuritySchemeRepo,
    };
    use golem_worker_service_base::service::component::ComponentResult;
    use golem_worker_service_base::service::gateway::api_definition::ApiDefinitionServiceDefault;
    use golem_worker_service_base::service::gateway::http_api_definition_validator::HttpApiDefinitionValidator;
    use golem_worker_service_base::service::gateway::security_scheme::DefaultSecuritySchemeService;
    use golem_worker_service_base::gateway_api_definition::http::MethodPattern;
    use golem_worker_service_base::api::{RouteRequestData, GatewayBindingData};
    use golem_common::model::GatewayBindingType;
    use http::StatusCode;
    use poem::test::TestClient;
    use std::marker::PhantomData;

    struct SqliteDb<'c> {
        db_path: String,
        lifetime: PhantomData<&'c ()>,
    }

    impl Default for SqliteDb<'_> {
        fn default() -> Self {
            Self {
                db_path: format!("/tmp/golem-worker-{}.db", uuid::Uuid::new_v4()),
                lifetime: PhantomData,
            }
        }
    }

    impl Drop for SqliteDb<'_> {
        fn drop(&mut self) {
            std::fs::remove_file(&self.db_path).unwrap();
        }
    }

    struct TestComponentService;

    #[async_trait]
    impl golem_worker_service_base::service::component::ComponentService<EmptyAuthCtx>
        for TestComponentService
    {
        async fn get_by_version(
            &self,
            _component_id: &ComponentId,
            _version: u64,
            _auth_ctx: &EmptyAuthCtx,
        ) -> ComponentResult<Component> {
            unimplemented!()
        }

        async fn get_latest(
            &self,
            _component_id: &ComponentId,
            _auth_ctx: &EmptyAuthCtx,
        ) -> ComponentResult<Component> {
            unimplemented!()
        }

        async fn create_or_update_constraints(
            &self,
            _component_id: &ComponentId,
            _constraints: FunctionConstraintCollection,
            _auth_ctx: &EmptyAuthCtx,
        ) -> ComponentResult<FunctionConstraintCollection> {
            unimplemented!()
        }
    }

    async fn make_route<'c>() -> (poem::Route, SqliteDb<'c>) {
        let db = SqliteDb::default();
        let db_config = DbSqliteConfig {
            database: db.db_path.to_string(),
            max_connections: 10,
        };

        db::sqlite_migrate(
            &db_config,
            MigrationsDir::new("./db/migration".into()).sqlite_migrations(),
        )
        .await
        .unwrap();

        let db_pool = db::create_sqlite_pool(&db_config).await.unwrap();

        let api_definition_repo: Arc<dyn ApiDefinitionRepo + Sync + Send> = Arc::new(
            LoggedApiDefinitionRepo::new(DbApiDefinitionRepo::new(db_pool.clone().into())),
        );
        let api_deployment_repo: Arc<dyn api_deployment::ApiDeploymentRepo + Sync + Send> =
            Arc::new(api_deployment::LoggedDeploymentRepo::new(
                api_deployment::DbApiDeploymentRepo::new(db_pool.clone().into()),
            ));

        let security_scheme_repo: Arc<dyn SecuritySchemeRepo + Sync + Send> = Arc::new(
            LoggedSecuritySchemeRepo::new(DbSecuritySchemeRepo::new(db_pool.clone().into())),
        );

        let identity_provider = Arc::new(DefaultIdentityProvider);

        let security_scheme_service = Arc::new(DefaultSecuritySchemeService::new(
            security_scheme_repo,
            identity_provider,
        ));

        let component_service: ComponentService = Arc::new(TestComponentService);
        let definition_service = ApiDefinitionServiceDefault::new(
            component_service.clone(),
            api_definition_repo,
            api_deployment_repo,
            security_scheme_service,
            Arc::new(HttpApiDefinitionValidator {}),
        );

        let endpoint = RegisterApiDefinitionApi::new(Arc::new(definition_service), component_service);

        (
            poem::Route::new().nest("", OpenApiService::new(endpoint, "test", "1.0")),
            db,
        )
    }

    #[test]
    async fn conflict_error_returned() {
        let (api, _db) = make_route().await;
        let client = TestClient::new(api);

        let definition = golem_worker_service_base::api::HttpApiDefinitionRequest {
            id: ApiDefinitionId("test".to_string()),
            version: ApiVersion("1.0".to_string()),
            routes: vec![],
            draft: false,
            security: None,
        };

        let response = client
            .post("/v1/api/definitions")
            .body_json(&definition)
            .send()
            .await;

        response.assert_status_is_ok();

        let response = client
            .post("/v1/api/definitions")
            .body_json(&definition)
            .send()
            .await;

        response.assert_status(http::StatusCode::CONFLICT);
    }

    #[test]
    async fn create_api_definition_yaml() {
        let (api, _db) = make_route().await;
        let client = TestClient::new(api);

        let definition = HttpApiDefinitionRequest {
            id: ApiDefinitionId("sample".to_string()),
            version: ApiVersion("42.0".to_string()),
            routes: vec![],
            draft: false,
            security: None,
        };

        let response = client
            .post("/v1/api/definitions")
            .body_yaml(&definition)
            .send()
            .await;

        response.assert_status(http::StatusCode::OK);
    }

    #[test]
    async fn create_api_definition_json() {
        let (api, _db) = make_route().await;
        let client = TestClient::new(api);

        let definition = HttpApiDefinitionRequest {
            id: ApiDefinitionId("sample".to_string()),
            version: ApiVersion("42.0".to_string()),
            routes: vec![],
            draft: false,
            security: None,
        };

        let response = client
            .post("/v1/api/definitions")
            .body_json(&definition)
            .send()
            .await;

        response.assert_status(http::StatusCode::OK);
    }

    #[test]
    async fn update_non_existant() {
        let (api, _db) = make_route().await;
        let client = TestClient::new(api);

        let definition = HttpApiDefinitionRequest {
            id: ApiDefinitionId("test".to_string()),
            version: ApiVersion("42.0".to_string()),
            routes: vec![],
            draft: false,
            security: None,
        };

        let response = client
            .put(format!(
                "/v1/api/definitions/{}/{}",
                definition.id.0, definition.version.0
            ))
            .body_json(&definition)
            .send()
            .await;

        response.assert_status(http::StatusCode::NOT_FOUND);
    }

    #[test]
    async fn get_all() {
        let (api, _db) = make_route().await;
        let client = TestClient::new(api);

        let definition = HttpApiDefinitionRequest {
            id: ApiDefinitionId("test".to_string()),
            version: ApiVersion("1.0".to_string()),
            routes: vec![],
            draft: false,
            security: None,
        };
        let response = client
            .post("/v1/api/definitions")
            .body_json(&definition)
            .send()
            .await;
        response.assert_status_is_ok();

        let definition = HttpApiDefinitionRequest {
            id: ApiDefinitionId("test".to_string()),
            version: ApiVersion("2.0".to_string()),
            routes: vec![],
            draft: false,
            security: None,
        };
        let response = client
            .post("/v1/api/definitions")
            .body_json(&definition)
            .send()
            .await;
        response.assert_status_is_ok();

        let response = client.get("/v1/api/definitions").send().await;
        response.assert_status_is_ok();
        let body = response.json().await;
        body.value().array().assert_len(2)
    }

    #[ignore] // There is already sql tests that does this
    #[test]
    async fn decode_openapi_json() {
        let (api, _db) = make_route().await;
        let client = TestClient::new(api);

        let response = client
            .put("/v1/api/definitions/import")
            .content_type("application/json")
            .body("Invalid JSON")
            .send()
            .await;

        response.assert_status(StatusCode::BAD_REQUEST);

        let response = client
            .put("/v1/api/definitions/import")
            .body_json(&serde_json::json!({
                "some": "json"
            }))
            .send()
            .await;

        response.assert_status(StatusCode::BAD_REQUEST);

        let openapi = r###"
        {
            "openapi": "3.0.0",
            "info": {
              "title": "Sample API",
              "version": "1.0.2"
            },
            "x-golem-api-definition-id": "shopping-cart-test-api",
            "x-golem-api-definition-version": "0.1.0",
            "paths": {
                "/{user-id}/get-cart-contents": {
                  "x-golem-worker-bridge": {
                    "worker-name": "worker-${request.path.user-id}",
                    "component-id": "2696abdc-df3a-4771-8215-d6af7aa4c408",
                    "component-version": "0",
                    "response": "${1}"
                  },
                  "get": {
                    "summary": "Get Cart Contents",
                    "description": "Get the contents of a user's cart",
                    "parameters": [
                      {
                        "name": "user-id",
                        "in": "path",
                        "required": true,
                        "schema": {
                          "type": "string"
                        }
                      }
                    ],
                    "responses": {
                      "200": {
                        "description": "OK",
                        "content":{
                          "application/json": {
                            "schema": {
                              "$ref": "#/components/schemas/CartItem"
                            }
                          }

                        }
                      },
                      "404": {
                        "description": "Contents not found"
                      }
                    }
                  }
                }
              },
              "components": {
                "schemas": {
                  "CartItem": {
                    "type": "object",
                    "properties": {
                      "id": {
                        "type": "string"
                      },
                      "name": {
                        "type": "string"
                      },
                      "price": {
                        "type": "number"
                      }
                    }
                  }
                }
            }
        }
        "###;

        let response = client
            .put("/v1/api/definitions/import")
            .content_type("application/json")
            .body(openapi)
            .send()
            .await;

        response.assert_status_is_ok();
    }

    #[test]
    async fn export_endpoint_preserves_custom_extensions() {
        let (api, _db) = make_route().await;
        let client = TestClient::new(api);

        let definition = HttpApiDefinitionRequest {
            id: ApiDefinitionId("test_export".to_string()),
            version: ApiVersion("1.0".to_string()),
            routes: vec![],
            draft: false,
            security: None,
        };

        let response = client
            .post("/v1/api/definitions")
            .body_json(&definition)
            .send()
            .await;
        response.assert_status(StatusCode::OK);

        let url = format!("/v1/api/definitions/{}/{}/export", definition.id.0, definition.version.0);
        let response = client.get(&url).send().await;
        response.assert_status(StatusCode::OK);

        let body_bytes = response.0.into_body().into_vec().await.expect("Failed to read body bytes");
        let body = String::from_utf8(body_bytes).unwrap();

        assert!(body.contains("x-golem-api-definition-id"));
        assert!(body.contains("test_export"));
        assert!(body.contains("x-golem-api-definition-version"));
        assert!(body.contains("1.0"));
        assert!(body.contains("security:"), "Exported YAML should contain 'security' field.");
        assert!(body.contains("corsPreflight:"), "Exported YAML should contain 'corsPreflight' field.");
    }

    #[test]
    async fn swagger_ui_binding_test() {
        let (api, _db) = make_route().await;
        let client = TestClient::new(api);

        let definition = HttpApiDefinitionRequest {
            id: ApiDefinitionId("swagger_test".to_string()),
            version: ApiVersion("1.0".to_string()),
            routes: vec![
                RouteRequestData {
                    method: MethodPattern::Get,
                    path: "/docs".to_string(),
                    binding: GatewayBindingData {
                        binding_type: Some(GatewayBindingType::SwaggerUi),
                        component_id: None,
                        worker_name: None,
                        idempotency_key: None,
                        response: None,
                        allow_origin: None,
                        allow_methods: None,
                        allow_headers: None,
                        expose_headers: None,
                        max_age: None,
                        allow_credentials: None,
                    },
                    security: None,
                    cors: None,
                }
            ],
            draft: false,
            security: None,
        };

        let response = client
            .post("/v1/api/definitions")
            .body_json(&definition)
            .send()
            .await;
        response.assert_status_is_ok();

        let url = format!("/v1/api/definitions/{}/{}", definition.id.0, definition.version.0);
        let response = client.get(&url).send().await;
        response.assert_status_is_ok();
        
        let body = response.json().await;
        let value = serde_json::to_value(body).unwrap();
        let body_value: HttpApiDefinitionResponseData = serde_json::from_value(value).unwrap();
        assert_eq!(body_value.routes.len(), 1);
        assert_eq!(body_value.routes[0].binding.binding_type, Some(golem_common::model::GatewayBindingType::SwaggerUi));

        let export_url = format!("/v1/api/definitions/{}/{}/export", definition.id.0, definition.version.0);
        let response = client.get(&export_url).send().await;
        response.assert_status_is_ok();

        let body_bytes = response.0.into_body().into_vec().await.expect("Failed to read body bytes");
        let body = String::from_utf8(body_bytes).unwrap();
        
        assert!(body.contains("swagger-ui"), "Exported YAML should contain 'swagger-ui' binding type");
        assert!(body.contains("x-golem-api-gateway-binding"), "Exported YAML should contain gateway binding extension");
    }
}
