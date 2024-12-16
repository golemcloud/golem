// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::api::{
    api_deployment::ApiDeploymentApi,
    openapi_generator::{generate_openapi, OpenApiSpec},
    security_scheme::SecuritySchemeApi,
    worker::WorkerApi,
};
use crate::service::Services;
use golem_api_grpc::proto::golem::apidefinition::{HttpApiDefinition, HttpRoute};
use golem_common::{json_yaml::JsonOrYaml, recorded_http_api_request, safe, SafeDisplay};
use golem_service_base::api_tags::ApiTags;
use golem_service_base::auth::{DefaultNamespace, EmptyAuthCtx};
use golem_worker_service_base::api::{
    ApiEndpointError, HealthcheckApi, HttpApiDefinitionRequest, HttpApiDefinitionResponseData,
};
use golem_worker_service_base::gateway_api_definition::http::{
    CompiledHttpApiDefinition, HttpApiDefinitionRequest as CoreHttpApiDefinitionRequest,
    OpenApiHttpApiDefinitionRequest,
};
use golem_worker_service_base::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use golem_worker_service_base::service::gateway::api_definition::ApiDefinitionService;
use poem::Route;
use poem_openapi::{
    param::{Path, Query},
    payload::{Json, PlainText},
    OpenApi, OpenApiService,
};
use std::result::Result;
use std::sync::Arc;
use tracing::{error, Instrument};

#[derive(Clone)]
pub struct RegisterApiDefinitionApi {
    definition_service: Arc<dyn ApiDefinitionService<EmptyAuthCtx, DefaultNamespace> + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/api/definitions", tag = ApiTags::ApiDefinition)]
impl RegisterApiDefinitionApi {
    pub fn new(
        definition_service: Arc<
            dyn ApiDefinitionService<EmptyAuthCtx, DefaultNamespace> + Sync + Send,
        >,
    ) -> Self {
        Self { definition_service }
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

            let result = HttpApiDefinitionResponseData::try_from(result).map_err(|e| {
                error!("Failed to convert to response data {}", e);
                ApiEndpointError::internal(safe(e))
            });

            result.map(Json)
        };

        record.result(response)
    }

    /// Create a new API definition
    ///
    /// Creates a new API definition described by Golem's API definition JSON document.
    /// If an API definition of the same version already exists, it's an error.
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

            let result =
                HttpApiDefinitionResponseData::try_from(compiled_definition).map_err(|e| {
                    error!("Failed to convert to response data {}", e);
                    ApiEndpointError::internal(safe(e))
                });

            result.map(Json)
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

                let result =
                    HttpApiDefinitionResponseData::try_from(compiled_definition).map_err(|e| {
                        error!("Failed to convert to response data {}", e);
                        ApiEndpointError::internal(safe(e))
                    });

                result.map(Json)
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

            let result =
                HttpApiDefinitionResponseData::try_from(compiled_definition).map_err(|e| {
                    error!("Failed to convert to response data {}", e);
                    ApiEndpointError::internal(safe(e))
                });

            result.map(Json)
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
    /// Otherwise, lists all API definitions.
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

            let values = data
                .into_iter()
                .map(HttpApiDefinitionResponseData::try_from)
                .collect::<Result<Vec<_>, String>>()
                .map_err(|e| {
                    error!("Failed to convert to response data {}", e);
                    ApiEndpointError::internal(safe(e))
                })?;

