use std::collections::HashMap;
use std::fmt::Display;
use std::sync::{Arc, Mutex};

use crate::api_definition::{ApiDefinition, ApiDefinitionId, Version};
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::config::RedisConfig;
use golem_common::redis::RedisPool;
use tracing::{debug, info};
use crate::auth::AuthService;

const API_DEFINITION_REDIS_NAMESPACE: &str = "apidefinition";

// A namespace here can be example: (account, project) etc.
#[async_trait]
pub trait RegisterApiDefinition<Namespace> {
    async fn register(&self, definition: &ApiDefinition, namespace: &Namespace) -> Result<(), ApiRegistrationError>;

    async fn get(
        &self,
        api_definition_key: &ApiDefinitionKey,
        namespace: &Namespace,
    ) -> Result<Option<ApiDefinition>, ApiRegistrationError>;

    async fn delete(
        &self,
        api_definition_key: &ApiDefinitionKey,
        namespace: &Namespace,
    ) -> Result<bool, ApiRegistrationError>;

    async fn get_all(&self, namespace: &Namespace) -> Result<Vec<ApiDefinition>, ApiRegistrationError>;

    async fn get_all_versions(
        &self,
        api_id: &ApiDefinitionId,
        namespace: &Namespace,
    ) -> Result<Vec<ApiDefinition>, ApiRegistrationError>;
}

#[derive(Debug, Clone, Eq, Hash, PartialEq)]
pub struct ApiDefinitionKey {
    pub id: ApiDefinitionId,
    pub version: Version,
}

impl ApiDefinitionKey {
    /// Specific Api Definition Key.
    fn get_api_definition_redis_key(&self) -> String {
        format!(
            "{API_DEFINITION_REDIS_NAMESPACE}:definition:{}:{}",
            self.id, self.version
        )
    }

    /// Value for the [`Self::get_all_apis_set_redis_key`] set.
    fn make_all_apis_redis_value(&self) -> String {
        format!("{}:{}", self.id.0, self.version.0)
    }

    /// Set containing all Api Definition Keys.
    /// Values should only be added by [`Self::make_all_apis_redis_value`].
    fn get_all_apis_set_redis_key() -> String {
        format!("{API_DEFINITION_REDIS_NAMESPACE}:definition:all_api_definitions")
    }
}

impl Display for ApiDefinitionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.id, self.version.0)
    }
}

impl From<&ApiDefinition> for ApiDefinitionKey {
    fn from(api_definition: &ApiDefinition) -> ApiDefinitionKey {
        ApiDefinitionKey {
            id: api_definition.id.clone(),
            version: api_definition.version.clone(),
        }
    }
}

impl TryFrom<&str> for ApiDefinitionKey {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let parts: Vec<&str> = value.split(':').collect();
        if parts.len() != 2 {
            Err(format!("Invalid ApiDefinitionKey string: {}", value))
        } else {
            Ok(ApiDefinitionKey {
                id: ApiDefinitionId(parts[0].to_string()),
                version: Version(parts[1].to_string()),
            })
        }
    }
}

#[derive(Debug, Clone)]
pub enum ApiRegistrationError {
    AlreadyExists(ApiDefinitionKey),
    InternalError(String),
}

impl Display for ApiRegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiRegistrationError::InternalError(msg) => write!(f, "InternalError: {}", msg),
            ApiRegistrationError::AlreadyExists(api_definition_key) => {
                write!(
                    f,
                    "AlreadyExists: ApiDefinition with id: {} and version:{} already exists",
                    api_definition_key.id, api_definition_key.version.0
                )
            }
        }
    }
}

pub struct InMemoryRegistry {
    registry: Mutex<HashMap<ApiDefinitionKey, ApiDefinition>>,
}

