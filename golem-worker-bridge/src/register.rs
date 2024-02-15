use std::collections::HashMap;
use std::fmt::Display;
use std::sync::Mutex;

use crate::api_definition::{ApiDefinition, ApiDefinitionId, Version};
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::config::RedisConfig;
use golem_common::redis::RedisPool;
use tracing::{debug, info};

const API_DEFINITION_REDIS_NAMESPACE: &str = "apidefinition";

#[async_trait]
pub trait RegisterApiDefinition {
    async fn register(&self, definition: &ApiDefinition) -> Result<(), ApiRegistrationError>;

    async fn get(
        &self,
        api_definition_key: &ApiDefinitionKey,
    ) -> Result<Option<ApiDefinition>, ApiRegistrationError>;

    async fn delete(
        &self,
        api_definition_key: &ApiDefinitionKey,
    ) -> Result<bool, ApiRegistrationError>;

    async fn get_all(&self) -> Result<Vec<ApiDefinition>, ApiRegistrationError>;

    async fn get_all_versions(
        &self,
        api_id: &ApiDefinitionId,
    ) -> Result<Vec<ApiDefinition>, ApiRegistrationError>;
}

#[derive(Debug, Clone, Eq, Hash, PartialEq)]
pub struct ApiDefinitionKey {
    pub id: ApiDefinitionId,
    pub version: Version,
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
impl RegisterApiDefinition for InMemoryRegistry {
    async fn register(&self, definition: &ApiDefinition) -> Result<(), ApiRegistrationError> {
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
    ) -> Result<Option<ApiDefinition>, ApiRegistrationError> {
        let registry = self.registry.lock().unwrap();
        Ok(registry.get(api_id).cloned())
    }

    async fn delete(&self, api_id: &ApiDefinitionKey) -> Result<bool, ApiRegistrationError> {
        let mut registry = self.registry.lock().unwrap();
        let result = registry.remove(api_id);
        Ok(result.is_some())
    }

    async fn get_all(&self) -> Result<Vec<ApiDefinition>, ApiRegistrationError> {
        let registry = self.registry.lock().unwrap();

        let result: Vec<ApiDefinition> = registry.values().cloned().collect();

        Ok(result)
    }

