use std::result::Result;
use std::sync::Arc;

use poem_openapi::param::Query;
use poem_openapi::payload::Json;
use poem_openapi::*;
use tracing::{error, info};

use golem_service_base::api_tags::ApiTags;
use golem_worker_service_base::api::common::ApiEndpointError;
use golem_worker_service_base::api::register_api_definition_api::ApiDefinition;
use golem_worker_service_base::api_definition;
use golem_worker_service_base::api_definition::{ApiDefinitionId, Version};
use golem_worker_service_base::auth::{AuthService, CommonNamespace, EmptyAuthCtx};
use golem_worker_service_base::oas_worker_bridge::*;
use golem_worker_service_base::service::api_definition_service::ApiDefinitionService;

pub struct RegisterApiDefinitionApi {
    pub definition_service:
        Arc<dyn ApiDefinitionService<CommonNamespace, EmptyAuthCtx> + Sync + Send>,
    pub auth_service: Arc<dyn AuthService<EmptyAuthCtx, CommonNamespace> + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/api/definitions", tag = ApiTags::ApiDefinition)]
impl RegisterApiDefinitionApi {
    pub fn new(
        definition_service: Arc<
            dyn ApiDefinitionService<CommonNamespace, EmptyAuthCtx> + Sync + Send,
        >,
        auth_service: Arc<dyn AuthService<EmptyAuthCtx, CommonNamespace> + Sync + Send>,
    ) -> Self {
        Self {
            definition_service,
            auth_service,
        }
    }

    #[oai(path = "/oas", method = "put")]
    async fn create_or_update_open_api(
        &self,
        payload: String,
    ) -> Result<Json<ApiDefinition>, ApiEndpointError> {
        let definition = get_api_definition(payload.as_str()).map_err(|e| {
            error!("Invalid Spec {}", e);
            ApiEndpointError::bad_request(e)
        })?;

        self.register_api(&definition).await?;

        let definition: ApiDefinition =
            definition.try_into().map_err(ApiEndpointError::internal)?;

        Ok(Json(definition))
    }

    #[oai(path = "/", method = "put")]
    async fn create_or_update(
        &self,
        payload: Json<ApiDefinition>,
    ) -> Result<Json<ApiDefinition>, ApiEndpointError> {
        info!("Save API definition - id: {}", &payload.id);

        let definition: api_definition::ApiDefinition = payload
            .0
            .try_into()
            .map_err(ApiEndpointError::bad_request)?;

        self.register_api(&definition).await?;

        let definition: ApiDefinition =
            definition.try_into().map_err(ApiEndpointError::internal)?;

        Ok(Json(definition))
    }

    #[oai(path = "/", method = "get")]
    async fn get(
        &self,
        #[oai(name = "api-definition-id")] api_definition_id_query: Query<ApiDefinitionId>,
        #[oai(name = "version")] api_definition_id_version: Query<Version>,
    ) -> Result<Json<Vec<ApiDefinition>>, ApiEndpointError> {
        let api_definition_id = api_definition_id_query.0;

        let api_version = api_definition_id_version.0;

        info!(
            "Get API definition - id: {}, version: {}",
            &api_definition_id, &api_version
        );

        let data = self
            .definition_service
            .get(&api_definition_id, &api_version, EmptyAuthCtx {})
            .await?;

        let values: Vec<ApiDefinition> = match data {
            Some(d) => {
                let definition: ApiDefinition = d
                    .api_definition
                    .try_into()
                    .map_err(ApiEndpointError::internal)?;
                vec![definition]
            }
            None => vec![],
        };

        Ok(Json(values))
    }

    #[oai(path = "/", method = "delete")]
    async fn delete(
        &self,
        #[oai(name = "api-definition-id")] api_definition_id_query: Query<ApiDefinitionId>,
        #[oai(name = "version")] api_definition_version_query: Query<Version>,
    ) -> Result<Json<String>, ApiEndpointError> {
        let api_definition_id = api_definition_id_query.0;
        let api_definition_version = api_definition_version_query.0;

        info!("Delete API definition - id: {}", &api_definition_id);

        let deleted = self
            .definition_service
            .delete(&api_definition_id, &api_definition_version, EmptyAuthCtx {})
            .await?;

        if deleted.is_some() {
            Ok(Json("API definition deleted".to_string()))
        } else {
            Ok(Json("API definition not found".to_string()))
        }
    }

    #[oai(path = "/all", method = "get")]
    async fn get_all(&self) -> Result<Json<Vec<ApiDefinition>>, ApiEndpointError> {
        let data = self.definition_service.get_all(EmptyAuthCtx {}).await?;

        let values = data
            .into_iter()
            .map(|d| d.api_definition.try_into())
            .collect::<Result<Vec<ApiDefinition>, _>>()
            .map_err(ApiEndpointError::internal)?;

        Ok(Json(values))
    }
}

impl RegisterApiDefinitionApi {
    async fn register_api(
        &self,
        definition: &api_definition::ApiDefinition,
    ) -> Result<(), ApiEndpointError> {
        self.definition_service
            .register(definition, EmptyAuthCtx {})
            .await
            .map_err(|e| {
                error!("API definition id: {} - register error: {e}", definition.id,);
                e
            })?;

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use golem_worker_service_base::auth::AuthServiceNoop;
    use poem::test::TestClient;

    use golem_worker_service_base::api_definition_repo::InMemoryRegistry;
    use golem_worker_service_base::service::api_definition_service::RegisterApiDefinitionDefault;

    use super::*;

    fn make_route() -> poem::Route {
        let definition_service = RegisterApiDefinitionDefault::new(
            Arc::new(AuthServiceNoop {}),
            Arc::new(InMemoryRegistry::default()),
        );

        let endpoint = RegisterApiDefinitionApi::new(
            Arc::new(definition_service),
            Arc::new(AuthServiceNoop {}),
        );

        poem::Route::new().nest("", OpenApiService::new(endpoint, "test", "1.0"))
    }

    #[tokio::test]
    async fn conflict_error_returned() {
        let api = make_route();
        let client = TestClient::new(api);

        let definition = api_definition::ApiDefinition {
            id: ApiDefinitionId("test".to_string()),
            version: Version("1.0".to_string()),
            routes: vec![],
        };

        let response = client
            .put("/v1/api/definitions")
            .body_json(&definition)
            .send()
            .await;

        response.assert_status_is_ok();

        let response = client
            .put("/v1/api/definitions")
            .body_json(&definition)
            .send()
            .await;

        response.assert_status(http::StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn get_all() {
        let api = make_route();
        let client = TestClient::new(api);

        let definition = api_definition::ApiDefinition {
            id: ApiDefinitionId("test".to_string()),
            version: Version("1.0".to_string()),
            routes: vec![],
        };
        let response = client
            .put("/v1/api/definitions")
            .body_json(&definition)
            .send()
            .await;
        response.assert_status_is_ok();

        let definition = api_definition::ApiDefinition {
            id: ApiDefinitionId("test".to_string()),
            version: Version("2.0".to_string()),
            routes: vec![],
        };
        let response = client
            .put("/v1/api/definitions")
            .body_json(&definition)
            .send()
            .await;
        response.assert_status_is_ok();

        let response = client.get("/v1/api/definitions/all").send().await;
        response.assert_status_is_ok();
        let body = response.json().await;
        body.value().array().assert_len(2)
    }
}
