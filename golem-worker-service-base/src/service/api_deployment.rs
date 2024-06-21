use crate::api_definition::{ApiDefinitionId, ApiDeployment, ApiSite, ApiSiteString};

use std::collections::HashSet;

use async_trait::async_trait;

use std::sync::Arc;
use tracing::log::error;

use crate::api_definition::http::{AllPathPatterns, HttpApiDefinition, Route};
use crate::http::router::{Router, RouterPattern};
use crate::repo::api_deployment::ApiDeploymentRecord;
use crate::repo::RepoError;
use crate::service::api_definition::ApiDefinitionIdWithVersion;
use std::fmt::Display;

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

#[derive(Debug, thiserror::Error)]
pub enum ApiDeploymentError<Namespace> {
    #[error("Api definition not found: {1}")]
    ApiDefinitionNotFound(Namespace, ApiDefinitionId),
    #[error("Api deployment not found: {1}")]
    ApiDeploymentNotFound(Namespace, ApiSiteString),
    #[error("Internal error: {0}")]
    InternalError(String),
    #[error("Api deployment conflict error: {0}")]
    DeploymentConflict(ApiSiteString),
    #[error("Api definitions conflict: {0:?}")]
    ConflictingDefinitions(Vec<String>),
}

impl<Namespace> From<RepoError> for ApiDeploymentError<Namespace> {
    fn from(error: RepoError) -> Self {
        match error {
            RepoError::Internal(e) => ApiDeploymentError::InternalError(e.clone()),
        }
    }
}

pub trait ConflictChecker {
    type Entity: Display + Send;
    fn find_conflicts(input: &[Self]) -> Vec<Self::Entity>
    where
        Self: Sized;
}

impl ConflictChecker for HttpApiDefinition {
    type Entity = AllPathPatterns;
    fn find_conflicts(definitions: &[Self]) -> Vec<Self::Entity> {
        let routes = definitions
            .iter()
            .flat_map(|def| def.routes.clone())
            .collect::<Vec<_>>();

        let mut router = Router::<Route>::new();

        let mut conflicting_path_patterns = vec![];

        for route in routes {
            let method: hyper::Method = route.clone().method.into();
            let path = route
                .clone()
                .path
                .path_patterns
                .iter()
                .map(|pattern| RouterPattern::from(pattern.clone()))
                .collect::<Vec<_>>();

            if !router.add_route(method.clone(), path.clone(), route) {
                let current_route = router.get_route(&method, &path).unwrap();

                conflicting_path_patterns.push(current_route.path.clone());
            }
        }

        conflicting_path_patterns
    }
}

pub struct ApiDeploymentServiceDefault {
    pub deployment_repo: Arc<dyn crate::repo::api_deployment::ApiDeploymentRepo + Sync + Send>,
    pub definition_repo: Arc<dyn crate::repo::api_definition::ApiDefinitionRepo + Sync + Send>,
}

impl ApiDeploymentServiceDefault {
    pub fn new(
        deployment_repo: Arc<dyn crate::repo::api_deployment::ApiDeploymentRepo + Sync + Send>,
        definition_repo: Arc<dyn crate::repo::api_definition::ApiDefinitionRepo + Sync + Send>,
    ) -> Self {
        Self {
            deployment_repo,
            definition_repo,
        }
    }
}

