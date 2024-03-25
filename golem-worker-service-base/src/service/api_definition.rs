use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::sync::Arc;

use crate::api_definition::{ApiDefinition, ApiDefinitionId, Version};
use crate::api_definition_repo::{ApiDefinitionRepo, ApiRegistrationRepoError};
use crate::auth::{CommonNamespace, EmptyAuthCtx};
use async_trait::async_trait;
use golem_service_base::model::Template;
use golem_service_base::service::auth::{AuthError, AuthService, Permission, WithNamespace};

use super::api_definition_validator::{ApiDefinitionValidatorService, ValidationError};
use super::template::TemplateService;

pub type ApiResult<T, Namespace> = Result<WithNamespace<T, Namespace>, ApiRegistrationError>;

// A namespace here can be example: (account, project) etc.
// Ideally a repo service and its implementation with a different service impl that takes care of
// validations, authorisations etc is the right approach. However we are keeping it simple for now.
#[async_trait]
pub trait ApiDefinitionService<Namespace, AuthCtx> {
    async fn register(
        &self,
        definition: &ApiDefinition,
        auth_ctx: AuthCtx,
    ) -> ApiResult<ApiDefinitionId, Namespace>;

    async fn get(
        &self,
        api_definition_id: &ApiDefinitionId,
        version: &Version,
        auth_ctx: AuthCtx,
    ) -> ApiResult<Option<ApiDefinition>, Namespace>;

    async fn delete(
        &self,
        api_definition_id: &ApiDefinitionId,
        version: &Version,
        auth_ctx: AuthCtx,
    ) -> ApiResult<Option<ApiDefinitionId>, Namespace>;

    async fn get_all(&self, auth_ctx: AuthCtx) -> ApiResult<Vec<ApiDefinition>, Namespace>;

    async fn get_all_versions(
        &self,
        api_id: &ApiDefinitionId,
        auth_ctx: AuthCtx,
    ) -> ApiResult<Vec<ApiDefinition>, Namespace>;
}

