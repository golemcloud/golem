use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::sync::Arc;

use async_trait::async_trait;

use crate::api_definition::http::HttpApiDefinition;
use golem_common::model::ComponentId;
use golem_service_base::model::Component;
use tracing::{error, info};

use crate::api_definition::{ApiDefinitionId, ApiVersion, HasGolemWorkerBindings};
use crate::repo::api_definition::ApiDefinitionRecord;
use crate::repo::api_definition::ApiDefinitionRepo;
use crate::repo::RepoError;

use super::api_definition_validator::{ApiDefinitionValidatorService, ValidationErrors};
use super::component::ComponentService;

pub type ApiResult<T, E> = Result<T, ApiDefinitionError<E>>;

#[derive(
    Eq, Hash, PartialEq, Clone, Debug, serde::Deserialize, bincode::Encode, bincode::Decode,
)]
pub struct ApiDefinitionIdWithVersion {
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiDefinitionError<E> {
    #[error(transparent)]
    ValidationError(#[from] ValidationErrors<E>),
    #[error("Unable to fetch component: {0:?}")]
    ComponentNotFoundError(Vec<ComponentId>),
    #[error("API definition not found: {0}")]
    ApiDefinitionNotFound(ApiDefinitionId),
    #[error("API definition is not draft: {0}")]
    ApiDefinitionNotDraft(ApiDefinitionId),
    #[error("API definition already exists: {0}")]
    ApiDefinitionAlreadyExists(ApiDefinitionId),
    #[error("Internal error: {0}")]
    InternalError(String),
}

impl<E> From<RepoError> for ApiDefinitionError<E> {
    fn from(error: RepoError) -> Self {
        match error {
            RepoError::Internal(e) => ApiDefinitionError::InternalError(e.clone()),
        }
    }
}

// A namespace here can be example: (account, project) etc.
// Ideally a repo service and its implementation with a different service impl that takes care of
// validations, authorisations etc is the right approach. However we are keeping it simple for now.
#[async_trait]
pub trait ApiDefinitionService<AuthCtx, Namespace, ValidationError> {
    async fn create(
        &self,
        definition: &HttpApiDefinition,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<ApiDefinitionId, ValidationError>;

    async fn update(
        &self,
        definition: &HttpApiDefinition,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<ApiDefinitionId, ValidationError>;

    async fn get(
        &self,
        id: &ApiDefinitionId,
        version: &ApiVersion,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<Option<HttpApiDefinition>, ValidationError>;

    async fn delete(
        &self,
        id: &ApiDefinitionId,
        version: &ApiVersion,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<Option<ApiDefinitionId>, ValidationError>;

    async fn get_all(
        &self,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<Vec<HttpApiDefinition>, ValidationError>;

    async fn get_all_versions(
        &self,
        id: &ApiDefinitionId,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<Vec<HttpApiDefinition>, ValidationError>;
}

pub struct ApiDefinitionServiceDefault<AuthCtx, ValidationError> {
    pub component_service: Arc<dyn ComponentService<AuthCtx> + Send + Sync>,
    pub definition_repo: Arc<dyn ApiDefinitionRepo + Sync + Send>,
    pub api_definition_validator:
        Arc<dyn ApiDefinitionValidatorService<HttpApiDefinition, ValidationError> + Sync + Send>,
}

impl<AuthCtx, ValidationError> ApiDefinitionServiceDefault<AuthCtx, ValidationError> {
    pub fn new(
        component_service: Arc<dyn ComponentService<AuthCtx> + Send + Sync>,
        definition_repo: Arc<dyn ApiDefinitionRepo + Sync + Send>,
        api_definition_validator: Arc<
            dyn ApiDefinitionValidatorService<HttpApiDefinition, ValidationError> + Sync + Send,
        >,
    ) -> Self {
        Self {
            component_service,
            definition_repo,
            api_definition_validator,
        }
    }

    async fn get_all_components(
        &self,
        definition: &HttpApiDefinition,
        auth_ctx: &AuthCtx,
    ) -> Result<Vec<Component>, ApiDefinitionError<ValidationError>> {
        let get_components = definition
            .get_golem_worker_bindings()
            .iter()
            .cloned()
            .map(|binding| async move {
                let id = &binding.component_id;
                self.component_service
                    .get_latest(id, auth_ctx)
                    .await
                    .map_err(|e| {
                        error!("Error getting latest component: {:?}", e);
                        id.clone()
                    })
            })
            .collect::<Vec<_>>();

        let components: Vec<Component> = {
            let results = futures::future::join_all(get_components).await;
            let (successes, errors) = results
                .into_iter()
                .partition::<Vec<_>, _>(|result| result.is_ok());

            // Ensure that all components were retrieved.
            if !errors.is_empty() {
                let errors: Vec<ComponentId> = errors.into_iter().map(|r| r.unwrap_err()).collect();
                return Err(ApiDefinitionError::ComponentNotFoundError(errors));
            }

            successes.into_iter().map(|r| r.unwrap()).collect()
        };

        Ok(components)
    }
}

#[async_trait]
impl<AuthCtx, Namespace, ValidationError> ApiDefinitionService<AuthCtx, Namespace, ValidationError>
    for ApiDefinitionServiceDefault<AuthCtx, ValidationError>
where
    AuthCtx: Send + Sync,
    Namespace: Display + Clone + Send + Sync,
{
    async fn create(
        &self,
        definition: &HttpApiDefinition,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<ApiDefinitionId, ValidationError> {
        info!(
            "Creating API definition - namespace: {}, id: {}, version: {}",
            namespace,
            definition.id.clone(),
            definition.version.clone()
        );

        let exists = self
            .definition_repo
            .get_draft(
                namespace.to_string().as_str(),
                definition.id.0.as_str(),
                definition.version.0.as_str(),
            )
            .await?;

        if exists.is_some() {
            return Err(ApiDefinitionError::ApiDefinitionAlreadyExists(
                definition.id.clone(),
            ));
        }

        let components = self.get_all_components(definition, auth_ctx).await?;

        self.api_definition_validator
            .validate(definition, components.as_slice())?;

        let record =
            ApiDefinitionRecord::new(namespace.clone(), definition.clone()).map_err(|_| {
                ApiDefinitionError::InternalError("Failed to convert record".to_string())
            })?;

        self.definition_repo.create(&record).await?;

        Ok(definition.id.clone())
    }

    async fn update(
        &self,
        definition: &HttpApiDefinition,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<ApiDefinitionId, ValidationError> {
        info!(
            "Updating API definition - namespace: {}, id: {}, version: {}",
            namespace,
            definition.id.clone(),
            definition.version.clone()
        );
        let draft = self
            .definition_repo
            .get_draft(
                namespace.to_string().as_str(),
                definition.id.0.as_str(),
                definition.version.0.as_str(),
            )
            .await?;

        match draft {
            Some(draft) if !draft => {
                return Err(ApiDefinitionError::ApiDefinitionNotDraft(
                    definition.id.clone(),
                ))
            }
            None => {
                return Err(ApiDefinitionError::ApiDefinitionNotFound(
                    definition.id.clone(),
                ))
            }
            _ => (),
        }

        let components = self.get_all_components(definition, auth_ctx).await?;

        self.api_definition_validator
            .validate(definition, components.as_slice())?;

        let record =
            ApiDefinitionRecord::new(namespace.clone(), definition.clone()).map_err(|_| {
                ApiDefinitionError::InternalError("Failed to convert record".to_string())
            })?;

        self.definition_repo.update(&record).await?;

        Ok(definition.id.clone())
    }

    async fn get(
        &self,
        id: &ApiDefinitionId,
        version: &ApiVersion,
        namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Option<HttpApiDefinition>, ValidationError> {
        info!(
            "Get API definition - namespace: {}, id: {}, version: {}",
            namespace, id, version
        );
        let value = self
            .definition_repo
            .get(&namespace.to_string(), id.0.as_str(), version.0.as_str())
            .await?;

        match value {
            Some(v) => {
                let definition = v.try_into().map_err(|_| {
                    ApiDefinitionError::InternalError("Failed to convert record".to_string())
                })?;
                Ok(Some(definition))
            }
            None => Ok(None),
        }
    }

    async fn delete(
        &self,
        id: &ApiDefinitionId,
        version: &ApiVersion,
        namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Option<ApiDefinitionId>, ValidationError> {
        info!(
            "Delete API definition - namespace: {}, id: {}, version: {}",
            namespace, id, version
        );
        let deleted = self
            .definition_repo
            .delete(&namespace.to_string(), id.0.as_str(), version.0.as_str())
            .await?;

        let value = if deleted { Some(id.clone()) } else { None };

        Ok(value)
    }

    async fn get_all(
        &self,
        namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Vec<HttpApiDefinition>, ValidationError> {
        info!("Get all API definitions - namespace: {}", namespace);
        let records = self.definition_repo.get_all(&namespace.to_string()).await?;

        let values: Vec<HttpApiDefinition> = records
            .iter()
            .map(|d| d.clone().try_into())
            .collect::<Result<Vec<HttpApiDefinition>, _>>()
            .map_err(|_| {
                ApiDefinitionError::InternalError("Failed to convert record".to_string())
            })?;

        Ok(values)
    }

    async fn get_all_versions(
        &self,
        id: &ApiDefinitionId,
        namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Vec<HttpApiDefinition>, ValidationError> {
        info!(
            "Get all API definitions versions - namespace: {}, id: {}",
            namespace, id
        );

        let records = self
            .definition_repo
            .get_all_versions(&namespace.to_string(), id.0.as_str())
            .await?;

        let values: Vec<HttpApiDefinition> = records
            .iter()
            .map(|d| d.clone().try_into())
            .collect::<Result<Vec<HttpApiDefinition>, _>>()
            .map_err(|_| {
                ApiDefinitionError::InternalError("Failed to convert record".to_string())
            })?;

        Ok(values)
    }
}

#[derive(Default)]
pub struct ApiDefinitionServiceNoop {}

#[async_trait]
impl<AuthCtx, Namespace, ValidationError> ApiDefinitionService<AuthCtx, Namespace, ValidationError>
    for ApiDefinitionServiceNoop
where
    AuthCtx: Send + Sync,
    Namespace: Display + Clone + Send + Sync,
{
    async fn create(
        &self,
        definition: &HttpApiDefinition,
        _namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<ApiDefinitionId, ValidationError> {
        Ok(definition.id.clone())
    }

    async fn update(
        &self,
        definition: &HttpApiDefinition,
        _namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<ApiDefinitionId, ValidationError> {
        Ok(definition.id.clone())
    }

    async fn get(
        &self,
        _id: &ApiDefinitionId,
        _version: &ApiVersion,
        _namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Option<HttpApiDefinition>, ValidationError> {
        Ok(None)
    }

    async fn delete(
        &self,
        _id: &ApiDefinitionId,
        _version: &ApiVersion,
        _namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Option<ApiDefinitionId>, ValidationError> {
        Ok(None)
    }

    async fn get_all(
        &self,
        _namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Vec<HttpApiDefinition>, ValidationError> {
        Ok(vec![])
    }

    async fn get_all_versions(
        &self,
        _id: &ApiDefinitionId,
        _namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Vec<HttpApiDefinition>, ValidationError> {
        Ok(vec![])
    }
}
