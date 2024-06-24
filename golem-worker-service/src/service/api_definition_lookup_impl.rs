use async_trait::async_trait;
use golem_worker_service_base::api_definition::http::HttpApiDefinition;

use golem_worker_service_base::auth::{EmptyAuthCtx, EmptyNamespace};
use golem_worker_service_base::http::InputHttpRequest;
use golem_worker_service_base::service::api_definition::ApiDefinitionService;
use golem_worker_service_base::service::api_definition_lookup::{
    ApiDefinitionLookupError, ApiDefinitionsLookup,
};
use golem_worker_service_base::service::api_deployment::ApiDeploymentService;
use golem_worker_service_base::service::http::http_api_definition_validator::RouteValidationError;
use std::sync::Arc;
use tracing::error;

pub struct CustomRequestDefinitionLookup {
    definition_service: Arc<
        dyn ApiDefinitionService<EmptyAuthCtx, EmptyNamespace, RouteValidationError> + Sync + Send,
    >,
    deployment_service: Arc<dyn ApiDeploymentService<EmptyNamespace> + Sync + Send>,
}

impl CustomRequestDefinitionLookup {
    pub fn new(
        definition_service: Arc<
            dyn ApiDefinitionService<EmptyAuthCtx, EmptyNamespace, RouteValidationError>
                + Sync
                + Send,
        >,
        deployment_service: Arc<dyn ApiDeploymentService<EmptyNamespace> + Sync + Send>,
    ) -> Self {
        Self {
            definition_service,
            deployment_service,
        }
    }
}

#[async_trait]
impl ApiDefinitionsLookup<InputHttpRequest, HttpApiDefinition> for CustomRequestDefinitionLookup {
    async fn get(
        &self,
        input_http_request: InputHttpRequest,
    ) -> Result<Vec<HttpApiDefinition>, ApiDefinitionLookupError> {
        // HOST should exist in Http Request
        let host = input_http_request
            .get_host()
            .ok_or(ApiDefinitionLookupError(
                "Host header not found".to_string(),
            ))?;

        let api_deployment = self
            .deployment_service
            .get_by_site(&host)
            .await
            .map_err(|err| {
                error!("Error getting api deployment from the repo: {:?}", err);
                ApiDefinitionLookupError(format!(
                    "Error getting api deployment from the repo: {:?}",
                    err
                ))
            })?
            .ok_or(ApiDefinitionLookupError(format!(
                "Api deployment with host: {} not found",
                &host
            )))?;

        let mut http_api_defs = vec![];

        for api_def in api_deployment.api_definition_keys {
            let value = self
                .definition_service
                .get(
                    &api_def.id,
                    &api_def.version,
                    &api_deployment.namespace,
                    &EmptyAuthCtx::default(),
                )
                .await
                .map_err(|err| {
                    error!("Error getting api definition from the repo: {}", err);
                    ApiDefinitionLookupError(format!(
                        "Error getting api definition from the repo: {}",
                        err
                    ))
                })?;

            let api_definition = value.ok_or(ApiDefinitionLookupError(format!(
                "Api definition with id: {} and version: {} not found",
                &api_def.id, &api_def.version
            )))?;

            http_api_defs.push(api_definition);
        }

        Ok(http_api_defs)
    }
}