            Ok(Json(values))
        };
        record.result(response)
    }

    /// Export an API Definition to OpenAPI
    #[oai(
        path = "/:id/:version/export",
        method = "get",
        operation_id = "export_api_definition"
    )]
    pub async fn export(
        &self,
        id: Path<ApiDefinitionId>,
        version: Path<ApiVersion>,
    ) -> Result<Json<OpenApiSpec>, ApiEndpointError> {
        let record = recorded_http_api_request!(
            "export_api_definition",
            api_definition_id = id.0.to_string(),
            version = version.0.to_string()
        );

        let response = {
            // Retrieve the API definition
            let api_definition = self
                .definition_service
                .get(
                    &id.0,
                    &version.0,
                    &DefaultNamespace::default(),
                    &EmptyAuthCtx::default(),
                )
                .instrument(record.span.clone())
                .await?;

            // Handle case where the API definition is not found
            let compiled_definition = api_definition.ok_or_else(|| {
                ApiEndpointError::not_found(safe(format!(
                    "No API definition found for id {} and version {}",
                    id.0, version.0
                )))
            })?;

            // Convert `CompiledHttpApiDefinition` to `HttpApiDefinition`

            let http_api_definition =
                convert_compiled_to_http(compiled_definition).map_err(|err| {
                    ApiEndpointError::internal(safe(format!(
                        "Error converting API definition: {:?}",
                        err
                    )))
                })?;

            // Generate OpenAPI Specification
            let openapi_spec = generate_openapi(&http_api_definition, &version.0.to_string());

            Ok(Json(openapi_spec))
        };

        record.result(response)
    }

    #[oai(path = "/swagger-ui/*path", method = "get")]
    async fn swagger_ui(&self, path: Path<String>) -> PlainText<String> {
        PlainText(format!("Swagger UI for path: {}", path.0))
    }
}

/// Configures the routes for the OpenAPI service, including `/docs` for Swagger and `/specs` for YAML.
fn build_api_routes(services: Arc<Services>) -> Route {
    let api_service = Arc::new(OpenApiService::new(
        (
            WorkerApi {
                component_service: services.component_service.clone(),
                worker_service: services.worker_service.clone(),
            },
            RegisterApiDefinitionApi::new(services.definition_service.clone()),
            ApiDeploymentApi::new(services.deployment_service.clone()),
            SecuritySchemeApi::new(services.security_scheme_service.clone()),
            HealthcheckApi,
        ),
        "API Service",
        "1.0",
    ));

    Route::new()
        .nest("/", api_service.spec_endpoint())
        .nest("/docs", api_service.swagger_ui())
        .nest("/specs", api_service.spec_endpoint_yaml())
}

struct SafeString(String);

impl SafeDisplay for SafeString {
    fn to_safe_string(&self) -> String {
        self.0.clone()
    }
}

/// Converts a `CompiledHttpApiDefinition<DefaultNamespace>` to `HttpApiDefinition`.
fn convert_compiled_to_http(
    compiled: CompiledHttpApiDefinition<DefaultNamespace>,
) -> Result<HttpApiDefinition, ApiEndpointError> {
    let routes = compiled
        .routes
        .into_iter()
        .map(|route| {
            Ok(HttpRoute {
                path: route.path.to_string(),
                method: http_method_to_i32(&route.method.to_string())?,
                binding: None,
                middleware: None,
            })
        })
        .collect::<Result<Vec<HttpRoute>, ApiEndpointError>>()?;
    Ok(HttpApiDefinition { routes })
}

fn http_method_to_i32(method: &str) -> Result<i32, ApiEndpointError> {
    match method.to_uppercase().as_str() {
        "GET" => Ok(0),
        "POST" => Ok(1),
        "PUT" => Ok(2),
        "DELETE" => Ok(3),
        "PATCH" => Ok(4),
        "HEAD" => Ok(5),
        "OPTIONS" => Ok(6),
        "TRACE" => Ok(7),
        _ => Err(ApiEndpointError::bad_request(SafeString(
            method.to_string(),
        ))),
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
            component_service,
            api_definition_repo,
            api_deployment_repo,
            security_scheme_service,
            Arc::new(HttpApiDefinitionValidator {}),
        );

        let endpoint = RegisterApiDefinitionApi::new(Arc::new(definition_service));

        (
            poem::Route::new().nest("", OpenApiService::new(endpoint, "test", "1.0")),
            db,
        )
    }

    #[test]
    async fn conflict_error_returned() {
        // Test to ensure attempting to create a duplicate API definition returns a conflict error.
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
    async fn update_non_existent() {
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
}