pub trait ApiNamespace:
    Eq
    + Hash
    + PartialEq
    + Clone
    + Debug
    + Display
    + Send
    + Sync
    + bincode::Encode
    + bincode::Decode
    + serde::de::DeserializeOwned
{
}
impl<
        T: Eq
            + Hash
            + PartialEq
            + Clone
            + Debug
            + Display
            + Send
            + Sync
            + bincode::Encode
            + bincode::Decode
            + serde::de::DeserializeOwned,
    > ApiNamespace for T
{
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
    pub version: Version,
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

#[derive(Debug, Clone, thiserror::Error)]
pub enum ApiRegistrationError {
    #[error(transparent)]
    AuthenticationError(#[from] AuthError),
    #[error(transparent)]
    RepoError(#[from] ApiRegistrationRepoError),
    #[error(transparent)]
    ValidationError(#[from] ValidationError),
}

pub struct RegisterApiDefinitionDefault<Namespace, AuthCtx> {
    pub template_service: Arc<dyn TemplateService<AuthCtx, Namespace> + Send + Sync>,
    pub auth_service: Arc<dyn AuthService<AuthCtx, Namespace> + Sync + Send>,
    pub register_repo: Arc<dyn ApiDefinitionRepo<Namespace> + Sync + Send>,
    pub api_definition_validator: Arc<dyn ApiDefinitionValidatorService + Sync + Send>,
}

impl<Namespace, AuthCtx> RegisterApiDefinitionDefault<Namespace, AuthCtx> {
    pub fn new(
        template_service: Arc<dyn TemplateService<AuthCtx, Namespace> + Send + Sync>,
        auth_service: Arc<dyn AuthService<AuthCtx, Namespace> + Sync + Send>,
        register_repo: Arc<dyn ApiDefinitionRepo<Namespace> + Sync + Send>,
        api_definition_validator: Arc<dyn ApiDefinitionValidatorService + Sync + Send>,
    ) -> Self {
        Self {
            template_service,
            auth_service,
            register_repo,
            api_definition_validator,
        }
    }
}

impl<Namespace: ApiNamespace, AuthCtx> RegisterApiDefinitionDefault<Namespace, AuthCtx> {
    pub async fn is_authorized(
        &self,
        permission: Permission,
        auth_ctx: &AuthCtx,
    ) -> Result<Namespace, ApiRegistrationError> {
        Ok(self
            .auth_service
            .is_authorized(permission, auth_ctx)
            .await?)
    }

    async fn get_all_templates(
        &self,
        definition: &ApiDefinition,
        auth_ctx: &AuthCtx,
    ) -> Result<Vec<Template>, ApiRegistrationError> {
        let get_templates = definition
            .routes
            .iter()
            .cloned()
            .map(|route| (route.binding.template.clone(), route))
            .collect::<HashMap<_, _>>()
            .into_values()
            .map(|route| async move {
                let id = &route.binding.template;
                self.template_service
                    .get_latest(id, auth_ctx)
                    .await
                    .map_err(|e| {
                        tracing::error!("Error getting latest template: {:?}", e);
                        // TODO: Better error message.
                        crate::service::api_definition_validator::RouteValidationError::from_route(
                            route,
                            "Error getting latest template".into(),
                        )
                    })
            })
            .collect::<Vec<_>>();

        let templates: Vec<Template> = {
            let results = futures::future::join_all(get_templates).await;
            let (successes, errors) = results
                .into_iter()
                .partition::<Vec<_>, _>(|result| result.is_ok());

            // Ensure that all templates were retrieved.
            if !errors.is_empty() {
                let errors = errors.into_iter().map(|r| r.unwrap_err()).collect();
                return Err(ValidationError { errors }.into());
            }

            successes
                .into_iter()
                .map(|r| r.unwrap())
                .map(|t| t.value)
                .collect()
        };

        Ok(templates)
    }
}

#[async_trait]
impl<Namespace: ApiNamespace, AuthCtx: Send + Sync> ApiDefinitionService<Namespace, AuthCtx>
    for RegisterApiDefinitionDefault<Namespace, AuthCtx>
{
    async fn register(
        &self,
        definition: &ApiDefinition,
        auth_ctx: AuthCtx,
    ) -> ApiResult<ApiDefinitionId, Namespace> {
        let namespace = self.is_authorized(Permission::Create, &auth_ctx).await?;

        let templates = self.get_all_templates(definition, &auth_ctx).await?;

        self.api_definition_validator
            .validate(definition, templates.as_slice())?;

        let key = ApiDefinitionKey {
            namespace: namespace.clone(),
            id: definition.id.clone(),
            version: definition.version.clone(),
        };

        self.register_repo.register(definition, &key).await?;

        Ok(WithNamespace {
            value: key.id,
            namespace,
        })
    }

    async fn get(
        &self,
        api_definition_id: &ApiDefinitionId,
        version: &Version,
        auth_ctx: AuthCtx,
    ) -> ApiResult<Option<ApiDefinition>, Namespace> {
        let namespace = self.is_authorized(Permission::View, &auth_ctx).await?;

        let key = ApiDefinitionKey {
            namespace: namespace.clone(),
            id: api_definition_id.clone(),
            version: version.clone(),
        };

        let value = self.register_repo.get(&key).await?;

        Ok(WithNamespace { value, namespace })
    }

    async fn delete(
        &self,
        api_definition_id: &ApiDefinitionId,
        version: &Version,
        auth_ctx: AuthCtx,
    ) -> ApiResult<Option<ApiDefinitionId>, Namespace> {
        let namespace = self.is_authorized(Permission::Delete, &auth_ctx).await?;

        let key = ApiDefinitionKey {
            namespace: namespace.clone(),
            id: api_definition_id.clone(),
            version: version.clone(),
        };

        let deleted = self.register_repo.delete(&key).await?;

        let value = if deleted { Some(key.id) } else { None };

        Ok(WithNamespace { value, namespace })
    }

    async fn get_all(&self, auth_ctx: AuthCtx) -> ApiResult<Vec<ApiDefinition>, Namespace> {
        let namespace = self.is_authorized(Permission::View, &auth_ctx).await?;
        let value = self.register_repo.get_all(&namespace).await?;
        Ok(WithNamespace { value, namespace })
    }

    async fn get_all_versions(
        &self,
        api_id: &ApiDefinitionId,
        auth_ctx: AuthCtx,
    ) -> ApiResult<Vec<ApiDefinition>, Namespace> {
        let namespace = self.is_authorized(Permission::View, &auth_ctx).await?;

        let value = self
            .register_repo
            .get_all_versions(api_id, &namespace)
            .await?;

        Ok(WithNamespace { value, namespace })
    }
}

pub struct RegisterApiDefinitionNoop {}

#[async_trait]
impl ApiDefinitionService<CommonNamespace, EmptyAuthCtx> for RegisterApiDefinitionNoop {
    async fn register(
        &self,
        api_definition: &ApiDefinition,
        _auth_ctx: EmptyAuthCtx,
    ) -> ApiResult<ApiDefinitionId, CommonNamespace> {
        Ok(WithNamespace {
            value: api_definition.id.clone(),
            namespace: Default::default(),
        })
    }

    async fn get(
        &self,
        _api_definition_id: &ApiDefinitionId,
        _version: &Version,
        _auth_ctx: EmptyAuthCtx,
    ) -> ApiResult<Option<ApiDefinition>, CommonNamespace> {
        Ok(WithNamespace {
            value: None,
            namespace: Default::default(),
        })
    }

    async fn delete(
        &self,
        _api_definition_id: &ApiDefinitionId,
        _version: &Version,
        _auth_ctx: EmptyAuthCtx,
    ) -> ApiResult<Option<ApiDefinitionId>, CommonNamespace> {
        Ok(WithNamespace {
            value: None,
            namespace: Default::default(),
        })
    }

    async fn get_all(
        &self,
        _auth_ctx: EmptyAuthCtx,
    ) -> ApiResult<Vec<ApiDefinition>, CommonNamespace> {
        Ok(WithNamespace {
            value: vec![],
            namespace: Default::default(),
        })
    }

    async fn get_all_versions(
        &self,
        _api_id: &ApiDefinitionId,
        _auth_ctx: EmptyAuthCtx,
    ) -> ApiResult<Vec<ApiDefinition>, CommonNamespace> {
        Ok(WithNamespace {
            value: vec![],
            namespace: Default::default(),
        })
    }
}
