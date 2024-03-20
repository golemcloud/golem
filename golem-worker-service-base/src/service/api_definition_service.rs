use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::sync::Arc;

use crate::api_definition::{ApiDefinition, ApiDefinitionId, Version};
use crate::api_definition_repo::{ApiDefinitionRepo, ApiRegistrationRepoError};
use crate::auth::{AuthError, AuthService, CommonNamespace, EmptyAuthCtx, Permission};
use async_trait::async_trait;

use super::api_definition_validator::{ApiDefinitionValidatorService, ValidationError};

// A namespace here can be example: (account, project) etc.
// Ideally a repo service and its implementation with a different service impl that takes care of
// validations, authorisations etc is the right approach. However we are keeping it simple for now.
#[async_trait]
pub trait ApiDefinitionService<Namespace, AuthCtx> {
    async fn register(
        &self,
        definition: &ApiDefinition,
        auth_ctx: AuthCtx,
    ) -> Result<ApiDefinitionIdAnnotated<Namespace>, ApiRegistrationError>;

    async fn get(
        &self,
        api_definition_id: &ApiDefinitionId,
        version: &Version,
        auth_ctx: AuthCtx,
    ) -> Result<Option<ApiDefinitionAnnotated<Namespace>>, ApiRegistrationError>;

    async fn delete(
        &self,
        api_definition_id: &ApiDefinitionId,
        version: &Version,
        auth_ctx: AuthCtx,
    ) -> Result<Option<ApiDefinitionIdAnnotated<Namespace>>, ApiRegistrationError>;

    async fn get_all(
        &self,
        auth_ctx: AuthCtx,
    ) -> Result<Vec<ApiDefinitionAnnotated<Namespace>>, ApiRegistrationError>;

    async fn get_all_versions(
        &self,
        api_id: &ApiDefinitionId,
        auth_ctx: AuthCtx,
    ) -> Result<Vec<ApiDefinitionAnnotated<Namespace>>, ApiRegistrationError>;
}

pub struct ApiDefinitionIdAnnotated<Namespace> {
    pub namespace: Namespace,
    pub api_definition_id: ApiDefinitionId,
}

pub struct ApiDefinitionAnnotated<Namespace> {
    pub namespace: Namespace,
    pub api_definition: ApiDefinition,
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
    pub auth_service: Arc<dyn AuthService<AuthCtx, Namespace> + Sync + Send>,
    pub register_repo: Arc<dyn ApiDefinitionRepo<Namespace> + Sync + Send>,
    pub api_definition_validator: Arc<dyn ApiDefinitionValidatorService + Sync + Send>,
}

