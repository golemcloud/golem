use std::collections::HashMap;
use std::error::Error;
use std::sync::Mutex;

use async_trait::async_trait;
use bytes::Bytes;
use golem_common::config::RedisConfig;
use golem_common::model::AccountId;
use golem_common::model::ProjectId;
use golem_common::redis::RedisPool;
use tracing::{debug, info};

use crate::apispec::{AccountApiDefinition, ApiDefinitionId};

const API_DEFINITION_REDIS_NAMESPACE: &str = "apidefinition";

#[async_trait]
pub trait RegisterApiDefinition {
    async fn register(&self, definition: &AccountApiDefinition) -> Result<(), Box<dyn Error>>;

    async fn get(
        &self,
        account_id: &AccountId,
        project_id: &ProjectId,
        api_definition_id: &ApiDefinitionId,
    ) -> Result<Option<AccountApiDefinition>, Box<dyn Error>>;

    async fn delete(
        &self,
        account_id: &AccountId,
        project_id: &ProjectId,
        api_definition_id: &ApiDefinitionId,
    ) -> Result<bool, Box<dyn Error>>;

    async fn get_all(
        &self,
        account_id: &AccountId,
        project_id: &ProjectId,
    ) -> Result<Vec<AccountApiDefinition>, Box<dyn Error>>;
}

pub struct InMemoryRegistry {
    registry: Mutex<HashMap<(AccountId, ProjectId, ApiDefinitionId), AccountApiDefinition>>,
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
    async fn register(&self, definition: &AccountApiDefinition) -> Result<(), Box<dyn Error>> {
        let mut registry = self.registry.lock().unwrap();

        let key: (AccountId, ProjectId, ApiDefinitionId) = (
            definition.account_id.clone(),
            definition.definition.project_id.clone(),
            definition.definition.id.clone(),
        );

        registry.insert(key, definition.clone());

        Ok(())
    }

    async fn get(
        &self,
        account_id: &AccountId,
        project_id: &ProjectId,
        api_id: &ApiDefinitionId,
    ) -> Result<Option<AccountApiDefinition>, Box<dyn Error>> {
        let key: (AccountId, ProjectId, ApiDefinitionId) =
            (account_id.clone(), project_id.clone(), api_id.clone());
        let registry = self.registry.lock().unwrap();

        Ok(registry.get(&key).cloned())
    }

    async fn delete(
        &self,
        account_id: &AccountId,
        project_id: &ProjectId,
        api_id: &ApiDefinitionId,
    ) -> Result<bool, Box<dyn Error>> {
        let key: (AccountId, ProjectId, ApiDefinitionId) =
            (account_id.clone(), project_id.clone(), api_id.clone());

        let mut registry = self.registry.lock().unwrap();

        let result = registry.remove(&key);

        Ok(result.is_some())
    }

