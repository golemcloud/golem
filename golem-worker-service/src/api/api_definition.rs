use futures::future::try_join_all;
// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
use golem_common::json_yaml::JsonOrYaml;
use golem_common::{recorded_http_api_request, safe};
use golem_service_base::api_tags::ApiTags;
use golem_service_base::auth::{DefaultNamespace, EmptyAuthCtx};
use golem_worker_service_base::api::ApiEndpointError;
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
        payload: JsonOrYaml<OpenApiHttpApiDefinition>,
    ) -> Result<Json<HttpApiDefinitionResponseData>, ApiEndpointError> {
        let record = recorded_http_api_request!("import_open_api",);

        let response = self
            .create_or_update_open_api_internal(payload.0)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn create_or_update_open_api_internal(
        &self,
        payload: OpenApiHttpApiDefinition,
    ) -> Result<Json<HttpApiDefinitionResponseData>, ApiEndpointError> {
        let compiled_definition = self
            .definition_service
            .create_with_oas(
                &payload,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        let result = HttpApiDefinitionResponseData::from_compiled_http_api_definition(
            compiled_definition,
            &self
                .definition_service
                .conversion_context(&DefaultNamespace(), &EmptyAuthCtx::default()),
        )
        .await
        .map_err(|e| {
            error!("Failed to convert to response data {}", e);
            ApiEndpointError::internal(safe(e))
        })?;

        Ok(Json(result))
    }

    /// Create a new API definition
    ///
    /// Creates a new API definition described by Golem's API definition JSON document.
    /// If an API definition of the same version already exists, it is an error.
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

        let response = self
            .create_internal(payload.0)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn create_internal(
        &self,
        payload: HttpApiDefinitionRequest,
    ) -> Result<Json<HttpApiDefinitionResponseData>, ApiEndpointError> {
        let definition: CoreHttpApiDefinitionRequest = payload
            .into_core(
                &self
                    .definition_service
                    .conversion_context(&DefaultNamespace(), &EmptyAuthCtx::default()),
            )
            .await
            .map_err(|err| ApiEndpointError::bad_request(safe(err)))?;

        let compiled_definition = self
            .definition_service
            .create(
                &definition,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        let result = HttpApiDefinitionResponseData::from_compiled_http_api_definition(
            compiled_definition,
            &self
                .definition_service
                .conversion_context(&DefaultNamespace(), &EmptyAuthCtx::default()),
        )
        .await
        .map_err(|e| {
            error!("Failed to convert to response data {}", e);
            ApiEndpointError::internal(safe(e))
        })?;

        Ok(Json(result))
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

        let response = self
            .update_internal(id.0, version.0, payload.0)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn update_internal(
        &self,
        id: ApiDefinitionId,
        version: ApiVersion,
        payload: HttpApiDefinitionRequest,
    ) -> Result<Json<HttpApiDefinitionResponseData>, ApiEndpointError> {
        let definition: CoreHttpApiDefinitionRequest = payload
            .into_core(
                &self
                    .definition_service
                    .conversion_context(&DefaultNamespace(), &EmptyAuthCtx::default()),
            )
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
            let compiled_definition = self
                .definition_service
                .update(
                    &definition,
                    &DefaultNamespace::default(),
                    &EmptyAuthCtx::default(),
                )
                .await?;

            let result = HttpApiDefinitionResponseData::from_compiled_http_api_definition(
                compiled_definition,
                &self
                    .definition_service
                    .conversion_context(&DefaultNamespace(), &EmptyAuthCtx::default()),
            )
            .await
            .map_err(|e| {
                error!("Failed to convert to response data {}", e);
                ApiEndpointError::internal(safe(e))
            })?;

            Ok(Json(result))
        }
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

        let response = self
            .get_internal(id.0, version.0)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn get_internal(
        &self,
        id: ApiDefinitionId,
        version: ApiVersion,
    ) -> Result<Json<HttpApiDefinitionResponseData>, ApiEndpointError> {
        let data = self
            .definition_service
            .get(
                &id,
                &version,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        let compiled_definition = data.ok_or(ApiEndpointError::not_found(safe(format!(
            "Can't find api definition with id {id}, and version {version}"
        ))))?;

        let result = HttpApiDefinitionResponseData::from_compiled_http_api_definition(
            compiled_definition,
            &self
                .definition_service
                .conversion_context(&DefaultNamespace(), &EmptyAuthCtx::default()),
        )
        .await
        .map_err(|e| {
            error!("Failed to convert to response data {}", e);
            ApiEndpointError::internal(safe(e))
        })?;

        Ok(Json(result))
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

        let response = self
            .delete_internal(id.0, version.0)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn delete_internal(
        &self,
        id: ApiDefinitionId,
        version: ApiVersion,
    ) -> Result<Json<String>, ApiEndpointError> {
        self.definition_service
            .delete(
                &id,
                &version,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        Ok(Json("API definition deleted".to_string()))
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

        let response = self
            .list_internal(api_definition_id_query.0)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn list_internal(
        &self,
        api_definition_id_query: Option<ApiDefinitionId>,
    ) -> Result<Json<Vec<HttpApiDefinitionResponseData>>, ApiEndpointError> {
        let auth_ctx = EmptyAuthCtx::default();

        let data = if let Some(id) = api_definition_id_query {
            self.definition_service
                .get_all_versions(&id, &DefaultNamespace::default(), &auth_ctx)
                .await?
        } else {
            self.definition_service
                .get_all(&DefaultNamespace::default(), &auth_ctx)
                .await?
        };

        let conversion_context = self
            .definition_service
            .conversion_context(&DefaultNamespace(), &auth_ctx);

        let converted = data.into_iter().map(|d| {
            HttpApiDefinitionResponseData::from_compiled_http_api_definition(d, &conversion_context)
        });

        let values = try_join_all(converted).await.map_err(|e| {
            error!("Failed to convert to response data {}", e);
            ApiEndpointError::internal(safe(e))
        })?;

        Ok(Json(values))
    }
}

#[cfg(test)]
mod test {
    use golem_service_base::migration::{Migrations, MigrationsDir};
    use test_r::test;

    use super::*;
    use async_trait::async_trait;
    use golem_common::config::DbSqliteConfig;
    use golem_common::model::component_constraint::{FunctionConstraints, FunctionSignature};
    use golem_common::model::ComponentId;
    use golem_service_base::db;
    use golem_service_base::db::sqlite::SqlitePool;
    use golem_service_base::model::{Component, ComponentName};
    use golem_worker_service_base::gateway_security::DefaultIdentityProvider;
    use golem_worker_service_base::repo::api_definition::{
        ApiDefinitionRepo, DbApiDefinitionRepo, LoggedApiDefinitionRepo,
    };
    use golem_worker_service_base::repo::api_deployment;
    use golem_worker_service_base::repo::security_scheme::{
        DbSecuritySchemeRepo, LoggedSecuritySchemeRepo, SecuritySchemeRepo,
    };
    use golem_worker_service_base::service::component::ComponentResult;
    use golem_worker_service_base::service::gateway::api_definition::{
        ApiDefinitionServiceConfig, ApiDefinitionServiceDefault,
    };
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
    impl
        golem_worker_service_base::service::component::ComponentService<
            DefaultNamespace,
            EmptyAuthCtx,
        > for TestComponentService
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

        async fn get_by_name(
            &self,
            _component_id: &ComponentName,
            _namespace: &DefaultNamespace,
            _auth_ctx: &EmptyAuthCtx,
        ) -> ComponentResult<Component> {
            unimplemented!()
        }

        async fn create_or_update_constraints(
            &self,
            _component_id: &ComponentId,
            _constraints: FunctionConstraints,
            _auth_ctx: &EmptyAuthCtx,
        ) -> ComponentResult<FunctionConstraints> {
            unimplemented!()
        }

        async fn delete_constraints(
            &self,
            _component_id: &ComponentId,
            _constraints: &[FunctionSignature],
            _auth_ctx: &EmptyAuthCtx,
        ) -> ComponentResult<FunctionConstraints> {
            unimplemented!()
        }
    }

    async fn make_route<'c>() -> (poem::Route, SqliteDb<'c>) {
        let db = SqliteDb::default();
        let db_config = DbSqliteConfig {
            database: db.db_path.to_string(),
            max_connections: 10,
        };

        db::sqlite::migrate(
            &db_config,
            MigrationsDir::new("./db/migration".into()).sqlite_migrations(),
        )
        .await
        .unwrap();

        let db_pool = SqlitePool::configured(&db_config).await.unwrap();

        let api_definition_repo: Arc<dyn ApiDefinitionRepo + Sync + Send> = Arc::new(
            LoggedApiDefinitionRepo::new(DbApiDefinitionRepo::new(db_pool.clone())),
        );
        let api_deployment_repo: Arc<dyn api_deployment::ApiDeploymentRepo + Sync + Send> =
            Arc::new(api_deployment::LoggedDeploymentRepo::new(
                api_deployment::DbApiDeploymentRepo::new(db_pool.clone()),
            ));

        let security_scheme_repo: Arc<dyn SecuritySchemeRepo + Sync + Send> = Arc::new(
            LoggedSecuritySchemeRepo::new(DbSecuritySchemeRepo::new(db_pool.clone())),
        );

        let identity_provider = Arc::new(DefaultIdentityProvider);

        let security_scheme_service = Arc::new(DefaultSecuritySchemeService::new(
            security_scheme_repo,
            identity_provider,
        ));

        let component_service = Arc::new(TestComponentService);
        let definition_service = ApiDefinitionServiceDefault::new(
            component_service,
            api_definition_repo,
            api_deployment_repo,
            security_scheme_service,
            Arc::new(HttpApiDefinitionValidator {}),
            ApiDefinitionServiceConfig::default(),
        );

        let endpoint = RegisterApiDefinitionApi::new(Arc::new(definition_service));

        (
            poem::Route::new().nest("", OpenApiService::new(endpoint, "test", "1.0")),
            db,
        )
    }

    #[test]
    async fn conflict_error_returned() {
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

        let response = client
            .post("/v1/api/definitions")
            .body_json(&definition)
            .send()
            .await;

        response.assert_status(StatusCode::CONFLICT);
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

        response.assert_status(StatusCode::OK);
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

        response.assert_status(StatusCode::OK);
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

        response.assert_status(StatusCode::NOT_FOUND);
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
