use std::result::Result;
use std::sync::Arc;

use golem_worker_service_base::api_definition::http::JsonOpenApiDefinition;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::*;
use tracing::{error, info};

use golem_service_base::api_tags::ApiTags;
use golem_worker_service_base::api::ApiEndpointError;
use golem_worker_service_base::api::HttpApiDefinition;
use golem_worker_service_base::api_definition::http::get_api_definition;
use golem_worker_service_base::api_definition::http::HttpApiDefinition as CoreHttpApiDefinition;
use golem_worker_service_base::api_definition::{ApiDefinitionId, ApiVersion};
use golem_worker_service_base::auth::{DefaultNamespace, EmptyAuthCtx};
use golem_worker_service_base::service::api_definition::ApiDefinitionService;
use golem_worker_service_base::service::http::http_api_definition_validator::RouteValidationError;

pub struct RegisterApiDefinitionApi {
    definition_service: Arc<
        dyn ApiDefinitionService<EmptyAuthCtx, DefaultNamespace, RouteValidationError>
            + Sync
            + Send,
    >,
}

#[OpenApi(prefix_path = "/v1/api/definitions", tag = ApiTags::ApiDefinition)]
impl RegisterApiDefinitionApi {
    pub fn new(
        definition_service: Arc<
            dyn ApiDefinitionService<EmptyAuthCtx, DefaultNamespace, RouteValidationError>
                + Sync
                + Send,
        >,
    ) -> Self {
        Self { definition_service }
    }

    #[oai(path = "/import", method = "put", operation_id = "import_open_api")]
    async fn create_or_update_open_api(
        &self,
        Json(openapi): Json<JsonOpenApiDefinition>,
    ) -> Result<Json<HttpApiDefinition>, ApiEndpointError> {
        let definition = get_api_definition(openapi.0).map_err(|e| {
            error!("Invalid Spec {}", e);
            ApiEndpointError::bad_request(e)
        })?;

        self.create_api(&definition).await?;

        let definition: HttpApiDefinition =
            definition.try_into().map_err(ApiEndpointError::internal)?;

        Ok(Json(definition))
    }

    #[oai(path = "/", method = "post", operation_id = "create_definition")]
    async fn create(
        &self,
        payload: Json<HttpApiDefinition>,
    ) -> Result<Json<HttpApiDefinition>, ApiEndpointError> {
        info!("Save API definition - id: {}", &payload.id);

        let definition: CoreHttpApiDefinition = payload
            .0
            .try_into()
            .map_err(ApiEndpointError::bad_request)?;

        self.create_api(&definition).await?;

        let definition: HttpApiDefinition =
            definition.try_into().map_err(ApiEndpointError::internal)?;

        Ok(Json(definition))
    }
    #[oai(
        path = "/:id/:version",
        method = "put",
        operation_id = "update_definition"
    )]
    async fn update(
        &self,
        id: Path<ApiDefinitionId>,
        version: Path<ApiVersion>,
        payload: Json<HttpApiDefinition>,
    ) -> Result<Json<HttpApiDefinition>, ApiEndpointError> {
        info!("Update API definition - id: {}", &payload.id);

        let definition: CoreHttpApiDefinition = payload
            .0
            .try_into()
            .map_err(ApiEndpointError::bad_request)?;

        if id.0 != definition.id {
            return Err(ApiEndpointError::bad_request("Unmatched url and body ids."));
        }

        if version.0 != definition.version {
            return Err(ApiEndpointError::bad_request(
                "Unmatched url and body versions.",
            ));
        }

        self.definition_service
            .update(
                &definition,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await
            .map_err(|e| {
                error!("API Definition ID: {} - update error: {e:?}", definition.id);
                e
            })?;

        let definition: HttpApiDefinition =
            definition.try_into().map_err(ApiEndpointError::internal)?;

        Ok(Json(definition))
    }

    #[oai(
        path = "/:id/:version",
        method = "get",
        operation_id = "get_definition"
    )]
    async fn get(
        &self,
        id: Path<ApiDefinitionId>,
        version: Path<ApiVersion>,
    ) -> Result<Json<HttpApiDefinition>, ApiEndpointError> {
        let api_definition_id = id.0;

        let api_version = version.0;

        info!(
            "Get API definition - id: {}, version: {}",
            &api_definition_id, &api_version
        );

        let data = self
            .definition_service
            .get(
                &api_definition_id,
                &api_version,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        let data = data.ok_or(ApiEndpointError::not_found(format!(
            "Can't find api definition with id {api_definition_id}, and version {api_version}"
        )))?;

        let value: HttpApiDefinition = data.try_into().map_err(ApiEndpointError::internal)?;

        Ok(Json(value))
    }

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
        let api_definition_id = id.0;
        let api_definition_version = version.0;

        info!("Delete API definition - id: {}", &api_definition_id);

        let deleted = self
            .definition_service
            .delete(
                &api_definition_id,
                &api_definition_version,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        if deleted.is_some() {
            Ok(Json("API definition deleted".to_string()))
        } else {
            Ok(Json("API definition not found".to_string()))
        }
    }

    #[oai(path = "/", method = "get", operation_id = "list_definitions")]
    async fn list(
        &self,
        #[oai(name = "api-definition-id")] api_definition_id_query: Query<Option<ApiDefinitionId>>,
    ) -> Result<Json<Vec<HttpApiDefinition>>, ApiEndpointError> {
        let data = if let Some(id) = api_definition_id_query.0 {
            self.definition_service
                .get_all_versions(&id, &DefaultNamespace::default(), &EmptyAuthCtx::default())
                .await?
        } else {
            self.definition_service
                .get_all(&DefaultNamespace::default(), &EmptyAuthCtx::default())
                .await?
        };

        let values = data
            .into_iter()
            .map(|d| d.try_into())
            .collect::<Result<Vec<HttpApiDefinition>, _>>()
            .map_err(ApiEndpointError::internal)?;

        Ok(Json(values))
    }
}

