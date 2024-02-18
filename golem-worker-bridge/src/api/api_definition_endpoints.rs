use std::collections::HashMap;
use std::result::Result;
use std::sync::Arc;

use golem_common::model::TemplateId;
use poem_openapi::param::Query;
use poem_openapi::payload::Json;
use poem_openapi::*;
use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::api::common::{ApiEndpointError, ApiTags};
use crate::api_definition;
use crate::api_definition::{ApiDefinitionId, MethodPattern, Version};
use crate::expr::Expr;
use crate::oas_worker_bridge::*;
use crate::register::{ApiDefinitionKey, ApiRegistrationError, RegisterApiDefinition};

pub struct RegisterApiDefinitionApi {
    pub definition_service: Arc<dyn RegisterApiDefinition + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/api/definitions", tag = ApiTags::ApiDefinition)]
impl RegisterApiDefinitionApi {
    pub fn new(definition_service: Arc<dyn RegisterApiDefinition + Sync + Send>) -> Self {
        Self { definition_service }
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

        register_api(self.definition_service.clone(), &definition).await?;

        let api_definition_key = ApiDefinitionKey {
            id: definition.id,
            version: definition.version,
        };

        let data = self
            .definition_service
            .get(&api_definition_key)
            .await
            .map_err(ApiEndpointError::internal)?;

        let definition = data.ok_or(ApiEndpointError::not_found("API Definition not found"))?;

        let definition: ApiDefinition =
            definition.try_into().map_err(ApiEndpointError::internal)?;

        Ok(Json(definition))
    }

