use std::result::Result;
use std::sync::Arc;

use poem_openapi::param::Query;
use poem_openapi::payload::Json;
use poem_openapi::*;
use tracing::{error, info};

use golem_service_base::api_tags::ApiTags;
use golem_worker_service_base::api::ApiEndpointError;
use golem_worker_service_base::api::HttpApiDefinition;
use golem_worker_service_base::api_definition::http::get_api_definition_from_oas;
use golem_worker_service_base::api_definition::http::HttpApiDefinition as CoreHttpApiDefinition;
use golem_worker_service_base::api_definition::{ApiDefinitionId, ApiVersion};
use golem_worker_service_base::auth::{CommonNamespace, EmptyAuthCtx};
use golem_worker_service_base::service::api_definition::ApiDefinitionService;
use golem_worker_service_base::service::http::http_api_definition_validator::RouteValidationError;

pub struct RegisterApiDefinitionApi {
    pub definition_service: DefinitionService,
}

type DefinitionService = Arc<
    dyn ApiDefinitionService<
            EmptyAuthCtx,
            CommonNamespace,
            CoreHttpApiDefinition,
            RouteValidationError,
        > + Sync
        + Send,
>;

#[OpenApi(prefix_path = "/v1/api/definitions", tag = ApiTags::ApiDefinition)]
impl RegisterApiDefinitionApi {
    pub fn new(definition_service: DefinitionService) -> Self {
        Self { definition_service }
    }

    #[oai(path = "/oas", method = "put")]
    async fn create_or_update_open_api(
        &self,
        payload: String,
    ) -> Result<Json<HttpApiDefinition>, ApiEndpointError> {
        let definition = get_api_definition_from_oas(payload.as_str()).map_err(|e| {
            error!("Invalid Spec {}", e);
            ApiEndpointError::bad_request(e)
        })?;

        self.register_api(&definition).await?;

        let definition: HttpApiDefinition =
            definition.try_into().map_err(ApiEndpointError::internal)?;

        Ok(Json(definition))
    }

    #[oai(path = "/", method = "put")]
    async fn create_or_update(
        &self,
        payload: Json<HttpApiDefinition>,
    ) -> Result<Json<HttpApiDefinition>, ApiEndpointError> {
        info!("Save API definition - id: {}", &payload.id);

        let definition: CoreHttpApiDefinition = payload
            .0
            .try_into()
            .map_err(ApiEndpointError::bad_request)?;

        self.register_api(&definition).await?;

        let definition: HttpApiDefinition =
            definition.try_into().map_err(ApiEndpointError::internal)?;

        Ok(Json(definition))
    }

    #[oai(path = "/", method = "get")]
    async fn get(
        &self,
        #[oai(name = "api-definition-id")] api_definition_id_query: Query<ApiDefinitionId>,
        #[oai(name = "version")] api_definition_id_version: Query<ApiVersion>,
    ) -> Result<Json<Vec<HttpApiDefinition>>, ApiEndpointError> {
        let api_definition_id = api_definition_id_query.0;

        let api_version = api_definition_id_version.0;

        info!(
            "Get API definition - id: {}, version: {}",
            &api_definition_id, &api_version
        );

        let data = self
            .definition_service
            .get(
                &api_definition_id,
                &api_version,
                CommonNamespace::default(),
                &EmptyAuthCtx {},
            )
            .await?;

        let values: Vec<HttpApiDefinition> = match data {
            Some(d) => {
                let definition: HttpApiDefinition =
                    d.try_into().map_err(ApiEndpointError::internal)?;
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
        #[oai(name = "version")] api_definition_version_query: Query<ApiVersion>,
    ) -> Result<Json<String>, ApiEndpointError> {
        let api_definition_id = api_definition_id_query.0;
        let api_definition_version = api_definition_version_query.0;

        info!("Delete API definition - id: {}", &api_definition_id);

        let deleted = self
            .definition_service
            .delete(
                &api_definition_id,
                &api_definition_version,
                CommonNamespace::default(),
                &EmptyAuthCtx {},
            )
            .await?;

        if deleted.is_some() {
            Ok(Json("API definition deleted".to_string()))
        } else {
            Ok(Json("API definition not found".to_string()))
        }
    }

    #[oai(path = "/all", method = "get")]
    async fn get_all(&self) -> Result<Json<Vec<HttpApiDefinition>>, ApiEndpointError> {
        let data = self
            .definition_service
            .get_all(CommonNamespace::default(), &EmptyAuthCtx {})
            .await?;

        let values = data
            .into_iter()
            .map(|d| d.try_into())
            .collect::<Result<Vec<HttpApiDefinition>, _>>()
            .map_err(ApiEndpointError::internal)?;

        Ok(Json(values))
    }
}

impl RegisterApiDefinitionApi {
    async fn register_api(
        &self,
        definition: &CoreHttpApiDefinition,
    ) -> Result<(), ApiEndpointError> {
        self.definition_service
            .register(definition, CommonNamespace::default(), &EmptyAuthCtx {})
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
    use golem_worker_service_base::service::api_definition_validator::ApiDefinitionValidatorNoop;
    use golem_worker_service_base::service::template::TemplateServiceNoop;
    use poem::test::TestClient;

    use golem_worker_service_base::repo::api_definition_repo::InMemoryRegistry;
    use golem_worker_service_base::service::api_definition::ApiDefinitionServiceDefault;

    use crate::service::template::TemplateService;

    use super::*;

    fn make_route() -> poem::Route {
        let template_service: TemplateService = Arc::new(TemplateServiceNoop {});
        let definition_service = ApiDefinitionServiceDefault::new(
            template_service,
            Arc::new(InMemoryRegistry::default()),
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

        let definition = golem_worker_service_base::api_definition::http::HttpApiDefinition {
            id: ApiDefinitionId("test".to_string()),
            version: ApiVersion("1.0".to_string()),
            routes: vec![],
        };
        let response = client
            .put("/v1/api/definitions")
            .body_json(&definition)
            .send()
            .await;
        response.assert_status_is_ok();

        let definition = golem_worker_service_base::api_definition::http::HttpApiDefinition {
            id: ApiDefinitionId("test".to_string()),
            version: ApiVersion("2.0".to_string()),
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
