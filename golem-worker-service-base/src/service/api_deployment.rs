use crate::api_definition::{ApiDefinitionId, ApiDeployment, ApiVersion, Host};
use crate::repo::api_definition_repo::ApiDefinitionRepo;
use crate::repo::api_deployment_repo::ApiDeploymentRepo;
use crate::repo::api_namespace::ApiNamespace;
use crate::service::api_definition::{ApiDefinitionKey, ApiDefinitionService};
use crate::service::api_definition_validator::ApiDefinitionValidatorService;
use crate::service::template::TemplateService;
use async_trait::async_trait;
use std::error::Error;
use std::sync::Arc;
use tracing::log::error;

#[async_trait]
pub trait ApiDeploymentService<Namespace> {
    async fn deploy(
        &self,
        deployment: &ApiDeployment<Namespace>,
    ) -> Result<(), ApiDeploymentError<Namespace>>;

    // Example: A newer version of API definition is in dev site, and older version of the same definition-id is in prod site.
    // Therefore Vec<ApiDeployment>
    async fn get_by_id(
        &self,
        namespace: &Namespace,
        api_definition_id: &ApiDefinitionId,
    ) -> Result<Vec<ApiDeployment<Namespace>>, ApiDeploymentError<Namespace>>;

    // Example: A version of API definition can only be utmost 1 deployment
    async fn get_by_id_and_version(
        &self,
        namespace: &Namespace,
        api_definition_id: &ApiDefinitionId,
        version: &ApiVersion,
    ) -> Result<Option<ApiDeployment<Namespace>>, ApiDeploymentError<Namespace>>;

    async fn delete(
        &self,
        namespace: &Namespace,
        host: &Host,
    ) -> Result<bool, ApiDeploymentError<Namespace>>;
}

enum ApiDeploymentError<Namespace> {
    ApiDefinitionNotFound(Namespace, ApiDefinitionId),
    ApiDeploymentNotFound(Namespace, Host),
    InternalError(String),
    DeploymentConflict(Host),
    ValidationError(String),
}

pub struct ApiDeploymentServiceDefault<Namespace, ApiDefinition> {
    pub deployment_repo: Arc<dyn ApiDeploymentRepo<Namespace> + Sync + Send>,
    pub definition_repo: Arc<dyn ApiDefinitionRepo<Namespace, ApiDefinition> + Sync + Send>,
}

impl<Namespace, ApiDefinition> ApiDeploymentServiceDefault<Namespace, ApiDefinition> {
    pub fn new(
        deployment_repo: Arc<dyn ApiDeploymentRepo<Namespace> + Sync + Send>,
        definition_repo: Arc<dyn ApiDefinitionRepo<Namespace, ApiDefinition> + Sync + Send>,
    ) -> Self {
        Self {
            deployment_repo,
            definition_repo,
        }
    }
}

#[async_trait]
impl<Namespace: ApiNamespace, ApiDefinition> ApiDeploymentService<Namespace>
    for ApiDeploymentServiceDefault<Namespace, ApiDefinition>
{
    async fn deploy(
        &self,
        deployment: &ApiDeployment<Namespace>,
    ) -> Result<(), ApiDeploymentError<Namespace>> {
        let api_definition_key = deployment.api_definition_id.clone();

        if let Ok(None) = self
            .definition_repo
            .get(&api_definition_key)
            .await
            .map_err(|err| {
                ApiDeploymentError::<Namespace>::InternalError(format!(
                    "Error getting api definition: {}",
                    err
                ))
            })
        {
            return Err(ApiDeploymentError::ApiDefinitionNotFound(
                api_definition_key.namespace,
                api_definition_key.id,
            ));
        }

        let existing_deployment =
            self.deployment_repo
                .get(&deployment.site)
                .await
                .map_err(|err| {
                    ApiDeploymentError::InternalError(format!(
                        "Error getting api deployment: {}",
                        err
                    ))
                })?;

        match existing_deployment {
            Some(existing_deployment)
                if existing_deployment.api_definition_id.namespace
                    != deployment.api_definition_id.namespace =>
            {
                error!(
                        "Failed to deploy api-definition of namespace {} with site: {} - site used by another API (under another namespace/API)",
                        &deployment.api_definition_id.namespace,
                        &deployment.site,
                );
                Err(ApiDeploymentError::DeploymentConflict(
                    existing_deployment.site,
                ))
            }
            _ => self
                .deployment_repo
                .deploy(deployment)
                .await
                .map_err(|err| {
                    ApiDeploymentError::InternalError(format!(
                        "Error deploying api deployment: {}",
                        err
                    ))
                }),
        }
    }

    async fn get_by_id(
        &self,
        namespace: &Namespace,
        api_definition_id: &ApiDefinitionId,
    ) -> Result<Vec<ApiDeployment<Namespace>>, ApiDeploymentError<Namespace>> {
        self.deployment_repo
            .get_by_id(namespace, api_definition_id)
            .await
            .map_err(|err| {
                ApiDeploymentError::InternalError(format!(
                    "Error getting api deployment by id: {}",
                    err
                ))
            })
    }

    async fn get_by_id_and_version(
        &self,
        namespace: &Namespace,
        api_definition_id: &ApiDefinitionId,
        version: &ApiVersion,
    ) -> Result<Option<ApiDeployment<Namespace>>, ApiDeploymentError<Namespace>> {
        let api_deployments = self
            .deployment_repo
            .get_by_id(namespace, api_definition_id)
            .await
            .map_err(|err| {
                ApiDeploymentError::InternalError(format!(
                    "Error getting api deployment by id and version: {}",
                    err
                ))
            })?;

        // Finding if any of the api_deployments match the input version
        api_deployments
            .into_iter()
            .find(|api_deployment| api_deployment.api_definition_id.version == *version)
            .map_or(Ok(None), |api_deployment| Ok(Some(api_deployment)))
    }

    async fn delete(
        &self,
        namespace: &Namespace,
        host: &Host,
    ) -> Result<bool, ApiDeploymentError<Namespace>> {
        let deployment = self.deployment_repo.get(host).await.map_err(|err| {
            ApiDeploymentError::InternalError(format!("Error getting api deployment: {}", err))
        })?;

        match deployment {
            Some(deployment) if deployment.api_definition_id.namespace != *namespace => {
                error!(
                        "Failed to delete api deployment of namespace {} with site: {} - site used by another API (under another namespace/API)",
                        namespace,
                        &host,
                );
                Err(ApiDeploymentError::DeploymentConflict(deployment.site))
            }
            Some(_) => self.deployment_repo.delete(&host).await.map_err(|err| {
                ApiDeploymentError::InternalError(format!("Error deleting api deployment: {}", err))
            }),
            None => Err(ApiDeploymentError::ApiDeploymentNotFound(
                namespace.clone(),
                host.clone(),
            )),
        }
    }
}
