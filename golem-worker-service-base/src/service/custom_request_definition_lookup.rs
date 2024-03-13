use std::fmt::Display;
use crate::api_definition::{ApiDefinition, ApiDefinitionId, Version};
use crate::api_definition_repo::ApiDefinitionRepo;
use crate::auth::{CommonNamespace};
use crate::http_request::InputHttpRequest;
use crate::oas_worker_bridge::{GOLEM_API_DEFINITION_ID_EXTENSION, GOLEM_API_DEFINITION_VERSION};
use crate::service::register_definition::{ApiDefinitionKey};
use async_trait::async_trait;
use http::HeaderMap;
use std::sync::Arc;
use tracing::error;

#[async_trait]
pub trait CustomRequestDefinitionLookup {
    async fn get(
        &self,
        input_http_request: InputHttpRequest<'_>,
    ) -> Result<ApiDefinition, ApiDefinitionLookupError>;
}

struct ApiDefinitionLookupError(String);

impl Display for ApiDefinitionLookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ApiDefinitionLookupError: {}", self.0)
    }
}

pub struct CustomRequestDefinitionLookupDefault {
    register_api_definition_repo: Arc<dyn ApiDefinitionRepo<CommonNamespace> + Sync + Send>,
}

impl CustomRequestDefinitionLookupDefault {
    pub fn new(
        register_api_definition_repo: Arc<dyn ApiDefinitionRepo<CommonNamespace> + Sync + Send>,
    ) -> Self {
        Self {
            register_api_definition_repo,
        }
    }
}

#[async_trait]
impl CustomRequestDefinitionLookup for CustomRequestDefinitionLookupDefault {
    async fn get(
        &self,
        input_http_request: InputHttpRequest<'_>,
    ) -> Result<ApiDefinition, ApiDefinitionLookupError> {
        let api_definition_id = match get_header_value(
            &input_http_request.headers,
            GOLEM_API_DEFINITION_ID_EXTENSION,
        ) {
            Ok(api_definition_id) => Ok(ApiDefinitionId(api_definition_id.to_string())),
            Err(err) => Err(ApiDefinitionLookupError(format!(
                "{} not found in the request headers. Error: {}",
                GOLEM_API_DEFINITION_ID_EXTENSION, err
            ))),
        }?;

        let version =
            match get_header_value(&input_http_request.headers, GOLEM_API_DEFINITION_VERSION) {
                Ok(version) => Ok(Version(version)),
                Err(err) => Err(ApiDefinitionLookupError(format!(
                    "{} not found in the request headers. Error: {}",
                    GOLEM_API_DEFINITION_VERSION, err
                ))),
            }?;

        let api_key = ApiDefinitionKey {
            namespace: CommonNamespace::default(),
            id: api_definition_id.clone(),
            version: version.clone(),
        };

        let value = self
            .register_api_definition_repo
            .get(&api_key)
            .await
            .map_err(|err| {
                error!("Error getting api definition from the repo: {}", err);
                ApiDefinitionLookupError(format!(
                    "Error getting api definition from the repo: {}",
                    err
                ))
            })?;

        value.ok_or(ApiDefinitionLookupError(format!(
            "Api definition with id: {} and version: {} not found",
            &api_definition_id, &version
        )))
    }
}

fn get_header_value(headers: &HeaderMap, header_name: &str) -> Result<String, String> {
    let header_value = headers
        .iter()
        .find(|(key, _)| key.as_str().to_lowercase() == header_name)
        .map(|(_, value)| value)
        .ok_or(format!("Missing {} header", header_name))?;

    header_value
        .to_str()
        .map(|x| x.to_string())
        .map_err(|e| format!("Invalid value for the header {} error: {}", header_name, e))
}