impl Default for InMemoryRegistry {
    fn default() -> Self {
        InMemoryRegistry {
            registry: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl RegisterApiDefinition<()> for InMemoryRegistry {
    async fn register(&self, definition: &ApiDefinition, _namespace: &()) -> Result<(), ApiRegistrationError> {
        let mut registry = self.registry.lock().unwrap();
        let key: ApiDefinitionKey = ApiDefinitionKey::from(definition);

        if let std::collections::hash_map::Entry::Vacant(e) = registry.entry(key.clone()) {
            e.insert(definition.clone());
            Ok(())
        } else {
            Err(ApiRegistrationError::AlreadyExists(key.clone()))
        }
    }

    async fn get(
        &self,
        api_id: &ApiDefinitionKey,
        _namespace: &()
    ) -> Result<Option<ApiDefinition>, ApiRegistrationError> {
        let registry = self.registry.lock().unwrap();
        Ok(registry.get(api_id).cloned())
    }

    async fn delete(&self, api_id: &ApiDefinitionKey,  _namespace: &()) -> Result<bool, ApiRegistrationError> {
        let mut registry = self.registry.lock().unwrap();
        let result = registry.remove(api_id);
        Ok(result.is_some())
    }

    async fn get_all(&self,  _namespace: &()) -> Result<Vec<ApiDefinition>, ApiRegistrationError> {
        let registry = self.registry.lock().unwrap();

        let result: Vec<ApiDefinition> = registry.values().cloned().collect();

        Ok(result)
    }

    async fn get_all_versions(
        &self,
        api_id: &ApiDefinitionId,
        _namespace: &()
    ) -> Result<Vec<ApiDefinition>, ApiRegistrationError> {
        let registry = self.registry.lock().unwrap();

        let result: Vec<ApiDefinition> = registry
            .values()
            .filter(|api| api.id == *api_id)
            .cloned()
            .collect();

        Ok(result)
    }
}

pub struct RedisApiRegistry<Namespace> {
    pool: RedisPool,
    auth_service: Arc<dyn AuthService<Namespace> + Sync + Send>,
}

impl<Namespace> RedisApiRegistry<Namespace> {
    pub async fn new(config: &RedisConfig, auth_service: Arc<dyn AuthService<Namespace> + Sync + Send>,) -> Result<RedisApiRegistry<Namespace>, ApiRegistrationError> {
        let pool_result = RedisPool::configured(config)
            .await
            .map_err(|err| ApiRegistrationError::InternalError(err.to_string()))?;

        Ok(RedisApiRegistry { pool: pool_result, auth_service })
    }
}

#[async_trait]
impl<Namespace> RegisterApiDefinition<Namespace> for RedisApiRegistry<Namespace> {
    async fn register(&self, definition: &ApiDefinition, namespace: &Namespace) -> Result<(), ApiRegistrationError> {
        debug!("Register definition: id: {}", definition.id);
        let key: ApiDefinitionKey = ApiDefinitionKey::from(definition);
        let redis_key = key.get_api_definition_redis_key();

        let value: u32 = self
            .pool
            .with("persistence", "get_definition")
            .exists(redis_key.clone())
            .await
            .map_err(|e| ApiRegistrationError::InternalError(e.to_string()))?;

        // if key already exists return conflict error
        if value > 0 {
            Err(ApiRegistrationError::AlreadyExists(key))
        } else {
            let definition_value = self
                .pool
                .serialize(definition)
                .map_err(|e| ApiRegistrationError::InternalError(e.to_string()))?;

            self.pool
                .with("persistence", "register_definition")
                .transaction({
                    move |transaction| async move {
                        let all_apis_value = key.make_all_apis_redis_value();

                        transaction
                            .set(redis_key, definition_value, None, None, false)
                            .await?;
                        transaction
                            .sadd(
                                ApiDefinitionKey::get_all_apis_set_redis_key(),
                                vec![all_apis_value],
                            )
                            .await?;

                        Ok(transaction)
                    }
                })
                .await
                .map_err(|e| ApiRegistrationError::InternalError(e.to_string()))?;

            Ok(())
        }
    }

    async fn get(
        &self,
        api_id: &ApiDefinitionKey,
        namespace: &Namespace
    ) -> Result<Option<ApiDefinition>, ApiRegistrationError> {
        info!("Get definition: id: {}", api_id);
        let key = api_id.get_api_definition_redis_key();
        let value: Option<Bytes> = self
            .pool
            .with("persistence", "get_definition")
            .get(key)
            .await
            .map_err(|e| ApiRegistrationError::InternalError(e.to_string()))?;

        match value {
            Some(value) => {
                let value: Result<ApiDefinition, ApiRegistrationError> = self
                    .pool
                    .deserialize(&value)
                    .map_err(|e| ApiRegistrationError::InternalError(e.to_string()));
                value.map(Some)
            }
            None => Ok(None),
        }
    }

    async fn delete(&self, api_id: &ApiDefinitionKey, namespace: &Namespace) -> Result<bool, ApiRegistrationError> {
        debug!("Delete definition: id: {}", api_id);
        let definition_key = api_id.get_api_definition_redis_key();
        let all_definitions_key = ApiDefinitionKey::get_all_apis_set_redis_key();

        let definition_value = api_id.make_all_apis_redis_value();

        let (definition_delete, _): (u32, ()) = self
            .pool
            .with("persistence", "delete_definition")
            .transaction({
                move |transaction| async move {
                    transaction.del(definition_key).await?;
                    transaction
                        .srem(all_definitions_key, vec![definition_value])
                        .await?;

                    Ok(transaction)
                }
            })
            .await
            .map_err(|e| ApiRegistrationError::InternalError(e.to_string()))?;

        Ok(definition_delete > 0)
    }

    async fn get_all(&self, namespace: &Namespace) -> Result<Vec<ApiDefinition>, ApiRegistrationError> {
        let all_apis: Vec<ApiDefinitionKey> = self.get_all_keys().await?;

        let api_definitions = self.get_all_api_definitions(all_apis).await?;

        Ok(api_definitions)
    }

    async fn get_all_versions(
        &self,
        api_id: &ApiDefinitionId,
        namespace: &Namespace
    ) -> Result<Vec<ApiDefinition>, ApiRegistrationError> {
        let api_versions: Vec<ApiDefinitionKey> = self
            .get_all_keys()
            .await?
            .into_iter()
            .filter(|api| api.id == *api_id)
            .collect();

        let api_definitions = self.get_all_api_definitions(api_versions).await?;

        Ok(api_definitions)
    }
}

impl<Namespace> RedisApiRegistry<Namespace> {
    async fn get_all_api_definitions(
        &self,
        keys: Vec<ApiDefinitionKey>,
    ) -> Result<Vec<ApiDefinition>, ApiRegistrationError> {
        let keys = keys
            .into_iter()
            .map(|api_id| api_id.get_api_definition_redis_key())
            .collect::<Vec<_>>();

        let result: Vec<Option<Bytes>> = self
            .pool
            .with("persistence", "mget_all_definitions")
            .mget(keys)
            .await
            .map_err(|e| ApiRegistrationError::InternalError(e.to_string()))?;

        let definitions = result
            .into_iter()
            .flatten()
            .map(|value| {
                self.pool
                    .deserialize(&value)
                    .map_err(|e| ApiRegistrationError::InternalError(e.to_string()))
            })
            .collect::<Result<Vec<ApiDefinition>, ApiRegistrationError>>()?;

        Ok(definitions)
    }

    async fn get_all_keys(&self) -> Result<Vec<ApiDefinitionKey>, ApiRegistrationError> {
        let all_apis_set_key = ApiDefinitionKey::get_all_apis_set_redis_key();
        let result: Vec<String> = self
            .pool
            .with("persistence", "get_all_definitions")
            .smembers(all_apis_set_key)
            .await
            .map_err(|e| ApiRegistrationError::InternalError(e.to_string()))?;

        let keys = result
            .into_iter()
            .filter_map(|definition_string| {
                ApiDefinitionKey::try_from(definition_string.as_str()).ok()
            })
            .collect();

        Ok(keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::config::RedisConfig;
    use crate::auth::AuthServiceNoop;

    use crate::register::{
        ApiDefinitionKey, InMemoryRegistry, RedisApiRegistry, RegisterApiDefinition,
    };

    fn get_simple_api_definition_example(
        id: &ApiDefinitionKey,
        path_pattern: &str,
        worker_id: &str,
    ) -> ApiDefinition {
        let yaml_string = format!(
            r#"
          id: '{}'
          version: 0.0.1
          routes:
          - method: Get
            path: '{}'
            binding:
              type: wit-worker
              template: 0b6d9cd8-f373-4e29-8a5a-548e61b868a5
              workerId: '{}'
              functionName: golem:it/api/get-cart-contents
              functionParams: []
        "#,
            id.id, path_pattern, worker_id
        );

        serde_yaml::from_str(yaml_string.as_str()).unwrap()
    }

    #[tokio::test]
    pub async fn test_in_memory_register() {
        let registry = InMemoryRegistry::default();

        let id = ApiDefinitionId("api1".to_string());
        let version = Version("0.0.1".to_string());

        let api_id1 = ApiDefinitionKey { id, version };

        let api_definition1 = get_simple_api_definition_example(
            &api_id1,
            "getcartcontent/{cart-id}",
            "cart-${path.cart-id}",
        );

        let id2 = ApiDefinitionId("api2".to_string());
        let version = Version("0.0.1".to_string());
        let api_id2 = ApiDefinitionKey { id: id2, version };

        let api_definition2 = get_simple_api_definition_example(
            &api_id2,
            "getcartcontent/{cart-id}",
            "cart-${path.cart-id}",
        );

        registry.register(&api_definition1, &()).await.unwrap();

        registry.register(&api_definition2, &()).await.unwrap();

        let api_definition1_result1 = registry.get(&api_id1, &()).await.unwrap_or(None);

        let api_definition2_result1 = registry.get(&api_id2, &()).await.unwrap_or(None);

        let api_definition_result2 = registry.get_all(&()).await.unwrap_or(vec![]);

        let delete1_result = registry.delete(&api_id1, &()).await.unwrap_or(false);

        let api_definition1_result3 = registry.get(&api_id1, &()).await.unwrap_or(None);

        let api_definition_result3 = registry.get_all(&()).await.unwrap_or(vec![]);

        let delete2_result = registry.delete(&api_id2, &()).await.unwrap_or(false);

        let api_definition2_result3 = registry.get(&api_id2, &()).await.unwrap_or(None);

        let api_definition_result4 = registry.get_all(&()).await.unwrap_or(vec![]);

        assert!(api_definition1_result1.is_some());
        assert!(!api_definition_result2.is_empty());
        assert!(api_definition2_result1.is_some());
        assert_eq!(api_definition1_result1.unwrap(), api_definition1);
        assert_eq!(api_definition_result2.len(), 2);
        assert!(delete1_result);
        assert!(delete2_result);
        assert!(api_definition1_result3.is_none());
        assert!(api_definition2_result3.is_none());
        assert_eq!(api_definition_result3[0], api_definition2);
        assert!(api_definition_result4.is_empty());
    }

    #[tokio::test]
    #[ignore]
    pub async fn test_redis_register() {
        let config = RedisConfig {
            key_prefix: "registry_test:".to_string(),
            database: 1,
            ..Default::default()
        };

        let auth_context = AuthServiceNoop{};

        let registry = RedisApiRegistry::new(&config, Arc::new(auth_context)).await.unwrap();

        let id1 = ApiDefinitionId("api1".to_string());
        let version = Version("0.0.1".to_string());
        let api_id1 = ApiDefinitionKey { id: id1, version };

        let api_definition1 = get_simple_api_definition_example(
            &api_id1,
            "getcartcontent/{cart-id}",
            "cart-${path.cart-id}",
        );

        let id2 = ApiDefinitionId("api2".to_string());
        let version = Version("0.0.1".to_string());
        let api_id2 = ApiDefinitionKey { id: id2, version };

        let api_definition2 = get_simple_api_definition_example(
            &api_id2,
            "getcartcontent/{cart-id}",
            "cart-${path.cart-id}",
        );

        registry.register(&api_definition1, &()).await.unwrap();

        registry.register(&api_definition2, &()).await.unwrap();

        let api_definition1_result1 = registry.get(&api_id1, &()).await.unwrap_or(None);

        let api_definition2_result1 = registry.get(&api_id2, &()).await.unwrap_or(None);

        let api_definition_result2 = registry.get_all(&()).await.unwrap_or(vec![]);

        let delete1_result = registry.delete(&api_id1, &()).await.unwrap_or(false);

        let api_definition1_result3 = registry.get(&api_id1, &()).await.unwrap_or(None);

        let api_definition_result3 = registry.get_all(&()).await.unwrap_or(vec![]);

        let delete2_result = registry.delete(&api_id2, &()).await.unwrap_or(false);

        let api_definition2_result3 = registry.get(&api_id2, &()).await.unwrap_or(None);

        let api_definition_result4 = registry.get_all(&()).await.unwrap_or(vec![]);

        assert!(api_definition1_result1.is_some());
        assert!(!api_definition_result2.is_empty());
        assert!(api_definition2_result1.is_some());
        assert_eq!(api_definition1_result1.unwrap(), api_definition1);
        assert_eq!(api_definition_result2.len(), 2);
        assert!(delete1_result);
        assert!(delete2_result);
        assert!(api_definition1_result3.is_none());
        assert!(api_definition2_result3.is_none());
        assert_eq!(api_definition_result3[0], api_definition2);
        assert!(api_definition_result4.is_empty());
    }

    #[test]
    pub fn test_get_api_definition_redis_key() {
        let id = ApiDefinitionId("api1".to_string());
        let version = Version("0.0.1".to_string());
        let api_id = ApiDefinitionKey { id, version };

        assert_eq!(
            api_id.get_api_definition_redis_key(),
            "apidefinition:definition:api1:0.0.1"
        );
    }
}
