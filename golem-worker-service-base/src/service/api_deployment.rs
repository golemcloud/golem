use crate::api_definition::{
    ApiDefinitionId, ApiDeployment, ApiSiteString, ApiVersion, HasIsDraft,
};
use crate::repo::api_definition_repo::ApiDefinitionRepo;
use crate::repo::api_deployment_repo::{ApiDeploymentRepo, ApiDeploymentRepoError};
use crate::repo::api_namespace::ApiNamespace;

use async_trait::async_trait;

use std::sync::Arc;
use tracing::log::error;
use crate::service::api_definition::ApiDefinitionKey;

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

    async fn get_by_host(
        &self,
        host: &ApiSiteString,
    ) -> Result<Option<ApiDeployment<Namespace>>, ApiDeploymentError<Namespace>>;

    async fn delete(
        &self,
        namespace: &Namespace,
        host: &ApiSiteString,
    ) -> Result<bool, ApiDeploymentError<Namespace>>;
}

pub enum ApiDeploymentError<Namespace> {
    ApiDefinitionNotFound(Namespace, ApiDefinitionId),
    ApiDeploymentNotFound(Namespace, ApiSiteString),
    InternalError(String),
    DeploymentConflict(ApiSiteString),
}

impl<Namespace> From<ApiDeploymentRepoError> for ApiDeploymentError<Namespace> {
    fn from(error: ApiDeploymentRepoError) -> Self {
        match error {
            ApiDeploymentRepoError::Internal(e) => ApiDeploymentError::InternalError(e.to_string()),
        }
    }
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
impl<Namespace: ApiNamespace, ApiDefinition: HasIsDraft + Send> ApiDeploymentService<Namespace>
    for ApiDeploymentServiceDefault<Namespace, ApiDefinition>
{
    async fn deploy(
        &self,
        deployment: &ApiDeployment<Namespace>,
    ) -> Result<(), ApiDeploymentError<Namespace>> {
        let api_definition_keys = deployment.api_definition_keys.clone();

        let mut api_definitions = vec![];

        for definition_key in api_definition_keys {
            let api_definition_key = ApiDefinitionKey {
                namespace: deployment.namespace.clone(),
                id: definition_key.id.clone(),
                version: definition_key.version.clone(),
            };

            let definition = self
                .definition_repo
                .get(&api_definition_key)
                .await
                .map_err(|err| {
                    ApiDeploymentError::<Namespace>::InternalError(format!(
                        "Error getting api definition: {}",
                        err
                    ))
                })?
                .ok_or(ApiDeploymentError::ApiDefinitionNotFound(
                    deployment.namespace.clone(),
                    definition_key.id.clone(),
                ))?;
            api_definitions.push((api_definition_key, definition))
        }

        let existing_deployment = self
            .deployment_repo
            .get(&ApiSiteString::from(&deployment.site))
            .await?;

        match existing_deployment {
            Some(existing_deployment) =>
            {
                let existing_namespace = existing_deployment.namespace;

                let new_deployment_namespace =
                    deployment.namespace.clone();

                if existing_namespace != new_deployment_namespace {
                    error!(
                         "Failed to deploy api-definition of namespace {} with site: {} - site used by another API (under another namespace/API)",
                        &new_deployment_namespace,
                        &deployment.site,
                    );
                    Err(ApiDeploymentError::DeploymentConflict(ApiSiteString::from(
                        &existing_deployment.site,
                    )))
                } else {
                    internal::deploy(api_definitions, deployment, self.definition_repo.clone(), self.deployment_repo.clone()).await
                }
            }
            None => {
                internal::deploy(api_definitions, deployment, self.definition_repo.clone(), self.deployment_repo.clone()).await
            }
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
            .map_err(|err| err.into())
    }

    async fn get_by_host(
        &self,
        host: &ApiSiteString,
    ) -> Result<Option<ApiDeployment<Namespace>>, ApiDeploymentError<Namespace>> {
        self.deployment_repo
            .get(host)
            .await
            .map_err(|err| err.into())
    }

    async fn delete(
        &self,
        namespace: &Namespace,
        host: &ApiSiteString,
    ) -> Result<bool, ApiDeploymentError<Namespace>> {
        let deployment = self.deployment_repo.get(host).await?;


        match deployment {
            Some(deployment) if deployment.namespace != *namespace => {
                error!(
                        "Failed to delete api deployment of namespace {} with site: {} - site used by another API (under another namespace/API)",
                        namespace,
                        &host,
                );
                Err(ApiDeploymentError::DeploymentConflict(ApiSiteString::from(
                    &deployment.site,
                )))
            }
            Some(_) => self
                .deployment_repo
                .delete(host)
                .await
                .map_err(|err| err.into()),
            None => Err(ApiDeploymentError::ApiDeploymentNotFound(
                namespace.clone(),
                host.clone(),
            )),
        }
    }
}

mod internal {
    use std::sync::Arc;
    use crate::api_definition::{ApiDeployment, HasIsDraft};
    use crate::repo::api_definition_repo::ApiDefinitionRepo;
    use crate::repo::api_deployment_repo::ApiDeploymentRepo;
    use crate::repo::api_namespace::ApiNamespace;
    use crate::service::api_definition::ApiDefinitionKey;
    use crate::service::api_deployment::ApiDeploymentError;

    pub(crate) async fn deploy<Namespace: ApiNamespace, ApiDefinition: HasIsDraft + Send>(
        api_definitions: Vec<(ApiDefinitionKey<Namespace>, ApiDefinition)>,
        deployment: &ApiDeployment<Namespace>,
        definition_repo: Arc<dyn ApiDefinitionRepo<Namespace, ApiDefinition> + Sync + Send>,
        deployment_repo: Arc<dyn ApiDeploymentRepo<Namespace> + Sync + Send>,
    ) -> Result<(), ApiDeploymentError<Namespace>> {
        for (key, definition) in api_definitions {
            if definition.is_draft() {
                definition_repo
                    .set_not_draft(&key)
                    .await
                    .map_err(|err| {
                        ApiDeploymentError::<Namespace>::InternalError(format!(
                            "Error freezing api definition: {}",
                            err
                        ))
                    })?;
            }
        }

        deployment_repo
            .deploy(deployment)
            .await
            .map_err(|err| err.into())
    }
}
