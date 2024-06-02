use async_trait::async_trait;
use golem_worker_service_base::api_definition::http::HttpApiDefinition;

use golem_worker_service_base::auth::CommonNamespace;
use golem_worker_service_base::http::InputHttpRequest;
use golem_worker_service_base::repo::api_definition_repo::ApiDefinitionRepo;
use golem_worker_service_base::service::api_definition_lookup::{
    ApiDefinitionLookup, ApiDefinitionLookupError,
};

use golem_worker_service_base::repo::api_deployment_repo::ApiDeploymentRepo;
use std::sync::Arc;
use tracing::error;

pub struct CustomRequestDefinitionLookupDefault {
    register_api_definition_repo:
        Arc<dyn ApiDefinitionRepo<CommonNamespace, HttpApiDefinition> + Sync + Send>,
    api_deployment_repo: Arc<dyn ApiDeploymentRepo<CommonNamespace> + Sync + Send>,
}

impl CustomRequestDefinitionLookupDefault {
    pub fn new(
        register_api_definition_repo: Arc<
            dyn ApiDefinitionRepo<CommonNamespace, HttpApiDefinition> + Sync + Send,
        >,
        api_deployment_repo: Arc<dyn ApiDeploymentRepo<CommonNamespace> + Sync + Send>,
    ) -> Self {
        Self {
            register_api_definition_repo,
            api_deployment_repo,
        }
    }
}

#[async_trait]
impl ApiDefinitionLookup<InputHttpRequest, HttpApiDefinition>
    for CustomRequestDefinitionLookupDefault
{
    async fn get(
        &self,
        input_http_request: InputHttpRequest,
    ) -> Result<HttpApiDefinition, ApiDefinitionLookupError> {
        // HOST should exist in Http Reequest
        let host = input_http_request
            .get_host()
            .ok_or(ApiDefinitionLookupError(
                "Host header not found".to_string(),
            ))?;

        let api_deployment = self
            .api_deployment_repo
            .get(&host)
            .await
            .map_err(|err| {
                error!("Error getting api deployment from the repo: {}", err);
                ApiDefinitionLookupError(format!(
                    "Error getting api deployment from the repo: {}",
                    err
                ))
            })?
            .ok_or(ApiDefinitionLookupError(format!(
                "Api deployment with host: {} not found",
                &host
            )))?;

        let api_key = api_deployment.api_definition_keys;

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
            &api_key.id, &api_key.version
        )))
    }
}
