use crate::service::auth::CloudNamespace;
use async_trait::async_trait;
use golem_worker_service_base::api_definition::http::HttpApiDefinition;
use golem_worker_service_base::api_definition::ApiSiteString;
use golem_worker_service_base::http::InputHttpRequest;
use golem_worker_service_base::repo::api_definition_repo::ApiDefinitionRepo;
use golem_worker_service_base::service::api_definition::ApiDefinitionKey;
use golem_worker_service_base::service::api_definition_lookup::{
    ApiDefinitionLookup, ApiDefinitionLookupError,
};
use golem_worker_service_base::service::api_deployment::{
    ApiDeploymentError, ApiDeploymentService,
};
use http::header::HOST;
use std::sync::Arc;

pub struct CloudHttpRequestDefinitionLookup {
    deployment_service: Arc<dyn ApiDeploymentService<CloudNamespace> + Sync + Send>,
    definition_repo: Arc<dyn ApiDefinitionRepo<CloudNamespace, HttpApiDefinition> + Sync + Send>,
}

impl CloudHttpRequestDefinitionLookup {
    pub fn new(
        deployment_service: Arc<dyn ApiDeploymentService<CloudNamespace> + Sync + Send>,
        definition_repo: Arc<
            dyn ApiDefinitionRepo<CloudNamespace, HttpApiDefinition> + Sync + Send,
        >,
    ) -> Self {
        Self {
            deployment_service,
            definition_repo,
        }
    }
}

#[async_trait]
impl ApiDefinitionLookup<InputHttpRequest, HttpApiDefinition> for CloudHttpRequestDefinitionLookup {
    async fn get(
        &self,
        input_http_request: InputHttpRequest,
    ) -> Result<HttpApiDefinition, ApiDefinitionLookupError> {
        let host = match input_http_request
            .headers
            .get(HOST)
            .and_then(|h| h.to_str().ok())
        {
            Some(host) => Ok(host.to_string()),
            None => Err(ApiDefinitionLookupError(
                "Host header not found".to_string(),
            )),
        }?;

        let api_deployment = self
            .deployment_service
            .get_by_host(&ApiSiteString(host))
            .await
            .map_err(|error| {
                ApiDefinitionLookupError(format!(
                    "Error getting API Deployment {}",
                    print_api_deployment_error(error)
                ))
            })?;

        match api_deployment {
            Some(api_deployment) => self
                .definition_repo
                .get(&ApiDefinitionKey {
                    namespace: CloudNamespace {
                        project_id: api_deployment.api_definition_id.namespace.project_id,
                        account_id: api_deployment.api_definition_id.namespace.account_id,
                    },
                    id: api_deployment.api_definition_id.id,
                    version: api_deployment.api_definition_id.version,
                })
                .await
                .map_err(|e| {
                    ApiDefinitionLookupError(format!("Error getting API Definition: {}", e))
                })?
                .ok_or(ApiDefinitionLookupError(
                    "API Definition not found".to_string(),
                )),
            None => Err(ApiDefinitionLookupError(
                "API Deployment not found".to_string(),
            )),
        }
    }
}

// TODO: Implement the Display for ApiDeploymentError
pub fn print_api_deployment_error(error: ApiDeploymentError<CloudNamespace>) -> String {
    match error {
        ApiDeploymentError::ApiDefinitionNotFound(namespace, id) => {
            format!(
                "ApiDefinitionNotFound: namespace: {:?}, id: {:?}",
                namespace, id
            )
        }
        ApiDeploymentError::ApiDeploymentNotFound(namespace, site) => {
            format!(
                "ApiDeploymentNotFound: namespace: {:?}, site: {:?}",
                namespace, site
            )
        }
        ApiDeploymentError::InternalError(error) => {
            format!("InternalError: {:?}", error)
        }
        ApiDeploymentError::DeploymentConflict(conflict) => {
            format!("DeploymentConflict: {:?}", conflict)
        }
    }
}
