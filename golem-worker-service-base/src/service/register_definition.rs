use std::fmt::Display;

use crate::api_definition::{ApiDefinition, ApiDefinitionId, Version};
use async_trait::async_trait;
use crate::register::ApiRegistrationRepoError;

// A namespace here can be example: (account, project) etc.
// Ideally a repo service and its implementation with a different service impl that takes care of
// validations, authorisations etc is the right approach. However we are keeping it simple for now.
#[async_trait]
pub trait RegisterApiDefinition<Namespace> {
    async fn register(
        &self,
        definition: &ApiDefinition,
        api_definition_key: &ApiDefinitionKey<Namespace>,
    ) -> Result<(), ApiRegistrationError<Namespace>>;

    async fn get(
        &self,
        api_definition_key: &ApiDefinitionKey<Namespace>,
        namespace: &Namespace,
    ) -> Result<Option<ApiDefinition>, ApiRegistrationError<Namespace>>;

    async fn delete(
        &self,
        api_definition_key: &ApiDefinitionKey<Namespace>,
        namespace: &Namespace,
    ) -> Result<bool, ApiRegistrationError<Namespace>>;

    async fn get_all(
        &self,
        namespace: &Namespace,
    ) -> Result<Vec<ApiDefinition>, ApiRegistrationError<Namespace>>;

    async fn get_all_versions(
        &self,
        api_id: &ApiDefinitionId,
        namespace: &Namespace,
    ) -> Result<Vec<ApiDefinition>, ApiRegistrationError<Namespace>>;
}

// An ApiDefinitionKey is just the original ApiDefinitionId with additional information of version and a possibility of namespace.
// A namespace here can be for example: account, project, production, dev or a composite value, or infact as simple
// as a constant string or unit.
// A namespace is not pre-tied to any other parts of original ApiDefinitionId to keep the ApiDefinition part simple, reusable.
#[derive(Debug, Clone, Eq, Hash, PartialEq)]
pub struct ApiDefinitionKey<Namespace> {
    pub namespace: Namespace,
    pub id: ApiDefinitionId,
    pub version: Version,
}


impl<Namespace: Display> Display for ApiDefinitionKey<Namespace> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.namespace, self.id, self.version.0)
    }
}

impl<Namespace: TryFrom<&str>> TryFrom<&str> for ApiDefinitionKey<Namespace> {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let parts: Vec<&str> = value.split(':').collect();
        if parts.len() != 3 {
            Err(format!("Invalid ApiDefinitionKey string: {}", value))
        } else {
            Ok(ApiDefinitionKey {
                namespace: Namespace::try_from(parts[0]),
                id: ApiDefinitionId(parts[1].to_string()),
                version: Version(parts[2].to_string()),
            })
        }
    }
}

#[derive(Debug, Clone)]
pub enum ApiRegistrationError<Namespace> {
    AlreadyExists(ApiDefinitionKey<Namespace>),
    InternalError(String),
}

impl<Namespace> From<ApiRegistrationRepoError<Namespace>> for ApiRegistrationError<Namespace> {
    fn from(value: ApiRegistrationRepoError<Namespace>) -> Self {
        match value {
            ApiRegistrationRepoError::InternalError(error) => ApiRegistrationError::InternalError(error),
            ApiRegistrationError::AlreadyExists(key) => ApiRegistrationError::AlreadyExists(key)
        }
    }
}

impl<Namespace: Display> Display for ApiRegistrationError<Namespace> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiRegistrationError::InternalError(msg) => write!(f, "InternalError: {}", msg),
            ApiRegistrationError::AlreadyExists(api_definition_key) => {
                write!(
                    f,
                    "AlreadyExists: ApiDefinition with id: {} and version:{} already exists in the namespace {}",
                    api_definition_key.id, api_definition_key.version.0, api_definition_key.namespace
                )
            }
        }
    }
}
