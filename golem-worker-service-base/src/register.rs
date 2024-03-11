use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use crate::api_definition::{ApiDefinition, ApiDefinitionId, Version};
use crate::auth::AuthService;
use crate::service::register_definition::ApiDefinitionKey;
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::config::RedisConfig;
use golem_common::redis::RedisPool;
use tracing::{debug, info};

#[async_trait]
pub trait RegisterApiDefinitionRepo<Namespace: Eq + Hash + PartialEq + Clone + Debug + Display> {
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

pub struct InMemoryRegistry<Namespace: Eq + Hash + PartialEq + Clone + Debug + Display> {
    registry: Mutex<HashMap<ApiDefinitionKey<Namespace>, ApiDefinition>>,
}

impl<Namespace: Eq + Hash + PartialEq + Clone + Debug + Display> Default for InMemoryRegistry<Namespace> {
    fn default() -> Self {
        InMemoryRegistry {
            registry: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl<Namespace: Eq + Hash + PartialEq + Clone + Debug + Display + Send + Sync> RegisterApiDefinitionRepo<Namespace>
    for InMemoryRegistry<Namespace>
{
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
        _namespace: &Namespace,
    ) -> Result<Vec<ApiDefinition>, ApiRegistrationRepoError> {
        let registry = self.registry.lock().unwrap();

        let result: Vec<ApiDefinition> = registry.values().cloned().collect();

        Ok(result)
    }

    async fn get_all_versions(
        &self,
        api_id: &ApiDefinitionId,
        namespace: &Namespace,
    ) -> Result<Vec<ApiDefinition>, ApiRegistrationRepoError> {
        todo!()
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

fn get_api_definition_redis_key<Namespace: Eq + Hash + PartialEq + Clone + Debug + Display>(
    namespace: &Namespace,
    api_id: &ApiDefinitionId,
) -> String {
    format!("{}:definition:{}:{}", "apidefinition", namespace, api_id)
}

fn get_namespace_redis_key<Namespace: Eq + Hash + PartialEq + Clone + Debug + Display>(namespace: &Namespace) -> String {
    format!("{}:definition:{}", "apidefinition", namespace)
}

#[async_trait]
impl<Namespace: Eq + Hash + PartialEq + Clone + Debug + Display + Sync> RegisterApiDefinitionRepo<Namespace> for RedisApiRegistry {
    async fn register(
        &self,
        definition: &ApiDefinition,
        key: &ApiDefinitionKey<Namespace>,
    ) -> Result<(), ApiRegistrationRepoError> {
        debug!(
            "Register API definition {} under namespace: {}",
            key.id, key.namespace
        );

        // TODO: Bring back version
        let definition_key = get_api_definition_redis_key(&key.namespace, &key.id);

        let definition_value = self
            .pool
            .serialize(definition)
            .map_err(|e| ApiRegistrationRepoError::InternalError(e.to_string()))?;

        self.pool
            .with("persistence", "register_definition")
            .set(definition_key, definition_value, None, None, false)
            .await
            .map_err(|e| ApiRegistrationRepoError::InternalError(e.to_string()))?;

        let project_key = get_namespace_redis_key(&key.namespace);

        // TODO; bring back the transaction part
        let definition_id_value = self
            .pool
            .serialize(&definition.id.to_string())
            .map_err(|e| ApiRegistrationRepoError::InternalError(e.to_string()))?;

        self.pool
            .with("persistence", "register_project_definition")
            .sadd(project_key, definition_id_value)
            .await
            .map_err(|e| ApiRegistrationRepoError::InternalError(e.to_string()))
    }

    async fn get(
        &self,
        api_id: &ApiDefinitionKey<Namespace>,
    ) -> Result<Option<ApiDefinition>, ApiRegistrationRepoError> {
        info!(
            "Get from namespace: {}, id: {}",
            api_id.namespace, api_id.id
        );
        let key = get_api_definition_redis_key(&api_id.namespace, &api_id.id);
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
        api_id: &ApiDefinitionKey<Namespace>,
    ) -> Result<bool, ApiRegistrationRepoError> {
        debug!(
            "Delete from namespace: {}, id: {}",
            &api_id.namespace, &api_id.id
        );
        let definition_key = get_api_definition_redis_key(&api_id.namespace, &api_id.id);

        let namespace_key = get_namespace_redis_key(&api_id.namespace);

        let definition_id_value = self
            .pool
            .serialize(&api_id.to_string())
            .map_err(|e| ApiRegistrationRepoError::InternalError(e.to_string()))?;

        let _ = self
            .pool
            .with("persistence", "delete_project_definition")
            .srem(namespace_key, definition_id_value)
            .await
            .map_err(|e| ApiRegistrationRepoError::InternalError(e.to_string()))?;

        let definition_delete: u32 = self
            .pool
            .with("persistence", "delete_definition")
            .del(definition_key)
            .await
            .map_err(|e| ApiRegistrationRepoError::InternalError(e.to_string()))?;

        Ok(definition_delete > 0)
    }

    async fn get_all(
        &self,
        namespace: &Namespace,
    ) -> Result<Vec<ApiDefinition>, ApiRegistrationRepoError> {
        info!("Get all definitions in the namespace: {}", namespace);

        let namespace_key = get_namespace_redis_key(&namespace);

        let project_ids: Vec<Bytes> = self
            .pool
            .with("persistence", "get_project_definition_ids")
            .smembers(&namespace_key)
            .await
            .map_err(|e| ApiRegistrationRepoError::InternalError(e.to_string()))?;

        let mut api_ids = Vec::new();

        for api_id_value in project_ids {
            let api_id: Result<String, ApiRegistrationRepoError> = self
                .pool
                .deserialize(&api_id_value)
                .map_err(|e| ApiRegistrationRepoError::InternalError(e.to_string()));

            api_ids.push(ApiDefinitionId(api_id?));
        }

        let mut definitions = Vec::new();

        for api_id in api_ids {
            let key = get_api_definition_redis_key(&namespace, &api_id);

            let value: Option<Bytes> = self
                .pool
                .with("persistence", "get_definition")
                .get(&key)
                .await
                .map_err(|e| ApiRegistrationRepoError::InternalError(e.to_string()))?;

            if let Some(value) = value {
                let definition: Result<ApiDefinition, ApiRegistrationRepoError> = self
                    .pool
                    .deserialize(&value)
                    .map_err(|e| ApiRegistrationRepoError::InternalError(e.to_string()));
                definitions.push(definition?);
            }
        }

        Ok(definitions)
    }

    async fn get_all_versions(
        &self,
        api_id: &ApiDefinitionId,
        namespace: &Namespace,
    ) -> Result<Vec<ApiDefinition>, ApiRegistrationRepoError> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthServiceNoop;
    use golem_common::config::RedisConfig;
    use std::fmt::Formatter;

    use crate::register::{
        ApiDefinitionKey, InMemoryRegistry, RedisApiRegistry, RegisterApiDefinitionRepo,
    };

    #[derive(Clone)]
    struct CommonNamespace(String);

    impl CommonNamespace {
        pub fn default() -> CommonNamespace {
            CommonNamespace("common".to_string())
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
        let namespace = CommonNamespace::default();

        let api_id1 = ApiDefinitionKey {
            namespace,
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
        let namespace = CommonNamespace::default();

        let api_id2 = ApiDefinitionKey {
            namespace,
            id: id2,
            version,
        };

        let api_definition2 = get_simple_api_definition_example(
            &api_id2,
            "getcartcontent/{cart-id}",
            "cart-${path.cart-id}",
        );

        registry.register(&api_definition1, &()).await.unwrap();

        registry.register(&api_definition2, &()).await.unwrap();

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
            key_prefix: "registry_test:".to_string(),
            database: 1,
            ..Default::default()
        };

        let auth_context = AuthServiceNoop {};

        let registry = RedisApiRegistry::new(&config).await.unwrap();

        let id1 = ApiDefinitionId("api1".to_string());
        let version = Version("0.0.1".to_string());
        let namespace = CommonNamespace::default();

        let api_id1 = ApiDefinitionKey {
            namespace,
            id: id1,
            version,
        };

        let api_definition1 = get_simple_api_definition_example(
            &api_id1,
            "getcartcontent/{cart-id}",
            "cart-${path.cart-id}",
        );

        let id2 = ApiDefinitionId("api2".to_string());
        let version = Version("0.0.1".to_string());
        let namespace = CommonNamespace::default();

        let api_id2 = ApiDefinitionKey {
            namespace,
            id: id2,
            version,
        };

        let api_definition2 = get_simple_api_definition_example(
            &api_id2,
            "getcartcontent/{cart-id}",
            "cart-${path.cart-id}",
        );

        registry.register(&api_definition1, &api_id1).await.unwrap();

        registry.register(&api_definition2, &()).await.unwrap();

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

    #[test]
    pub fn test_get_api_definition_redis_key() {
        let id = ApiDefinitionId("api1".to_string());
        let version = Version("0.0.1".to_string());
        let namespace = CommonNamespace::default();

        let api_id = ApiDefinitionKey {
            namespace,
            id,
            version,
        };

        assert_eq!(
            api_id.get_api_definition_redis_key(),
            "apidefinition:definition:common:api1"
        );
    }
}