    async fn get_all(
        &self,
        account_id: &AccountId,
        project_id: &ProjectId,
    ) -> Result<Vec<AccountApiDefinition>, Box<dyn Error>> {
        let registry = self.registry.lock().unwrap();

        let result: Vec<AccountApiDefinition> = registry
            .values()
            .filter(|x| &x.account_id == account_id && &x.definition.project_id == project_id)
            .cloned()
            .collect();

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
    async fn register(&self, definition: &AccountApiDefinition) -> Result<(), Box<dyn Error>> {
        debug!(
            "Register account: {}, project: {}, id: {}",
            definition.account_id, definition.definition.project_id, definition.definition.id
        );
        let definition_key = get_api_definition_redis_key(
            &definition.account_id,
            &definition.definition.project_id,
            &definition.definition.id,
        );

        let definition_value = self.pool.serialize(definition).map_err(|e| e.to_string())?;

        self.pool
            .with("persistence", "register_definition")
            .set(definition_key, definition_value, None, None, false)
            .await
            .map_err(|e| e.to_string())?;

        let project_key = get_project_api_definition_redis_key(
            &definition.account_id,
            &definition.definition.project_id,
        );

        let definition_id_value = self
            .pool
            .serialize(&definition.definition.id.to_string())
            .map_err(|e| e.to_string())?;

        self.pool
            .with("persistence", "register_project_definition")
            .sadd(project_key, definition_id_value)
            .await
            .map_err(|e| e.to_string().into())
    }

    async fn get(
        &self,
        account_id: &AccountId,
        project_id: &ProjectId,
        api_id: &ApiDefinitionId,
    ) -> Result<Option<AccountApiDefinition>, Box<dyn Error>> {
        info!(
            "Get account: {}, project: {}, id: {}",
            account_id, project_id, api_id
        );
        let key = get_api_definition_redis_key(account_id, project_id, api_id);
        let value: Option<Bytes> = self
            .pool
            .with("persistence", "get_definition")
            .get(key)
            .await
            .map_err(|e| e.to_string())?;

        match value {
            Some(value) => {
                let value: Result<AccountApiDefinition, Box<dyn Error>> = self
                    .pool
                    .deserialize(&value)
                    .map_err(|e| e.to_string().into());
                value.map(Some)
            }
            None => Ok(None),
        }
    }

    async fn get_all(
        &self,
        account_id: &AccountId,
        project_id: &ProjectId,
    ) -> Result<Vec<AccountApiDefinition>, Box<dyn Error>> {
        info!("Get account: {}, project: {}", account_id, project_id);

        let project_key = get_project_api_definition_redis_key(account_id, project_id);

        let project_ids: Vec<Bytes> = self
            .pool
            .with("persistence", "get_project_definition_ids")
            .smembers(&project_key)
            .await
            .map_err(|e| e.to_string())?;

        let mut api_ids = Vec::new();

        for api_id_value in project_ids {
            let api_id: Result<String, Box<dyn Error>> = self
                .pool
                .deserialize(&api_id_value)
                .map_err(|e| e.to_string().into());

            api_ids.push(ApiDefinitionId(api_id?));
        }

        let mut definitions = Vec::new();

        for api_id in api_ids {
            let key = get_api_definition_redis_key(account_id, project_id, &api_id);

            let value: Option<Bytes> = self
                .pool
                .with("persistence", "get_definition")
                .get(&key)
                .await
                .map_err(|e| e.to_string())?;

            if let Some(value) = value {
                let definition: Result<AccountApiDefinition, Box<dyn Error>> = self
                    .pool
                    .deserialize(&value)
                    .map_err(|e| e.to_string().into());
                definitions.push(definition?);
            }
        }

        Ok(definitions)
    }

    async fn delete(
        &self,
        account_id: &AccountId,
        project_id: &ProjectId,
        api_id: &ApiDefinitionId,
    ) -> Result<bool, Box<dyn Error>> {
        debug!(
            "Delete account: {}, project: {}, id: {}",
            account_id, project_id, api_id
        );
        let definition_key = get_api_definition_redis_key(account_id, project_id, api_id);

        let project_key = get_project_api_definition_redis_key(account_id, project_id);

        let definition_id_value = self
            .pool
            .serialize(&api_id.to_string())
            .map_err(|e| e.to_string())?;

        let _ = self
            .pool
            .with("persistence", "delete_project_definition")
            .srem(project_key, definition_id_value)
            .await
            .map_err(|e| e.to_string())?;

        let definition_delete: u32 = self
            .pool
            .with("persistence", "delete_definition")
            .del(definition_key)
            .await
            .map_err(|e| e.to_string())?;

        Ok(definition_delete > 0)
    }
}

fn get_api_definition_redis_key(
    account_id: &AccountId,
    project_id: &ProjectId,
    api_id: &ApiDefinitionId,
) -> String {
    format!(
        "{}:definition:{}:{}:{}",
        API_DEFINITION_REDIS_NAMESPACE, account_id, project_id, api_id
    )
}

fn get_project_api_definition_redis_key(account_id: &AccountId, project_id: &ProjectId) -> String {
    format!(
        "{}:definition:{}:{}",
        API_DEFINITION_REDIS_NAMESPACE, account_id, project_id
    )
}

#[cfg(test)]
mod tests {
    use golem_common::config::RedisConfig;
    use golem_common::model::AccountId;
    use golem_common::model::ProjectId;

    use crate::apispec::{AccountApiDefinition, ApiDefinition, ApiDefinitionId};
    use crate::register::{
        get_api_definition_redis_key, InMemoryRegistry, RedisApiRegistry, RegisterApiDefinition,
    };

    fn get_simple_api_definition_example(
        id: &ApiDefinitionId,
        project_id: &ProjectId,
        path_pattern: &str,
        worker_id: &str,
    ) -> ApiDefinition {
        let yaml_string = format!(
            r#"
          id: '{}'
          version: 0.0.1
          projectId: '{}'
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
            id, project_id, path_pattern, worker_id
        );

        serde_yaml::from_str(yaml_string.as_str()).unwrap()
    }

