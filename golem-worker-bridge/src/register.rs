use std::collections::HashMap;
use std::error::Error;
use std::sync::Mutex;

use crate::api_definition::{ApiDefinition, ApiDefinitionId};
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::config::RedisConfig;
use golem_common::redis::RedisPool;
use tracing::{debug, info};

const API_DEFINITION_REDIS_NAMESPACE: &str = "apidefinition";

#[async_trait]
pub trait RegisterApiDefinition {
    async fn register(&self, definition: &ApiDefinition) -> Result<(), Box<dyn Error>>;

    async fn get(
        &self,
        api_definition_id: &ApiDefinitionId,
    ) -> Result<Option<ApiDefinition>, Box<dyn Error>>;

    async fn delete(&self, api_definition_id: &ApiDefinitionId) -> Result<bool, Box<dyn Error>>;

    async fn get_all(&self) -> Result<Vec<ApiDefinition>, Box<dyn Error>>;
}

pub struct InMemoryRegistry {
    registry: Mutex<HashMap<ApiDefinitionId, ApiDefinition>>,
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
    async fn register(&self, definition: &ApiDefinition) -> Result<(), Box<dyn Error>> {
        let mut registry = self.registry.lock().unwrap();

        let key: ApiDefinitionId = definition.id.clone();

        registry.insert(key, definition.clone());

        Ok(())
    }

    async fn get(&self, api_id: &ApiDefinitionId) -> Result<Option<ApiDefinition>, Box<dyn Error>> {
        let registry = self.registry.lock().unwrap();

        Ok(registry.get(api_id).cloned())
    }

    async fn delete(&self, api_id: &ApiDefinitionId) -> Result<bool, Box<dyn Error>> {
        let mut registry = self.registry.lock().unwrap();

        let result = registry.remove(api_id);

        Ok(result.is_some())
    }

    async fn get_all(&self) -> Result<Vec<ApiDefinition>, Box<dyn Error>> {
        let registry = self.registry.lock().unwrap();

        let result: Vec<ApiDefinition> = registry.values().cloned().collect();

        Ok(result)
    }
}

pub struct RedisApiRegistry {
    pool: RedisPool,
}

impl RedisApiRegistry {
    pub async fn new(config: &RedisConfig) -> Result<RedisApiRegistry, Box<dyn Error>> {
        let pool = golem_common::redis::RedisPool::configured(config).await?;
        Ok(Self { pool })
    }
}

#[async_trait]
impl RegisterApiDefinition for RedisApiRegistry {
    async fn register(&self, definition: &ApiDefinition) -> Result<(), Box<dyn Error>> {
        debug!("Register account: id: {}", definition.id);

        let definition_key = get_api_definition_redis_key(&definition.id);

        let definition_value = self.pool.serialize(definition).map_err(|e| e.to_string())?;

        self.pool
            .with("persistence", "register_definition")
            .set(definition_key, definition_value, None, None, false)
            .await
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    async fn get(&self, api_id: &ApiDefinitionId) -> Result<Option<ApiDefinition>, Box<dyn Error>> {
        info!("Get account: id: {}", api_id);
        let key = get_api_definition_redis_key(api_id);
        let value: Option<Bytes> = self
            .pool
            .with("persistence", "get_definition")
            .get(key)
            .await
            .map_err(|e| e.to_string())?;

        match value {
            Some(value) => {
                let value: Result<ApiDefinition, Box<dyn Error>> = self
                    .pool
                    .deserialize(&value)
                    .map_err(|e| e.to_string().into());
                value.map(Some)
            }
            None => Ok(None),
        }
    }

    async fn get_all(&self) -> Result<Vec<ApiDefinition>, Box<dyn Error>> {
        unimplemented!("get_all")
    }

    async fn delete(&self, api_id: &ApiDefinitionId) -> Result<bool, Box<dyn Error>> {
        debug!("Delete account: id: {}", api_id);
        let definition_key = get_api_definition_redis_key(api_id);

        let definition_delete: u32 = self
            .pool
            .with("persistence", "delete_definition")
            .del(definition_key)
            .await
            .map_err(|e| e.to_string())?;

        Ok(definition_delete > 0)
    }
}

fn get_api_definition_redis_key(api_id: &ApiDefinitionId) -> String {
    format!("{}:definition:{}", API_DEFINITION_REDIS_NAMESPACE, api_id)
}

#[cfg(test)]
mod tests {
    use crate::api_definition::{ApiDefinition, ApiDefinitionId};
    use golem_common::config::RedisConfig;

    use crate::register::{
        get_api_definition_redis_key, InMemoryRegistry, RedisApiRegistry, RegisterApiDefinition,
    };

    fn get_simple_api_definition_example(
        id: &ApiDefinitionId,
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
            id, path_pattern, worker_id
        );

        serde_yaml::from_str(yaml_string.as_str()).unwrap()
    }

    #[tokio::test]
    pub async fn test_in_memory_register() {
        let registry = InMemoryRegistry::default();

        let api_id1 = ApiDefinitionId("api1".to_string());

        let api_definition1 = get_simple_api_definition_example(
            &api_id1,
            "getcartcontent/{cart-id}",
            "cart-${path.cart-id}",
        );

        let api_id2 = ApiDefinitionId("api2".to_string());

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

        let api_id1 = ApiDefinitionId("api1".to_string());

        let api_definition1 = get_simple_api_definition_example(
            &api_id1,
            "getcartcontent/{cart-id}",
            "cart-${path.cart-id}",
        );

        let api_id2 = ApiDefinitionId("api2".to_string());

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
        assert_eq!(api_definition1_result1.unwrap(), &api_definition1);
        assert_eq!(api_definition_result2.len(), 2);
        assert!(delete1_result);
        assert!(delete2_result);
        assert!(api_definition1_result3.is_none());
        assert!(api_definition2_result3.is_none());
        assert_eq!(api_definition_result3[0], &api_definition2);
        assert!(api_definition_result4.is_empty());
    }

    #[test]
    pub fn test_get_api_definition_redis_key() {
        let api_id = ApiDefinitionId("api1".to_string());

        assert_eq!(
            get_api_definition_redis_key(&api_id),
            "apidefinition:definition:api1"
        );
    }
}
