use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::sync::Arc;

use async_trait::async_trait;

use golem_common::model::ComponentId;
use golem_service_base::model::Component;

use crate::api_definition::{
    ApiDefinitionId, ApiVersion, HasApiDefinitionId, HasGolemWorkerBindings, HasVersion,
};
use crate::repo::api_definition_repo::{ApiDefinitionRepo, ApiRegistrationRepoError};
use crate::repo::api_namespace::ApiNamespace;

use super::api_definition_validator::{ApiDefinitionValidatorService, ValidationErrors};
use super::component::ComponentService;

pub type ApiResult<T, E> = Result<T, ApiRegistrationError<E>>;

// A namespace here can be example: (account, project) etc.
// Ideally a repo service and its implementation with a different service impl that takes care of
// validations, authorisations etc is the right approach. However we are keeping it simple for now.
#[async_trait]
pub trait ApiDefinitionService<AuthCtx, Namespace, ApiDefinition, ValidationError> {
    async fn create(
        &self,
        definition: &ApiDefinition,
        namespace: Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<ApiDefinitionId, ValidationError>;

    async fn update(
        &self,
        definition: &ApiDefinition,
        namespace: Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<ApiDefinitionId, ValidationError>;

    async fn get(
        &self,
        api_definition_id: &ApiDefinitionId,
        version: &ApiVersion,
        namespace: Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<Option<ApiDefinition>, ValidationError>;

    async fn delete(
        &self,
        api_definition_id: &ApiDefinitionId,
        version: &ApiVersion,
        namespace: Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<Option<ApiDefinitionId>, ValidationError>;

    async fn get_all(
        &self,
        namespace: Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<Vec<ApiDefinition>, ValidationError>;

    async fn get_all_versions(
        &self,
        api_id: &ApiDefinitionId,
        namespace: Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<Vec<ApiDefinition>, ValidationError>;
}

// An ApiDefinitionKey is just the original ApiDefinitionId with additional information of version and a possibility of namespace.
// A namespace here can be for example: account, project, production, dev or a composite value, or infact as simple
// as a constant string or unit.
// A namespace is not pre-tied to any other parts of original ApiDefinitionId to keep the ApiDefinition part simple, reusable.
#[derive(
    Eq, Hash, PartialEq, Clone, Debug, serde::Deserialize, bincode::Encode, bincode::Decode,
)]
pub struct ApiDefinitionKey<Namespace> {
    pub namespace: Namespace,
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
}

impl<Namespace: Display> ApiDefinitionKey<Namespace> {
    pub fn displayed(&self) -> ApiDefinitionKey<String> {
        ApiDefinitionKey {
            namespace: self.namespace.to_string(),
            id: self.id.clone(),
            version: self.version.clone(),
        }
    }
}

impl<Namespace: Display> Display for ApiDefinitionKey<Namespace> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{namespace}::{id}::{version}",
            namespace = self.namespace,
            id = self.id,
            version = self.version
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ApiRegistrationError<E> {
    #[error(transparent)]
    RepoError(#[from] ApiRegistrationRepoError),
    #[error(transparent)]
    ValidationError(#[from] ValidationErrors<E>),
    #[error("Unable to fetch component: {0:?}")]
    ComponentNotFoundError(Vec<ComponentId>),
}

pub struct ApiDefinitionServiceDefault<AuthCtx, Namespace, ApiDefinition, ValidationError> {
    pub component_service: Arc<dyn ComponentService<AuthCtx> + Send + Sync>,
    pub register_repo: Arc<dyn ApiDefinitionRepo<Namespace, ApiDefinition> + Sync + Send>,
    pub api_definition_validator:
        Arc<dyn ApiDefinitionValidatorService<ApiDefinition, ValidationError> + Sync + Send>,
}

impl<AuthCtx, Namespace, ApiDefinition, ValidationError>
    ApiDefinitionServiceDefault<AuthCtx, Namespace, ApiDefinition, ValidationError>
where
    Namespace: ApiNamespace + Send + Sync,
    ApiDefinition: GolemApiDefinition + Sync,
{
    pub fn new(
        component_service: Arc<dyn ComponentService<AuthCtx> + Send + Sync>,
        register_repo: Arc<dyn ApiDefinitionRepo<Namespace, ApiDefinition> + Sync + Send>,
        api_definition_validator: Arc<
            dyn ApiDefinitionValidatorService<ApiDefinition, ValidationError> + Sync + Send,
        >,
    ) -> Self {
        Self {
            component_service,
            register_repo,
            api_definition_validator,
        }
    }

    async fn get_all_components(
        &self,
        definition: &ApiDefinition,
        auth_ctx: &AuthCtx,
    ) -> Result<Vec<Component>, ApiRegistrationError<ValidationError>> {
        let get_components = definition
            .get_golem_worker_bindings()
            .iter()
            .cloned()
            .map(|binding| async move {
                let id = &binding.component;
                self.component_service
                    .get_latest(id, auth_ctx)
                    .await
                    .map_err(|e| {
                        tracing::error!("Error getting latest component: {:?}", e);
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
                return Err(ApiRegistrationError::ComponentNotFoundError(errors));
            }

            successes.into_iter().map(|r| r.unwrap()).collect()
        };

        Ok(components)
    }
}

pub trait GolemApiDefinition: HasGolemWorkerBindings + HasApiDefinitionId + HasVersion {}

impl<T: HasGolemWorkerBindings + HasApiDefinitionId + HasVersion> GolemApiDefinition for T {}

#[async_trait]
impl<AuthCtx, Namespace, ApiDefinition, ValidationError>
    ApiDefinitionService<AuthCtx, Namespace, ApiDefinition, ValidationError>
    for ApiDefinitionServiceDefault<AuthCtx, Namespace, ApiDefinition, ValidationError>
where
    AuthCtx: Send + Sync,
    Namespace: ApiNamespace + Send + Sync,
    ApiDefinition: GolemApiDefinition + Sync,
{
    async fn create(
        &self,
        definition: &ApiDefinition,
        namespace: Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<ApiDefinitionId, ValidationError> {
        let components = self.get_all_components(definition, auth_ctx).await?;

        self.api_definition_validator
            .validate(definition, components.as_slice())?;

        let key = ApiDefinitionKey {
            namespace,
            id: definition.get_api_definition_id().clone(),
            version: definition.get_version().clone(),
        };

        self.register_repo.create(definition, &key).await?;

        Ok(key.id)
    }

    async fn update(
        &self,
        definition: &ApiDefinition,
        namespace: Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<ApiDefinitionId, ValidationError> {
        let components = self.get_all_components(definition, auth_ctx).await?;

        self.api_definition_validator
            .validate(definition, components.as_slice())?;

        let key = ApiDefinitionKey {
            namespace,
            id: definition.get_api_definition_id().clone(),
            version: definition.get_version().clone(),
        };

        self.register_repo.update(definition, &key).await?;

        Ok(key.id)
    }

    async fn get(
        &self,
        api_definition_id: &ApiDefinitionId,
        version: &ApiVersion,
        namespace: Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Option<ApiDefinition>, ValidationError> {
        let key = ApiDefinitionKey {
            namespace,
            id: api_definition_id.clone(),
            version: version.clone(),
        };

        let value = self.register_repo.get(&key).await?;

        Ok(value)
    }

    async fn delete(
        &self,
        api_definition_id: &ApiDefinitionId,
        version: &ApiVersion,
        namespace: Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Option<ApiDefinitionId>, ValidationError> {
        let key = ApiDefinitionKey {
            namespace: namespace.clone(),
            id: api_definition_id.clone(),
            version: version.clone(),
        };

        let deleted = self.register_repo.delete(&key).await?;

        let value = if deleted { Some(key.id) } else { None };

        Ok(value)
    }

    async fn get_all(
        &self,
        namespace: Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Vec<ApiDefinition>, ValidationError> {
        let value = self.register_repo.get_all(&namespace).await?;
        Ok(value)
    }

    async fn get_all_versions(
        &self,
        api_id: &ApiDefinitionId,
        namespace: Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Vec<ApiDefinition>, ValidationError> {
        let value = self
            .register_repo
            .get_all_versions(api_id, &namespace)
            .await?;

        Ok(value)
    }
}

pub struct RegisterApiDefinitionNoop {}

#[async_trait]
impl<AuthCtx, Namespace, ApiDefinition, ValidationError>
    ApiDefinitionService<AuthCtx, Namespace, ApiDefinition, ValidationError>
    for RegisterApiDefinitionNoop
where
    Namespace: Default + Send + Sync + 'static,
{
    async fn create(
        &self,
        _definition: &ApiDefinition,
        _namespace: Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<ApiDefinitionId, ValidationError> {
        Ok(ApiDefinitionId("noop".to_string()))
    }

    async fn update(
        &self,
        _definition: &ApiDefinition,
        _namespace: Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<ApiDefinitionId, ValidationError> {
        Ok(ApiDefinitionId("noop".to_string()))
    }

    async fn get(
        &self,
        _api_definition_id: &ApiDefinitionId,
        _version: &ApiVersion,
        _namespace: Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Option<ApiDefinition>, ValidationError> {
        Ok(None)
    }

    async fn delete(
        &self,
        _api_definition_id: &ApiDefinitionId,
        _version: &ApiVersion,
        _namespace: Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Option<ApiDefinitionId>, ValidationError> {
        Ok(None)
    }

    async fn get_all(
        &self,
        _namespace: Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Vec<ApiDefinition>, ValidationError> {
        Ok(vec![])
    }

    async fn get_all_versions(
        &self,
        _api_id: &ApiDefinitionId,
        _namespace: Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Vec<ApiDefinition>, ValidationError> {
        Ok(vec![])
    }
}