    #[tokio::test]
    pub async fn test_in_memory_register() {
        let registry = InMemoryRegistry::default();

        let account_id = AccountId::from("a1");

        let project_id = "15d70aa5-2e23-4ee3-b65c-4e1d702836a3"
            .parse::<ProjectId>()
            .unwrap();

        let api_id1 = ApiDefinitionId("api1".to_string());

        let api_definition1 = get_simple_api_definition_example(
            &api_id1,
            &project_id,
            "getcartcontent/{cart-id}",
            "cart-${path.cart-id}",
        );

        let api_id2 = ApiDefinitionId("api2".to_string());

        let api_definition2 = get_simple_api_definition_example(
            &api_id2,
            &project_id,
            "getcartcontent/{cart-id}",
            "cart-${path.cart-id}",
        );

        let definition1 = AccountApiDefinition::new(&account_id, &api_definition1);

        registry.register(&definition1).await.unwrap();

        let definition2 = AccountApiDefinition::new(&account_id, &api_definition2);

        registry.register(&definition2).await.unwrap();

        let api_definition1_result1 = registry
            .get(&account_id, &project_id, &api_id1)
            .await
            .unwrap_or(None);

        let api_definition2_result1 = registry
            .get(&account_id, &project_id, &api_id2)
            .await
            .unwrap_or(None);

        let api_definition_result2 = registry
            .get_all(&account_id, &project_id)
            .await
            .unwrap_or(vec![]);

        let delete1_result = registry
            .delete(&account_id, &project_id, &api_id1)
            .await
            .unwrap_or(false);

        let api_definition1_result3 = registry
            .get(&account_id, &project_id, &api_id1)
            .await
            .unwrap_or(None);

        let api_definition_result3 = registry
            .get_all(&account_id, &project_id)
            .await
            .unwrap_or(vec![]);

        let delete2_result = registry
            .delete(&account_id, &project_id, &api_id2)
            .await
            .unwrap_or(false);

        let api_definition2_result3 = registry
            .get(&account_id, &project_id, &api_id2)
            .await
            .unwrap_or(None);

        let api_definition_result4 = registry
            .get_all(&account_id, &project_id)
            .await
            .unwrap_or(vec![]);

        assert!(api_definition1_result1.is_some());
        assert!(!api_definition_result2.is_empty());
        assert!(api_definition2_result1.is_some());
        assert_eq!(api_definition1_result1.unwrap(), definition1);
        assert_eq!(api_definition_result2.len(), 2);
        assert!(delete1_result);
        assert!(delete2_result);
        assert!(api_definition1_result3.is_none());
        assert!(api_definition2_result3.is_none());
        assert_eq!(api_definition_result3[0], definition2);
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

        let account_id = AccountId::from("a1");

        let project_id = "15d70aa5-2e23-4ee3-b65c-4e1d702836a3"
            .parse::<ProjectId>()
            .unwrap();

        let api_id1 = ApiDefinitionId("api1".to_string());

        let api_definition1 = get_simple_api_definition_example(
            &api_id1,
            &project_id,
            "getcartcontent/{cart-id}",
            "cart-${path.cart-id}",
        );

        let api_id2 = ApiDefinitionId("api2".to_string());

        let api_definition2 = get_simple_api_definition_example(
            &api_id2,
            &project_id,
            "getcartcontent/{cart-id}",
            "cart-${path.cart-id}",
        );

        let definition1 = AccountApiDefinition::new(&account_id, &api_definition1);

        registry.register(&definition1).await.unwrap();

        let definition2 = AccountApiDefinition::new(&account_id, &api_definition2);

        registry.register(&definition2).await.unwrap();

        let api_definition1_result1 = registry
            .get(&account_id, &project_id, &api_id1)
            .await
            .unwrap_or(None);

        let api_definition2_result1 = registry
            .get(&account_id, &project_id, &api_id2)
            .await
            .unwrap_or(None);

        let api_definition_result2 = registry
            .get_all(&account_id, &project_id)
            .await
            .unwrap_or(vec![]);

        let delete1_result = registry
            .delete(&account_id, &project_id, &api_id1)
            .await
            .unwrap_or(false);

        let api_definition1_result3 = registry
            .get(&account_id, &project_id, &api_id1)
            .await
            .unwrap_or(None);

        let api_definition_result3 = registry
            .get_all(&account_id, &project_id)
            .await
            .unwrap_or(vec![]);

        let delete2_result = registry
            .delete(&account_id, &project_id, &api_id2)
            .await
            .unwrap_or(false);

        let api_definition2_result3 = registry
            .get(&account_id, &project_id, &api_id2)
            .await
            .unwrap_or(None);

        let api_definition_result4 = registry
            .get_all(&account_id, &project_id)
            .await
            .unwrap_or(vec![]);

        assert!(api_definition1_result1.is_some());
        assert!(!api_definition_result2.is_empty());
        assert!(api_definition2_result1.is_some());
        assert_eq!(api_definition1_result1.unwrap(), definition1);
        assert_eq!(api_definition_result2.len(), 2);
        assert!(delete1_result);
        assert!(delete2_result);
        assert!(api_definition1_result3.is_none());
        assert!(api_definition2_result3.is_none());
        assert_eq!(api_definition_result3[0], definition2);
        assert!(api_definition_result4.is_empty());
    }

    #[test]
    pub fn test_get_api_definition_redis_key() {
        let account_id = AccountId::from("a1");
        let project_id = "15d70aa5-2e23-4ee3-b65c-4e1d702836a3"
            .parse::<ProjectId>()
            .unwrap();
        let api_id = ApiDefinitionId("api1".to_string());

        assert_eq!(
            get_api_definition_redis_key(&account_id, &project_id, &api_id),
            "apidefinition:definition:a1:15d70aa5-2e23-4ee3-b65c-4e1d702836a3:api1"
        );
    }
}
