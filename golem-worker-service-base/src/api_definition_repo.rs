use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Debug, Display};
use std::sync::Mutex;

use crate::api_definition::{ApiDefinition, ApiDefinitionId};
use crate::service::api_definition_service::{ApiDefinitionKey, ApiNamespace};
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::config::RedisConfig;
use golem_common::redis::RedisPool;
use tracing::{debug, info};

#[async_trait]
pub trait ApiDefinitionRepo<Namespace: ApiNamespace> {
    async fn register(
        &self,
        definition: &ApiDefinition,
        key: &ApiDefinitionKey<Namespace>,
    ) -> Result<(), ApiRegistrationRepoError>;

    async fn get(
        &self,
        api_definition_key: &ApiDefinitionKey<Namespace>,
    ) -> Result<Option<ApiDefinition>, ApiRegistrationRepoError>;

    async fn delete(
        &self,
        api_definition_key: &ApiDefinitionKey<Namespace>,
    ) -> Result<bool, ApiRegistrationRepoError>;

    async fn get_all(
        &self,
        namespace: &Namespace,
    ) -> Result<Vec<ApiDefinition>, ApiRegistrationRepoError>;

    async fn get_all_versions(
        &self,
        api_id: &ApiDefinitionId,
        namespace: &Namespace,
    ) -> Result<Vec<ApiDefinition>, ApiRegistrationRepoError>;
}

#[derive(Debug, Clone)]
pub enum ApiRegistrationRepoError {
    AlreadyExists(ApiDefinitionKey<String>),
    InternalError(String),
}

impl From<Box<dyn Error>> for ApiRegistrationRepoError {
    fn from(value: Box<dyn Error>) -> Self {
        ApiRegistrationRepoError::InternalError(value.to_string())
    }
}

impl Display for ApiRegistrationRepoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiRegistrationRepoError::InternalError(msg) => write!(f, "InternalError: {}", msg),
            ApiRegistrationRepoError::AlreadyExists(api_definition_key) => {
                write!(
                    f,
                    "AlreadyExists: ApiDefinition with id: {} and version:{} already exists in the namespace {}",
                    api_definition_key.id, api_definition_key.version.0, api_definition_key.namespace
                )
            }
        }
    }
}

pub struct InMemoryRegistry<Namespace> {
    registry: Mutex<HashMap<ApiDefinitionKey<Namespace>, ApiDefinition>>,
}