#[async_trait]
impl<Namespace: Display + TryFrom<String> + Eq + Clone + Send + Sync>
    ApiDeploymentService<Namespace> for ApiDeploymentServiceDefault
{
    async fn deploy(
        &self,
        deployment: &ApiDeployment<Namespace>,
    ) -> Result<(), ApiDeploymentError<Namespace>> {
        // Existing deployment
        let existing_deployment_records = self
            .deployment_repo
            .get_by_site(deployment.site.to_string().as_str())
            .await?;

        let mut existing_api_definition_keys: HashSet<ApiDefinitionIdWithVersion> = HashSet::new();

        for deployment_record in existing_deployment_records {
            if deployment_record.namespace != deployment.namespace.to_string()
                || deployment_record.subdomain != deployment.site.subdomain
                || deployment_record.host != deployment.site.host
            {
                error!(
                         "Failed to deploy api-definition of namespace {} with site: {} - site used by another API (under another namespace/API)",
                        &deployment.namespace,
                        &deployment.site,
                    );
                return Err(ApiDeploymentError::DeploymentConflict(ApiSiteString::from(
                    &ApiSite {
                        host: deployment_record.host,
                        subdomain: deployment_record.subdomain,
                    },
                )));
            }

            existing_api_definition_keys.insert(ApiDefinitionIdWithVersion {
                id: deployment_record.definition_id.into(),
                version: deployment_record.definition_version.into(),
            });
        }

        let mut new_deployment_records: Vec<ApiDeploymentRecord> = vec![];

        let mut set_not_draft: Vec<ApiDefinitionIdWithVersion> = vec![];

        for api_definition_key in deployment.api_definition_keys.clone() {
            if !existing_api_definition_keys.contains(&api_definition_key) {
                let draft = self
                    .definition_repo
                    .get_draft(
                        deployment.namespace.to_string().as_str(),
                        api_definition_key.id.0.as_str(),
                        api_definition_key.version.0.as_str(),
                    )
                    .await?;

                if let Some(draft) = draft {
                    if draft {
                        set_not_draft.push(api_definition_key.clone());
                    }
                } else {
                    return Err(ApiDeploymentError::ApiDefinitionNotFound(
                        deployment.namespace.clone(),
                        api_definition_key.id.clone(),
                    ));
                }

                new_deployment_records.push(ApiDeploymentRecord {
                    namespace: deployment.namespace.to_string(),
                    subdomain: deployment.site.subdomain.clone(),
                    host: deployment.site.host.clone(),
                    definition_id: api_definition_key.id.0.clone(),
                    definition_version: api_definition_key.version.0.clone(),
                });
            }
        }

        if !new_deployment_records.is_empty() {
            for api_definition_key in set_not_draft {
                self.definition_repo
                    .set_not_draft(
                        deployment.namespace.to_string().as_str(),
                        api_definition_key.id.0.as_str(),
                        api_definition_key.version.0.as_str(),
                    )
                    .await?;
            }

            self.deployment_repo.create(new_deployment_records).await?;
        }

        Ok(())
    }

    async fn get_by_id(
        &self,
        namespace: &Namespace,
        definition_id: &ApiDefinitionId,
    ) -> Result<Vec<ApiDeployment<Namespace>>, ApiDeploymentError<Namespace>> {
        let existing_deployment_records = self
            .deployment_repo
            .get_by_id(namespace.to_string().as_str(), definition_id.0.as_str())
            .await?;

        let mut values: Vec<ApiDeployment<Namespace>> = vec![];

        for deployment_record in existing_deployment_records {
            let site = ApiSite {
                host: deployment_record.host,
                subdomain: deployment_record.subdomain,
            };

            let namespace: Namespace = deployment_record.namespace.try_into().map_err(|_| {
                ApiDeploymentError::InternalError("Failed to convert namespace".to_string())
            })?;

            let api_definition_key = ApiDefinitionIdWithVersion {
                id: deployment_record.definition_id.into(),
                version: deployment_record.definition_version.into(),
            };

            match values
                .iter_mut()
                .find(|val| val.site == site && val.namespace == namespace)
            {
                Some(val) => {
                    val.api_definition_keys.push(api_definition_key);
                }
                None => {
                    values.push(ApiDeployment {
                        site,
                        namespace,
                        api_definition_keys: vec![api_definition_key],
                    });
                }
            }
        }

        Ok(values)
    }

    async fn get_by_host(
        &self,
        site: &ApiSiteString,
    ) -> Result<Option<ApiDeployment<Namespace>>, ApiDeploymentError<Namespace>> {
        let existing_deployment_records = self
            .deployment_repo
            .get_by_site(site.to_string().as_str())
            .await?;

        let mut api_definition_keys: Vec<ApiDefinitionIdWithVersion> = vec![];

        let mut site: Option<ApiSite> = None;

        let mut namespace: Option<Namespace> = None;

        for deployment_record in existing_deployment_records {
            if site.is_none() {
                site = Some(ApiSite {
                    host: deployment_record.host,
                    subdomain: deployment_record.subdomain,
                });
            }

            if namespace.is_none() {
                namespace = Some(deployment_record.namespace.try_into().map_err(|_| {
                    ApiDeploymentError::InternalError("Failed to convert namespace".to_string())
                })?);
            }

            api_definition_keys.push(ApiDefinitionIdWithVersion {
                id: deployment_record.definition_id.into(),
                version: deployment_record.definition_version.into(),
            });
        }

        match (site, namespace) {
            (Some(site), Some(namespace)) => Ok(Some(ApiDeployment {
                namespace,
                site,
                api_definition_keys,
            })),
            _ => Ok(None),
        }
    }

    async fn delete(
        &self,
        namespace: &Namespace,
        site: &ApiSiteString,
    ) -> Result<bool, ApiDeploymentError<Namespace>> {
        let existing_deployment_records = self
            .deployment_repo
            .get_by_site(site.to_string().as_str())
            .await?;

        if existing_deployment_records.is_empty() {
            Err(ApiDeploymentError::ApiDeploymentNotFound(
                namespace.clone(),
                site.clone(),
            ))
        } else if existing_deployment_records
            .iter()
            .any(|value| value.namespace != namespace.to_string())
        {
            error!(
                         "Failed to delete api deployment of namespace {} with site: {} - site used by another API (under another namespace/API)",
                        namespace,
                        &site,
                    );
            Err(ApiDeploymentError::DeploymentConflict(site.clone()))
        } else {
            self.deployment_repo
                .delete(existing_deployment_records)
                .await
                .map_err(|e| e.into())
        }
    }
}

pub struct ApiDeploymentServiceNoop {}

#[async_trait]
impl<Namespace: Display + TryFrom<String> + Eq + Clone + Send + Sync>
    ApiDeploymentService<Namespace> for ApiDeploymentServiceNoop
{
    async fn deploy(
        &self,
        _deployment: &ApiDeployment<Namespace>,
    ) -> Result<(), ApiDeploymentError<Namespace>> {
        todo!()
    }

    async fn get_by_id(
        &self,
        _namespace: &Namespace,
        _api_definition_id: &ApiDefinitionId,
    ) -> Result<Vec<ApiDeployment<Namespace>>, ApiDeploymentError<Namespace>> {
        todo!()
    }

    async fn get_by_host(
        &self,
        _host: &ApiSiteString,
    ) -> Result<Option<ApiDeployment<Namespace>>, ApiDeploymentError<Namespace>> {
        todo!()
    }

    async fn delete(
        &self,
        _namespace: &Namespace,
        _host: &ApiSiteString,
    ) -> Result<bool, ApiDeploymentError<Namespace>> {
        todo!()
    }
}