impl<Namespace, AuthCtx> RegisterApiDefinitionDefault<Namespace, AuthCtx> {
    pub fn new(
        auth_service: Arc<dyn AuthService<AuthCtx, Namespace> + Sync + Send>,
        register_repo: Arc<dyn ApiDefinitionRepo<Namespace> + Sync + Send>,
        api_definition_validator: Arc<dyn ApiDefinitionValidatorService + Sync + Send>,
    ) -> Self {
        Self {
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

    pub async fn register_api(
        &self,
        api_definition: &ApiDefinition,
        key: &ApiDefinitionKey<Namespace>,
    ) -> Result<(), ApiRegistrationError> {
        Ok(self.register_repo.register(api_definition, key).await?)
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
    ) -> Result<ApiDefinitionIdAnnotated<Namespace>, ApiRegistrationError> {
        let namespace = self.is_authorized(Permission::Create, &auth_ctx).await?;

        self.api_definition_validator.validate(definition).await?;

        let key = ApiDefinitionKey {
            namespace: namespace.clone(),
            id: definition.id.clone(),
            version: definition.version.clone(),
        };

        self.register_repo.register(definition, &key).await?;

        let result = ApiDefinitionIdAnnotated {
            namespace,
            api_definition_id: key.id,
        };

        Ok(result)
    }

    async fn get(
        &self,
        api_definition_id: &ApiDefinitionId,
        version: &Version,
        auth_ctx: AuthCtx,
    ) -> Result<Option<ApiDefinitionAnnotated<Namespace>>, ApiRegistrationError> {
        let namespace = self.is_authorized(Permission::View, &auth_ctx).await?;

        let key = ApiDefinitionKey {
            namespace: namespace.clone(),
            id: api_definition_id.clone(),
            version: version.clone(),
        };

        let api_definition = self.register_repo.get(&key).await?;

        let result = api_definition.map(|api_definition| ApiDefinitionAnnotated {
            namespace,
            api_definition,
        });

        Ok(result)
    }

    async fn delete(
        &self,
        api_definition_id: &ApiDefinitionId,
        version: &Version,
        auth_ctx: AuthCtx,
    ) -> Result<Option<ApiDefinitionIdAnnotated<Namespace>>, ApiRegistrationError> {
        let namespace = self.is_authorized(Permission::Delete, &auth_ctx).await?;

        let key = ApiDefinitionKey {
            namespace: namespace.clone(),
            id: api_definition_id.clone(),
            version: version.clone(),
        };

        let deleted = self.register_repo.delete(&key).await?;

        let result = if deleted {
            Some(ApiDefinitionIdAnnotated {
                namespace,
                api_definition_id: key.id,
            })
        } else {
            None
        };

        Ok(result)
    }

    async fn get_all(
        &self,
        auth_ctx: AuthCtx,
    ) -> Result<Vec<ApiDefinitionAnnotated<Namespace>>, ApiRegistrationError> {
        let namespace = self.is_authorized(Permission::View, &auth_ctx).await?;

        let definitions = self.register_repo.get_all(&namespace).await?;

        let result = definitions
            .into_iter()
            .map(|definition| ApiDefinitionAnnotated {
                namespace: namespace.clone(),
                api_definition: definition.clone(),
            })
            .collect();

        Ok(result)
    }

    async fn get_all_versions(
        &self,
        api_id: &ApiDefinitionId,
        auth_ctx: AuthCtx,
    ) -> Result<Vec<ApiDefinitionAnnotated<Namespace>>, ApiRegistrationError> {
        let namespace = self.is_authorized(Permission::View, &auth_ctx).await?;

        let definitions = self
            .register_repo
            .get_all_versions(api_id, &namespace)
            .await?;

        let result = definitions
            .into_iter()
            .map(|definition| ApiDefinitionAnnotated {
                namespace: namespace.clone(),
                api_definition: definition,
            })
            .collect();

        Ok(result)
    }
}

pub struct RegisterApiDefinitionNoop {}

#[async_trait]
impl ApiDefinitionService<CommonNamespace, EmptyAuthCtx> for RegisterApiDefinitionNoop {
    async fn register(
        &self,
        api_definition: &ApiDefinition,
        _auth_ctx: EmptyAuthCtx,
    ) -> Result<ApiDefinitionIdAnnotated<CommonNamespace>, ApiRegistrationError> {
        Ok(ApiDefinitionIdAnnotated {
            namespace: CommonNamespace::default(),
            api_definition_id: api_definition.id.clone(),
        })
    }

    async fn get(
        &self,
        _api_definition_id: &ApiDefinitionId,
        _version: &Version,
        _auth_ctx: EmptyAuthCtx,
    ) -> Result<Option<ApiDefinitionAnnotated<CommonNamespace>>, ApiRegistrationError> {
        Ok(None)
    }

    async fn delete(
        &self,
        _api_definition_id: &ApiDefinitionId,
        _version: &Version,
        _auth_ctx: EmptyAuthCtx,
    ) -> Result<Option<ApiDefinitionIdAnnotated<CommonNamespace>>, ApiRegistrationError> {
        Ok(None)
    }

    async fn get_all(
        &self,
        _auth_ctx: EmptyAuthCtx,
    ) -> Result<Vec<ApiDefinitionAnnotated<CommonNamespace>>, ApiRegistrationError> {
        Ok(vec![])
    }

    async fn get_all_versions(
        &self,
        _api_id: &ApiDefinitionId,
        _auth_ctx: EmptyAuthCtx,
    ) -> Result<Vec<ApiDefinitionAnnotated<CommonNamespace>>, ApiRegistrationError> {
        Ok(vec![])
    }
}