impl<Namespace> Default for InMemoryRegistry<Namespace> {
    fn default() -> Self {
        InMemoryRegistry {
            registry: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl<Namespace: ApiNamespace> ApiDefinitionRepo<Namespace> for InMemoryRegistry<Namespace> {
    async fn register(
        &self,
        definition: &ApiDefinition,
        key: &ApiDefinitionKey<Namespace>,
    ) -> Result<(), ApiRegistrationRepoError> {
        let mut registry = self.registry.lock().unwrap();

        if let std::collections::hash_map::Entry::Vacant(e) = registry.entry(key.clone()) {
            e.insert(definition.clone());
            Ok(())
        } else {
            Err(ApiRegistrationRepoError::AlreadyExists(
                key.with_namespace_displayed(),
            ))
        }
    }

    async fn get(
        &self,
        api_id: &ApiDefinitionKey<Namespace>,
    ) -> Result<Option<ApiDefinition>, ApiRegistrationRepoError> {
        let registry = self.registry.lock().unwrap();
        Ok(registry.get(api_id).cloned())
    }

    async fn delete(
        &self,
        api_id: &ApiDefinitionKey<Namespace>,
    ) -> Result<bool, ApiRegistrationRepoError> {
        let mut registry = self.registry.lock().unwrap();
        let result = registry.remove(api_id);
        Ok(result.is_some())
    }

    async fn get_all(
        &self,
        namespace: &Namespace,
    ) -> Result<Vec<ApiDefinition>, ApiRegistrationRepoError> {
        let registry = self.registry.lock().unwrap();

        let result: Vec<ApiDefinition> = registry
            .iter()
            .filter(|(k, _)| k.namespace == *namespace)
            .map(|(_, v)| v.clone())
            .collect();

        Ok(result)
    }

    async fn get_all_versions(
        &self,
        api_id: &ApiDefinitionId,
        namespace: &Namespace,
    ) -> Result<Vec<ApiDefinition>, ApiRegistrationRepoError> {
        let registry = self.registry.lock().unwrap();
        let result = registry
            .iter()
            .filter(|(k, _)| k.namespace == *namespace && k.id == *api_id)
            .map(|(_, v)| v.clone())
            .collect();

        Ok(result)
    }
}

pub struct RedisApiRegistry {
    pool: RedisPool,
}

impl RedisApiRegistry {
    pub async fn new(config: &RedisConfig) -> Result<RedisApiRegistry, ApiRegistrationRepoError> {
        let pool_result = RedisPool::configured(config)
            .await
            .map_err(|err| ApiRegistrationRepoError::InternalError(err.to_string()))?;

        Ok(RedisApiRegistry { pool: pool_result })
    }
}

#[async_trait]
impl<Namespace: ApiNamespace> ApiDefinitionRepo<Namespace> for RedisApiRegistry {
    async fn register(
        &self,
        definition: &ApiDefinition,
        key: &ApiDefinitionKey<Namespace>,
    ) -> Result<(), ApiRegistrationRepoError> {
        debug!(
            "Register API definition {} under namespace: {}",
            key.id, key.namespace
        );

        let definition_key = redis_keys::api_definition_key(key);

        let exists_count: u32 = self
            .pool
            .with("persistence", "get_definition")
            .exists(definition_key.clone())
            .await
            .map_err(|e| ApiRegistrationRepoError::InternalError(e.to_string()))?;

        if exists_count > 0 {
            Err(ApiRegistrationRepoError::AlreadyExists(
                key.with_namespace_displayed(),
            ))
        } else {
            let definition_value = self
                .pool
                .serialize(definition)
                .map_err(|e| ApiRegistrationRepoError::InternalError(e.to_string()))?;

            let namespace_set_key = redis_keys::namespace_set_key(&key.namespace);
            let namespace_set_value = redis_keys::namespace_set_value(key)?;

            self.pool
                .with("persistence", "register_definition")
                .transaction(|transaction| async move {
                    transaction
                        .set(definition_key, definition_value, None, None, false)
                        .await?;
                    transaction
                        .sadd(namespace_set_key, vec![namespace_set_value])
                        .await?;

                    Ok(transaction)
                })
                .await
                .map_err(|e| ApiRegistrationRepoError::InternalError(e.to_string()))?;

            Ok(())
        }
    }

    async fn get(
        &self,
        key: &ApiDefinitionKey<Namespace>,
    ) -> Result<Option<ApiDefinition>, ApiRegistrationRepoError> {
        info!("Get from namespace: {}, id: {}", key.namespace, key.id);
        let key = redis_keys::api_definition_key(key);
        let value: Option<Bytes> = self
            .pool
            .with("persistence", "get_definition")
            .get(key)
            .await
            .map_err(|e| ApiRegistrationRepoError::InternalError(e.to_string()))?;

        match value {
            Some(value) => {
                let value: Result<ApiDefinition, ApiRegistrationRepoError> = self
                    .pool
                    .deserialize(&value)
                    .map_err(|e| ApiRegistrationRepoError::InternalError(e.to_string()));

                value.map(Some)
            }
            None => Ok(None),
        }
    }

    async fn delete(
        &self,
        key: &ApiDefinitionKey<Namespace>,
    ) -> Result<bool, ApiRegistrationRepoError> {
        debug!("Delete from namespace: {}, id: {}", &key.namespace, &key.id);
        let definition_key = redis_keys::api_definition_key(key);
        let all_definitions_key = redis_keys::namespace_set_key(&key.namespace);
        let definition_value = redis_keys::namespace_set_value(key)?;

        let (definition_delete, _): (u32, ()) = self
            .pool
            .with("persistence", "delete_definition")
            .transaction(|transaction| async move {
                transaction.del(definition_key).await?;
                transaction
                    .srem(all_definitions_key, vec![definition_value])
                    .await?;

                Ok(transaction)
            })
            .await
            .map_err(|e| ApiRegistrationRepoError::InternalError(e.to_string()))?;

        Ok(definition_delete > 0)
    }

    async fn get_all(
        &self,
        namespace: &Namespace,
    ) -> Result<Vec<ApiDefinition>, ApiRegistrationRepoError> {
        info!("Get all definitions in the namespace: {}", namespace);

        let api_ids = self.get_all_keys(namespace).await?;

        self.get_all_api_definitions(api_ids).await
    }

    async fn get_all_versions(
        &self,
        api_id: &ApiDefinitionId,
        namespace: &Namespace,
    ) -> Result<Vec<ApiDefinition>, ApiRegistrationRepoError> {
        let api_ids = self.get_all_keys(namespace).await?;

        let api_ids = api_ids
            .into_iter()
            .filter(|k| k.id == *api_id)
            .collect::<Vec<_>>();

        self.get_all_api_definitions(api_ids).await
    }
}

impl RedisApiRegistry {
    /// Retrieve all keys for a given namespace.
    async fn get_all_keys<Namespace: ApiNamespace>(
        &self,
        namespace: &Namespace,
    ) -> Result<Vec<ApiDefinitionKey<Namespace>>, ApiRegistrationRepoError> {
        let namespace_key = redis_keys::namespace_set_key(namespace);

        let project_ids: Vec<Bytes> = self
            .pool
            .with("persistence", "get_project_definition_ids")
            .smembers(&namespace_key)
            .await
            .map_err(|e| ApiRegistrationRepoError::InternalError(e.to_string()))?;

        let api_ids = project_ids
            .into_iter()
            .map(redis_keys::namespace_set_value_deserialize)
            .collect::<Result<Vec<ApiDefinitionKey<Namespace>>, ApiRegistrationRepoError>>()?;

        Ok(api_ids)
    }

    /// Retrieve all api definitions for a given set of keys.
    async fn get_all_api_definitions<Namespace: ApiNamespace>(
        &self,
        keys: Vec<ApiDefinitionKey<Namespace>>,
    ) -> Result<Vec<ApiDefinition>, ApiRegistrationRepoError> {
        if keys.is_empty() {
            return Ok(vec![]);
        }

        let keys = keys
            .into_iter()
            .map(|k| redis_keys::api_definition_key(&k))
            .collect::<Vec<_>>();

        let result: Vec<Option<Bytes>> = self
            .pool
            .with("persistence", "mget_all_definitions")
            .mget(keys)
            .await
            .map_err(|e| ApiRegistrationRepoError::InternalError(e.to_string()))?;

        let definitions = result
            .into_iter()
            .flatten()
            .map(|value| {
                self.pool
                    .deserialize(&value)
                    .map_err(|e| ApiRegistrationRepoError::InternalError(e.to_string()))
            })
            .collect::<Result<Vec<ApiDefinition>, ApiRegistrationRepoError>>()?;

        Ok(definitions)
    }
}

mod redis_keys {

    use crate::service::api_definition_service::{ApiDefinitionKey, ApiNamespace};

    use super::ApiRegistrationRepoError;

    /// Key API Definition.
    pub fn api_definition_key<Namespace: ApiNamespace>(
        key: &ApiDefinitionKey<Namespace>,
    ) -> String {
        format!(
            "apidefinition:definition:{}:{}:{}",
            key.namespace, key.id.0, key.version.0,
        )
    }

    /// Key for redis set containing all the apis in a namespace.
    pub fn namespace_set_key<Namespace: ApiNamespace>(namespace: &Namespace) -> String {
        format!("apidefinition:definition:{}", namespace)
    }

    /// Value for the [`get_namespace_redis_key`] set.
    pub fn namespace_set_value<Namespace: ApiNamespace>(
        key: &ApiDefinitionKey<Namespace>,
    ) -> Result<bytes::Bytes, ApiRegistrationRepoError> {
        golem_common::serialization::serialize(key).map_err(ApiRegistrationRepoError::InternalError)
    }

    pub fn namespace_set_value_deserialize<Namespace: ApiNamespace>(
        value: bytes::Bytes,
    ) -> Result<ApiDefinitionKey<Namespace>, ApiRegistrationRepoError> {
        golem_common::serialization::deserialize(&value)
            .map_err(ApiRegistrationRepoError::InternalError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_definition::Version;
    use bincode::{Decode, Encode};
    use golem_common::config::RedisConfig;
    use serde::Deserialize;
    use std::fmt::Formatter;

    use crate::api_definition_repo::{
        ApiDefinitionKey, ApiDefinitionRepo, InMemoryRegistry, RedisApiRegistry,
    };

    #[derive(Clone, Eq, PartialEq, Debug, Hash, Decode, Encode, Deserialize)]
    struct CommonNamespace(String);

    impl CommonNamespace {
        fn new(namespace: impl Into<String>) -> Self {
            CommonNamespace(namespace.into())
        }
    }

    impl Display for CommonNamespace {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    fn get_simple_api_definition_example(
        id: &ApiDefinitionKey<CommonNamespace>,
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
        let namespace = CommonNamespace::new("default");

        let api_id1 = ApiDefinitionKey {
            namespace: namespace.clone(),
            id,
            version,
        };

        let api_definition1 = get_simple_api_definition_example(
            &api_id1,
            "getcartcontent/{cart-id}",
            "cart-${path.cart-id}",
        );

        let id2 = ApiDefinitionId("api2".to_string());
        let version = Version("0.0.1".to_string());
        let namespace = CommonNamespace::new("default");

        let api_id2 = ApiDefinitionKey {
            namespace: namespace.clone(),
            id: id2,
            version,
        };

        let api_definition2 = get_simple_api_definition_example(
            &api_id2,
            "getcartcontent/{cart-id}",
            "cart-${path.cart-id}",
        );

        registry.register(&api_definition1, &api_id1).await.unwrap();

        registry.register(&api_definition2, &api_id2).await.unwrap();

        let api_definition1_result1 = registry.get(&api_id1).await.unwrap_or(None);

        let api_definition2_result1 = registry.get(&api_id2).await.unwrap_or(None);

        let api_definition_result2 = registry.get_all(&namespace).await.unwrap_or(vec![]);

        let delete1_result = registry.delete(&api_id1).await.unwrap_or(false);

        let api_definition1_result3 = registry.get(&api_id1).await.unwrap_or(None);

        let api_definition_result3 = registry.get_all(&namespace).await.unwrap_or(vec![]);

        let delete2_result = registry.delete(&api_id2).await.unwrap_or(false);

        let api_definition2_result3 = registry.get(&api_id2).await.unwrap_or(None);

        let api_definition_result4 = registry.get_all(&namespace).await.unwrap_or(vec![]);

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
            key_prefix: "".to_string(),
            database: 0,
            ..Default::default()
        };

        let registry = RedisApiRegistry::new(&config).await.unwrap();

        let namespace = CommonNamespace::new("test");

        let api_id = ApiDefinitionId("api1".to_string());

        let api_id1 = ApiDefinitionKey {
            namespace: namespace.clone(),
            id: api_id.clone(),
            version: Version("0.0.1".to_string()),
        };

        let api_definition1 = get_simple_api_definition_example(
            &api_id1,
            "getcartcontent/{cart-id}",
            "cart-${path.cart-id}",
        );

        // Registration of an api definition

        registry.register(&api_definition1, &api_id1).await.unwrap();

        let retrieved_api = registry.get(&api_id1).await.unwrap().unwrap();

        assert_eq!(
            api_definition1, retrieved_api,
            "Failed to retrieve the api definition"
        );

        assert_eq!(
            vec![api_definition1.clone()],
            registry.get_all(&namespace).await.unwrap(),
            "Failed to retrieve all the api definitions"
        );

        assert_eq!(
            vec![api_definition1.clone()],
            registry
                .get_all_versions(&api_id1.id, &namespace)
                .await
                .unwrap(),
            "Failed to retrieve all the api definitions"
        );

        // Two versions of the same api definition

        let api_id2 = ApiDefinitionKey {
            namespace: namespace.clone(),
            id: api_id.clone(),
            version: Version("0.0.2".to_string()),
        };

        let api_definition2 = get_simple_api_definition_example(
            &api_id2,
            "getcartcontent/{cart-id}",
            "cart-${path.cart-id}",
        );

        registry.register(&api_definition2, &api_id2).await.unwrap();

        assert_eq!(
            vec![api_definition1.clone(), api_definition2.clone()],
            registry
                .get_all_versions(&api_id, &namespace)
                .await
                .unwrap(),
            "Failed to retrieve all the api definitions"
        );

        // Add completely new api definition.

        let api_id2 = ApiDefinitionId("api2".to_string());
        let api_id3 = ApiDefinitionKey {
            namespace: namespace.clone(),
            id: api_id2.clone(),
            version: Version("0.0.1".to_string()),
        };

        let api_definition3 = get_simple_api_definition_example(
            &api_id3,
            "getcartcontent/{cart-id}",
            "cart-${path.cart-id}",
        );
        registry.register(&api_definition3, &api_id3).await.unwrap();

        assert_eq!(
            vec![
                api_definition1.clone(),
                api_definition2.clone(),
                api_definition3.clone(),
            ],
            registry.get_all(&namespace).await.unwrap(),
            "Failed to retrieve all the api definitions"
        );

        assert_eq!(
            vec![api_definition3.clone()],
            registry
                .get_all_versions(&api_id2, &namespace)
                .await
                .unwrap(),
            "Failed to retrieve all the api definitions"
        );

        // Deletions.

        assert!(
            registry.delete(&api_id1).await.unwrap(),
            "Failed to delete the api definition"
        );

        assert_eq!(
            vec![api_definition2.clone(), api_definition3.clone()],
            registry.get_all(&namespace).await.unwrap(),
            "Failed to retrieve all the api definitions"
        );

        assert_eq!(
            vec![api_definition2.clone()],
            registry
                .get_all_versions(&api_id, &namespace)
                .await
                .unwrap(),
            "Failed to retrieve all the api definitions"
        );

        // Ensure namespaces are separate.

        let namespace2 = CommonNamespace::new("test2");
        let api_id4 = ApiDefinitionId("api4".to_string());
        let api_id5 = ApiDefinitionKey {
            namespace: namespace2.clone(),
            id: api_id4.clone(),
            version: Version("0.0.1".to_string()),
        };

        let api_definition4 = get_simple_api_definition_example(
            &api_id5,
            "getcartcontent/{cart-id}",
            "cart-${path.cart-id}",
        );

        registry.register(&api_definition4, &api_id5).await.unwrap();

        assert_eq!(
            vec![api_definition2.clone(), api_definition3.clone()],
            registry.get_all(&namespace).await.unwrap(),
            "Failed to retrieve all the api definitions for namespace 1"
        );

        assert_eq!(
            vec![api_definition4.clone()],
            registry.get_all(&namespace2).await.unwrap(),
            "Failed to retrieve all the api definitions for namespace 2"
        );
    }
}