    #[oai(path = "/", method = "put")]
    async fn create_or_update(
        &self,
        payload: Json<ApiDefinition>,
    ) -> Result<Json<ApiDefinition>, ApiEndpointError> {
        let api_definition_key = ApiDefinitionKey {
            id: payload.id.clone(),
            version: payload.version.clone(),
        };

        info!("Save API definition - id: {}", &api_definition_key.id);

        let definition: api_definition::ApiDefinition = payload
            .0
            .clone()
            .try_into()
            .map_err(ApiEndpointError::bad_request)?;

        register_api(self.definition_service.clone(), &definition).await?;

        let data = self
            .definition_service
            .get(&api_definition_key)
            .await
            .map_err(ApiEndpointError::internal)?;

        let definition = data.ok_or(ApiEndpointError::not_found("API Definition not found"))?;

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
        let api_definition_id_optional = api_definition_id_query.0;

        let api_version = api_definition_id_version.0;

        let api_definition_key = ApiDefinitionKey {
            id: api_definition_id_optional,
            version: api_version,
        };

        info!(
            "Get API definition - id: {}, version: {}",
            &api_definition_key.id, &api_definition_key.version
        );

        let data = self
            .definition_service
            .get(&api_definition_key)
            .await
            .map_err(ApiEndpointError::internal)?;

        let values: Vec<ApiDefinition> = match data {
            Some(d) => {
                let definition: ApiDefinition = d.try_into().map_err(ApiEndpointError::internal)?;
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

        let api_definition_key = ApiDefinitionKey {
            id: api_definition_id.clone(),
            version: api_definition_version,
        };

        info!("Delete API definition - id: {}", &api_definition_id);

        let data = self
            .definition_service
            .get(&api_definition_key)
            .await
            .map_err(ApiEndpointError::internal)?;

        if data.is_some() {
            self.definition_service
                .delete(&api_definition_key)
                .await
                .map_err(ApiEndpointError::internal)?;

            return Ok(Json("API definition deleted".to_string()));
        }

        Err(ApiEndpointError::not_found("API definition not found"))
    }

    #[oai(path = "/all", method = "get")]
    async fn get_all(&self) -> Result<Json<Vec<ApiDefinition>>, ApiEndpointError> {
        let data = self
            .definition_service
            .get_all()
            .await
            .map_err(ApiEndpointError::internal)?;

        let values = data
            .into_iter()
            .map(|d| d.try_into())
            .collect::<Result<Vec<ApiDefinition>, _>>()
            .map_err(ApiEndpointError::internal)?;

        Ok(Json(values))
    }
}

async fn register_api(
    definition_service: Arc<dyn RegisterApiDefinition + Sync + Send>,
    definition: &api_definition::ApiDefinition,
) -> Result<(), ApiEndpointError> {
    definition_service
        .register(definition)
        .await
        .map_err(|reg_error| {
            error!(
                "API definition id: {} - register error: {}",
                definition.id, reg_error
            );

            match reg_error {
                ApiRegistrationError::AlreadyExists(_) => {
                    ApiEndpointError::already_exists(reg_error)
                }
                ApiRegistrationError::InternalError(_) => ApiEndpointError::bad_request(reg_error),
            }
        })
}

#[cfg(test)]
mod test {
    use poem::test::TestClient;

    use crate::register::InMemoryRegistry;

    use super::*;

    fn make_route() -> poem::Route {
        let definition_service = Arc::new(InMemoryRegistry::default());
        let endpoint = RegisterApiDefinitionApi::new(definition_service);

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

// Mostly this data structures that represents the actual incoming request
// exist due to the presence of complicated Expr data type in api_definition::ApiDefinition.
// Consider them to be otherwise same
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
struct ApiDefinition {
    pub id: ApiDefinitionId,
    pub version: Version,
    pub routes: Vec<Route>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
struct Route {
    pub method: MethodPattern,
    pub path: String,
    pub binding: GolemWorkerBinding,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
struct GolemWorkerBinding {
    pub template: TemplateId,
    pub worker_id: serde_json::value::Value,
    pub function_name: String,
    pub function_params: Vec<serde_json::value::Value>,
    pub response: Option<ResponseMapping>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
struct ResponseMapping {
    pub body: serde_json::value::Value,
    // ${function.return}
    pub status: serde_json::value::Value,
    // "200" or if ${response.body.id == 1} "200" else "400"
    pub headers: HashMap<String, serde_json::value::Value>,
}

impl TryFrom<api_definition::ApiDefinition> for ApiDefinition {
    type Error = String;

    fn try_from(value: api_definition::ApiDefinition) -> Result<Self, Self::Error> {
        let mut routes = Vec::new();
        for route in value.routes {
            let v = Route::try_from(route)?;
            routes.push(v);
        }

        Ok(Self {
            id: value.id,
            version: value.version,
            routes,
        })
    }
}

impl TryInto<api_definition::ApiDefinition> for ApiDefinition {
    type Error = String;

    fn try_into(self) -> Result<api_definition::ApiDefinition, Self::Error> {
        let mut routes = Vec::new();

        for route in self.routes {
            let v = route.try_into()?;
            routes.push(v);
        }

        Ok(api_definition::ApiDefinition {
            id: self.id,
            version: self.version,
            routes,
        })
    }
}

impl TryFrom<api_definition::Route> for Route {
    type Error = String;

    fn try_from(value: api_definition::Route) -> Result<Self, Self::Error> {
        let path = value.path.to_string();
        let binding = GolemWorkerBinding::try_from(value.binding)?;

        Ok(Self {
            method: value.method,
            path,
            binding,
        })
    }
}

impl TryInto<api_definition::Route> for Route {
    type Error = String;

    fn try_into(self) -> Result<api_definition::Route, Self::Error> {
        let path =
            api_definition::PathPattern::from(self.path.as_str()).map_err(|e| e.to_string())?;
        let binding = self.binding.try_into()?;

        Ok(api_definition::Route {
            method: self.method,
            path,
            binding,
        })
    }
}

impl TryFrom<api_definition::ResponseMapping> for ResponseMapping {
    type Error = String;

    fn try_from(value: api_definition::ResponseMapping) -> Result<Self, Self::Error> {
        let body = serde_json::to_value(value.body).map_err(|e| e.to_string())?;
        let status = serde_json::to_value(value.status).map_err(|e| e.to_string())?;
        let mut headers = HashMap::new();
        for (key, value) in value.headers {
            let v = serde_json::to_value(value).map_err(|e| e.to_string())?;
            headers.insert(key.to_string(), v);
        }
        Ok(Self {
            body,
            status,
            headers,
        })
    }
}

impl TryInto<api_definition::ResponseMapping> for ResponseMapping {
    type Error = String;

    fn try_into(self) -> Result<api_definition::ResponseMapping, Self::Error> {
        let body: Expr = serde_json::from_value(self.body).map_err(|e| e.to_string())?;
        let status: Expr = serde_json::from_value(self.status).map_err(|e| e.to_string())?;
        let mut headers = HashMap::new();
        for (key, value) in self.headers {
            let v: Expr = serde_json::from_value(value).map_err(|e| e.to_string())?;
            headers.insert(key.to_string(), v);
        }

        Ok(api_definition::ResponseMapping {
            body,
            status,
            headers,
        })
    }
}

impl TryFrom<api_definition::GolemWorkerBinding> for GolemWorkerBinding {
    type Error = String;

    fn try_from(value: api_definition::GolemWorkerBinding) -> Result<Self, Self::Error> {
        let response: Option<ResponseMapping> = match value.response {
            Some(v) => {
                let r = ResponseMapping::try_from(v)?;
                Some(r)
            }
            None => None,
        };
        let worker_id = serde_json::to_value(value.worker_id).map_err(|e| e.to_string())?;
        let mut function_params = Vec::new();
        for param in value.function_params {
            let v = serde_json::to_value(param).map_err(|e| e.to_string())?;
            function_params.push(v);
        }

        Ok(Self {
            template: value.template,
            worker_id,
            function_name: value.function_name,
            function_params,
            response,
        })
    }
}

impl TryInto<api_definition::GolemWorkerBinding> for GolemWorkerBinding {
    type Error = String;

    fn try_into(self) -> Result<api_definition::GolemWorkerBinding, Self::Error> {
        let response: Option<api_definition::ResponseMapping> = match self.response {
            Some(v) => {
                let r: api_definition::ResponseMapping = v.try_into()?;
                Some(r)
            }
            None => None,
        };

        let worker_id: Expr = serde_json::from_value(self.worker_id).map_err(|e| e.to_string())?;
        let mut function_params = Vec::new();

        for param in self.function_params {
            let v: Expr = serde_json::from_value(param).map_err(|e| e.to_string())?;
            function_params.push(v);
        }

        Ok(api_definition::GolemWorkerBinding {
            template: self.template,
            worker_id,
            function_name: self.function_name,
            function_params,
            response,
        })
    }
}