    async fn get_all_versions(
        &self,
        api_id: &ApiDefinitionId,
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

pub struct RedisApiRegistry {
    pool: RedisPool,
}

impl RedisApiRegistry {
    pub async fn new(config: &RedisConfig) -> Result<RedisApiRegistry, ApiRegistrationError> {
        let pool_result = golem_common::redis::RedisPool::configured(config)
            .await
            .map_err(|err| ApiRegistrationError::InternalError(err.to_string()))?;

        Ok(RedisApiRegistry { pool: pool_result })
    }
}

#[async_trait]
impl RegisterApiDefinition for RedisApiRegistry {
    async fn register(&self, definition: &ApiDefinition) -> Result<(), ApiRegistrationError> {
        debug!("Register definition: id: {}", definition.id);
        let key: ApiDefinitionKey = ApiDefinitionKey::from(definition);
        let redis_key = get_api_definition_redis_key(&key);

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
                    let api_id = key.id.clone();
                    let api_version = key.version.0.clone();

                    move |transaction| async move {
                        let definition_key = get_api_definition_redis_key(&key);
                        let version_set_key = get_api_definition_redis_key_version_set(&api_id);

                        transaction
                            .set(definition_key, definition_value, None, None, false)
                            .await?;
                        transaction.sadd(version_set_key, vec![api_version]).await?;
                        transaction
                            .sadd(get_all_apis_set_redis_key(), vec![api_id.0])
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
    ) -> Result<Option<ApiDefinition>, ApiRegistrationError> {
        info!("Get definition: id: {}", api_id);
        let key = get_api_definition_redis_key(api_id);
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

    async fn delete(&self, api_id: &ApiDefinitionKey) -> Result<bool, ApiRegistrationError> {
        debug!("Delete definition: id: {}", api_id);
        let definition_key = get_api_definition_redis_key(api_id);
        let version_set_key = get_api_definition_redis_key_version_set(&api_id.id);

        let (definition_delete, _, num_versions): (u32, (), u32) = self
            .pool
            .with("persistence", "delete_definition")
            .transaction({
                let api_version = api_id.version.0.clone();

                move |transaction| async move {
                    transaction.del(definition_key).await?;
                    transaction
                        .srem(version_set_key.clone(), vec![api_version])
                        .await?;
                    transaction.scard(version_set_key).await?;

                    Ok(transaction)
                }
            })
            .await
            .map_err(|e| ApiRegistrationError::InternalError(e.to_string()))?;

        // If the definition was deleted and there are no more versions, remove the definition from the all_api_definitions set.
        if definition_delete > 1 && num_versions == 0 {
            let all_apis_key = get_all_apis_set_redis_key();
            let api_version = api_id.version.0.clone();
            self.pool
                .with("persistence", "delete_definition")
                .srem(all_apis_key, vec![api_version])
                .await
                .map_err(|e| {
                    tracing::error!(
                        "Failed to remove definition from all_api_definitions set: {}",
                        e
                    );
                    ApiRegistrationError::InternalError(e.to_string())
                })?;
        }

        Ok(definition_delete > 0)
    }

    async fn get_all(&self) -> Result<Vec<ApiDefinition>, ApiRegistrationError> {
        let all_apis: Vec<ApiDefinitionId> = {
            let result: Vec<String> = self
                .pool
                .with("persistence", "get_all_definitions")
                .smembers(get_all_apis_set_redis_key())
                .await
                .map_err(|e| ApiRegistrationError::InternalError(e.to_string()))?;

            result.into_iter().map(ApiDefinitionId).collect()
        };

        let all_versions: Vec<(ApiDefinitionId, Vec<String>)> = {
            let futures = all_apis.into_iter().map(|api_id| async move {
                let version_set_key = get_api_definition_redis_key_version_set(&api_id);
                let versions: Vec<String> = self
                    .pool
                    .with("persistence", "get_all_definitions")
                    .smembers(version_set_key)
                    .await
                    .map_err(|e| ApiRegistrationError::InternalError(e.to_string()))?;
                Ok((api_id, versions))
            });

            futures::future::try_join_all(futures).await?
        };

        let all_definitions: Vec<Option<ApiDefinition>> = {
            let futures = all_versions
                .into_iter()
                .flat_map(|(api_id, versions)| {
                    versions.into_iter().map(move |version| ApiDefinitionKey {
                        id: api_id.clone(),
                        version: Version(version),
                    })
                })
                .map(|api_id| async move { self.get(&api_id).await })
                .collect::<Vec<_>>();

            futures::future::try_join_all(futures).await?
        };

        Ok(all_definitions.into_iter().flatten().collect::<Vec<_>>())
    }

    async fn get_all_versions(
        &self,
        api_id: &ApiDefinitionId,
    ) -> Result<Vec<ApiDefinition>, ApiRegistrationError> {
        let version_set_key = get_api_definition_redis_key_version_set(api_id);

        // Fetch all versions from the set
        let versions: Vec<String> = self
            .pool
            .with("persistence", "get_all_versions")
            .smembers(version_set_key)
            .await
            .map_err(|e| ApiRegistrationError::InternalError(e.to_string()))?;

        // For each version, construct an ApiDefinitionKey and fetch the ApiDefinition
        let futures: Vec<_> = versions
            .into_iter()
            .map(|version| ApiDefinitionKey {
                id: api_id.clone(),
                version: Version(version),
            })
            .map(|api_id| async move { self.get(&api_id).await })
            .collect();

        let api_definitions: Vec<Option<ApiDefinition>> = futures::future::try_join_all(futures)
            .await
            .map_err(|e| ApiRegistrationError::InternalError(e.to_string()))?;

        Ok(api_definitions.into_iter().flatten().collect())
    }
}

// Specific Api Definition Key.
fn get_api_definition_redis_key(api_id: &ApiDefinitionKey) -> String {
    format!(
        "{API_DEFINITION_REDIS_NAMESPACE}:definition:{}:{}",
        api_id.id, api_id.version
    )
}

// All Api Definition IDs.
fn get_all_apis_set_redis_key() -> String {
    format!("{API_DEFINITION_REDIS_NAMESPACE}:definition:all_api_definitions")
}

// All Api Versions for a given Api Definition ID.
fn get_api_definition_redis_key_version_set(api_id: &ApiDefinitionId) -> String {
    format!("{API_DEFINITION_REDIS_NAMESPACE}:definition:{api_id}:versions",)
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::config::RedisConfig;

    use crate::register::{
        get_api_definition_redis_key, ApiDefinitionKey, InMemoryRegistry, RedisApiRegistry,
        RegisterApiDefinition,
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

        registry.register(&api_definition1).await.unwrap();

        registry.register(&api_definition2).await.unwrap();

        let api_definition1_result1 = registry.get(&api_id1).await.unwrap_or(None);

        let api_definition2_result1 = registry.get(&api_id2).await.unwrap_or(None);

        let api_definition_result2 = registry.get_all().await.unwrap_or(vec![]);

        let delete1_result = registry.delete(&api_id1).await.unwrap_or(false);

        let api_definition1_result3 = registry.get(&api_id1).await.unwrap_or(None);

        let api_definition_result3 = registry.get_all().await.unwrap_or(vec![]);

        let delete2_result = registry.delete(&api_id2).await.unwrap_or(false);

        let api_definition2_result3 = registry.get(&api_id2).await.unwrap_or(None);

        let api_definition_result4 = registry.get_all().await.unwrap_or(vec![]);

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

        let registry = RedisApiRegistry::new(&config).await.unwrap();

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

        registry.register(&api_definition1).await.unwrap();

        registry.register(&api_definition2).await.unwrap();

        let api_definition1_result1 = registry.get(&api_id1).await.unwrap_or(None);

        let api_definition2_result1 = registry.get(&api_id2).await.unwrap_or(None);

        let api_definition_result2 = registry.get_all().await.unwrap_or(vec![]);

        let delete1_result = registry.delete(&api_id1).await.unwrap_or(false);

        let api_definition1_result3 = registry.get(&api_id1).await.unwrap_or(None);

        let api_definition_result3 = registry.get_all().await.unwrap_or(vec![]);

        let delete2_result = registry.delete(&api_id2).await.unwrap_or(false);

        let api_definition2_result3 = registry.get(&api_id2).await.unwrap_or(None);

        let api_definition_result4 = registry.get_all().await.unwrap_or(vec![]);

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
            get_api_definition_redis_key(&api_id),
            "apidefinition:definition:api1:0.0.1"
        );
    }

    #[test]
    pub fn test_get_all_apis_set_redis_key() {
        let id = ApiDefinitionId("api1".to_string());
        assert_eq!(
            get_api_definition_redis_key_version_set(&id),
            "apidefinition:definition:api1:versions"
        );
    }
}
