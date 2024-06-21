use async_trait::async_trait;
use golem_worker_service_base::api_definition::http::HttpApiDefinition;

use golem_worker_service_base::auth::{CommonNamespace, EmptyAuthCtx, EmptyNamespace};
use golem_worker_service_base::http::InputHttpRequest;
use golem_worker_service_base::repo::api_definition_repo::ApiDefinitionRepo;
use golem_worker_service_base::service::api_definition_lookup::{
    ApiDefinitionLookupError, ApiDefinitionsLookup,
};

use golem_worker_service_base::repo::api_deployment_repo::ApiDeploymentRepo;
use golem_worker_service_base::service::api_definition::{ApiDefinitionKey, ApiDefinitionService2};
use golem_worker_service_base::service::api_deployment::ApiDeploymentService;
use golem_worker_service_base::service::http::http_api_definition_validator::RouteValidationError;
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
impl ApiDefinitionsLookup<InputHttpRequest, HttpApiDefinition>
    for CustomRequestDefinitionLookupDefault
{
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

        let mut http_api_defs = vec![];

        for api_defs in api_deployment.api_definition_keys {
            let api_key = ApiDefinitionKey {
                namespace: api_deployment.namespace.clone(),
                id: api_defs.id.clone(),
                version: api_defs.version.clone(),
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

            let api_definition = value.ok_or(ApiDefinitionLookupError(format!(
                "Api definition with id: {} and version: {} not found",
                &api_key.id, &api_key.version
            )))?;

            http_api_defs.push(api_definition);
        }

        Ok(http_api_defs)
    }
}

pub struct CustomRequestDefinitionLookup {
    definition_service: Arc<
        dyn ApiDefinitionService2<EmptyAuthCtx, EmptyNamespace, RouteValidationError> + Sync + Send,
    >,
    deployment_service: Arc<dyn ApiDeploymentService<EmptyNamespace> + Sync + Send>,
}

impl CustomRequestDefinitionLookup {
    pub fn new(
        definition_service: Arc<
            dyn ApiDefinitionService2<EmptyAuthCtx, EmptyNamespace, RouteValidationError>
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
            .get_by_host(&host)
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
                    &EmptyAuthCtx {},
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