impl RegisterApiDefinitionApi {
    async fn create_api(&self, definition: &CoreHttpApiDefinition) -> Result<(), ApiEndpointError> {
        self.definition_service
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

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use golem_worker_service_base::service::api_definition_validator::ApiDefinitionValidatorNoop;
    use golem_worker_service_base::service::component::ComponentServiceNoop;
    use http::StatusCode;
    use poem::test::TestClient;

    use golem_worker_service_base::repo::api_definition::InMemoryApiDefinitionRepo;
    use golem_worker_service_base::service::api_definition::ApiDefinitionServiceDefault;

    use crate::service::component::ComponentService;

    use super::*;

    fn make_route() -> poem::Route {
        let component_service: ComponentService = Arc::new(ComponentServiceNoop {});
        let definition_service = ApiDefinitionServiceDefault::new(
            component_service,
            Arc::new(InMemoryApiDefinitionRepo::default()),
            Arc::new(ApiDefinitionValidatorNoop {}),
        );

        let endpoint = RegisterApiDefinitionApi::new(Arc::new(definition_service));

        poem::Route::new().nest("", OpenApiService::new(endpoint, "test", "1.0"))
    }

    #[tokio::test]
    async fn conflict_error_returned() {
        let api = make_route();
        let client = TestClient::new(api);

        let definition = golem_worker_service_base::api_definition::http::HttpApiDefinition {
            id: ApiDefinitionId("test".to_string()),
            version: ApiVersion("1.0".to_string()),
            routes: vec![],
            draft: false,
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

    #[tokio::test]
    async fn update_non_existant() {
        let api = make_route();
        let client = TestClient::new(api);

        let definition = golem_worker_service_base::api_definition::http::HttpApiDefinition {
            id: ApiDefinitionId("test".to_string()),
            version: ApiVersion("42.0".to_string()),
            routes: vec![],
            draft: false,
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

    #[tokio::test]
    async fn get_all() {
        let api = make_route();
        let client = TestClient::new(api);

        let definition = golem_worker_service_base::api_definition::http::HttpApiDefinition {
            id: ApiDefinitionId("test".to_string()),
            version: ApiVersion("1.0".to_string()),
            routes: vec![],
            draft: false,
        };
        let response = client
            .post("/v1/api/definitions")
            .body_json(&definition)
            .send()
            .await;
        response.assert_status_is_ok();

        let definition = golem_worker_service_base::api_definition::http::HttpApiDefinition {
            id: ApiDefinitionId("test".to_string()),
            version: ApiVersion("2.0".to_string()),
            routes: vec![],
            draft: false,
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

    #[tokio::test]
    async fn decode_openapi_json() {
        let api = make_route();
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
                    "response": "${{headers : {ContentType: \"json\", user-id: \"foo\"}, body: worker.response, status: 200}}"
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
