use crate::model::AccountApiDomain;
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::config::RedisConfig;
use golem_common::model::{AccountId, ProjectId};
use golem_common::redis::RedisPool;
use std::collections::HashMap;
use std::error::Error;
use std::sync::Mutex;
use tracing::{debug, info};

const API_DEFINITION_REDIS_NAMESPACE: &str = "apidefinition";

#[async_trait]
pub trait ApiDomainRepo {
    async fn create_or_update(&self, domain: &AccountApiDomain) -> Result<(), Box<dyn Error>>;

    async fn get(&self, domain_name: &str) -> Result<Option<AccountApiDomain>, Box<dyn Error>>;

    async fn delete(&self, domain_name: &str) -> Result<bool, Box<dyn Error>>;

    async fn get_by_id(
        &self,
        account_id: &AccountId,
        project_id: &ProjectId,
    ) -> Result<Vec<AccountApiDomain>, Box<dyn Error>>;
}

pub struct InMemoryApiDomainRepo {
    domains: Mutex<HashMap<String, AccountApiDomain>>,
}

impl Default for InMemoryApiDomainRepo {
    fn default() -> Self {
        InMemoryApiDomainRepo {
            domains: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl ApiDomainRepo for InMemoryApiDomainRepo {
    async fn create_or_update(&self, domain: &AccountApiDomain) -> Result<(), Box<dyn Error>> {
        debug!("Create or update - domain: {}", domain.domain.domain_name);

        let key = domain.domain.domain_name.to_string().clone();

        let mut domains = self.domains.lock().unwrap();

        domains.insert(key, domain.clone());

        Ok(())
    }

    async fn get(&self, domain_name: &str) -> Result<Option<AccountApiDomain>, Box<dyn Error>> {
        debug!("Get - domain: {}", domain_name);
        let domains = self.domains.lock().unwrap();

        let domains = domains.get(domain_name).cloned();

        Ok(domains)
    }

    async fn delete(&self, domain_name: &str) -> Result<bool, Box<dyn Error>> {
        debug!("Delete - domain: {}", domain_name);
        let mut domains = self.domains.lock().unwrap();

        let domains = domains.remove(domain_name);

        Ok(domains.is_some())
    }

    async fn get_by_id(
        &self,
        account_id: &AccountId,
        project_id: &ProjectId,
    ) -> Result<Vec<AccountApiDomain>, Box<dyn Error>> {
        debug!("Get - account: {}, project: {}", account_id, project_id);
        let registry = self.domains.lock().unwrap();

        let result: Vec<AccountApiDomain> = registry
            .values()
            .filter(|x| &x.account_id == account_id && &x.domain.project_id == project_id)
            .cloned()
            .collect();

        Ok(result)
    }
}

pub struct RedisApiDomainRepo {
    pool: RedisPool,
}

impl RedisApiDomainRepo {
    pub async fn new(config: &RedisConfig) -> Result<RedisApiDomainRepo, Box<dyn Error>> {
        let pool = golem_common::redis::RedisPool::configured(config).await?;
        Ok(Self { pool })
    }
}

#[async_trait]
impl ApiDomainRepo for RedisApiDomainRepo {
    async fn create_or_update(&self, domain: &AccountApiDomain) -> Result<(), Box<dyn Error>> {
        debug!("Create or update - domain: {}", domain.domain.domain_name);

        let key = get_api_domain_name_redis_key(domain.domain.domain_name.as_str());

        let value = self.pool.serialize(domain).map_err(|e| e.to_string())?;

        let project_key = get_api_domain_redis_key(&domain.account_id, &domain.domain.project_id);

        self.pool
            .with("persistence", "register_domains")
            .set(key, value, None, None, false)
            .await
            .map_err(|e| e.to_string())?;

        let project_domain_value = self
            .pool
            .serialize(&domain.domain.domain_name.to_string())
            .map_err(|e| e.to_string())?;
        let score: f64 = 1.0;

        self.pool
            .with("persistence", "register_domains")
            .zadd(
                project_key,
                None,
                None,
                false,
                false,
                (score, project_domain_value),
            )
            .await
            .map_err(|e| e.to_string().into())
    }

    async fn get(&self, domain_name: &str) -> Result<Option<AccountApiDomain>, Box<dyn Error>> {
        info!("Get - domain: {}", domain_name);
        let key = get_api_domain_name_redis_key(domain_name);
        let value: Option<Bytes> = self
            .pool
            .with("persistence", "get_domains")
            .get(key)
            .await
            .map_err(|e| e.to_string())?;

        match value {
            Some(value) => {
                let value: Result<AccountApiDomain, Box<dyn Error>> = self
                    .pool
                    .deserialize(&value)
                    .map_err(|e| e.to_string().into());
                value.map(Some)
            }
            None => Ok(None),
        }
    }

    async fn delete(&self, domain_name: &str) -> Result<bool, Box<dyn Error>> {
        debug!("Delete - domain: {}", domain_name);
        let key = get_api_domain_name_redis_key(domain_name);
        let value: Option<Bytes> = self
            .pool
            .with("persistence", "delete_domains")
            .get(key.clone())
            .await
            .map_err(|e| e.to_string())?;

        match value {
            Some(value) => {
                let domains: AccountApiDomain =
                    self.pool.deserialize(&value).map_err(|e| e.to_string())?;

                let project_domain_key =
                    get_api_domain_redis_key(&domains.account_id, &domains.domain.project_id);

                let project_domain_value = self
                    .pool
                    .serialize(&domains.domain.domain_name.to_string())
                    .map_err(|e| e.to_string())?;

                let _ = self
                    .pool
                    .with("persistence", "delete_domains")
                    .zrem(project_domain_key, project_domain_value)
                    .await
                    .map_err(|e| e.to_string())?;

                let definition_delete: u32 = self
                    .pool
                    .with("persistence", "delete_domains")
                    .del(key)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(definition_delete > 0)
            }
            None => Ok(false),
        }
    }

    async fn get_by_id(
        &self,
        account_id: &AccountId,
        project_id: &ProjectId,
    ) -> Result<Vec<AccountApiDomain>, Box<dyn Error>> {
        debug!("Get - account: {}, project: {}", account_id, project_id);
        let project_domain_key = get_api_domain_redis_key(account_id, project_id);

        let project_domain_values: Vec<Bytes> = self
            .pool
            .with("persistence", "get_domains")
            .zrange(&project_domain_key, 0, -1, None, false, None, false)
            .await
            .map_err(|e| e.to_string())?;

        let mut project_domains = Vec::new();

        for value in project_domain_values {
            let domain: Result<String, Box<dyn Error>> = self
                .pool
                .deserialize(&value)
                .map_err(|e| e.to_string().into());

            project_domains.push(domain?);
        }

        let mut domains = Vec::new();

        for domain in project_domains {
            let key = get_api_domain_name_redis_key(domain.as_str());

            let value: Option<Bytes> = self
                .pool
                .with("persistence", "get_domains")
                .get(&key)
                .await
                .map_err(|e| e.to_string())?;

            if let Some(value) = value {
                let domain: Result<AccountApiDomain, Box<dyn Error>> = self
                    .pool
                    .deserialize(&value)
                    .map_err(|e| e.to_string().into());
                domains.push(domain?);
            }
        }

        Ok(domains)
    }
}

fn get_api_domain_name_redis_key(domain_name: &str) -> String {
    format!("{}:domain:{}", API_DEFINITION_REDIS_NAMESPACE, domain_name)
}

fn get_api_domain_redis_key(account_id: &AccountId, project_id: &ProjectId) -> String {
    format!(
        "{}:domain:{}:{}",
        API_DEFINITION_REDIS_NAMESPACE, account_id, project_id
    )
}

#[cfg(test)]
mod tests {
    use golem_common::config::RedisConfig;
    use golem_common::model::AccountId;
    use golem_common::model::ProjectId;

    use crate::model::{AccountApiDomain, ApiDomain};
    use crate::repo::api_domain::{
        get_api_domain_name_redis_key, get_api_domain_redis_key, ApiDomainRepo,
        InMemoryApiDomainRepo, RedisApiDomainRepo,
    };

    #[tokio::test]
    pub async fn test_in_memory_registry() {
        let registry = InMemoryApiDomainRepo::default();

        let account_id = AccountId::from("a1");

        let project_id = "15d70aa5-2e23-4ee3-b65c-4e1d702836a3"
            .parse::<ProjectId>()
            .unwrap();

        let domain_name = "my-domain.com".to_string();
        let domain = AccountApiDomain::new(
            &account_id,
            &ApiDomain {
                project_id,
                domain_name: domain_name.clone(),
                name_servers: vec!["ns1.com".to_string(), "ns2.com".to_string()],
            },
        );

        let _ = registry.create_or_update(&domain).await;

        let result = registry.get(domain_name.as_str()).await.unwrap_or(None);

        let result1 = registry
            .get_by_id(&domain.account_id, &domain.domain.project_id)
            .await
            .unwrap_or(vec![]);

        let delete = registry.delete(domain_name.as_str()).await.unwrap_or(false);

        let result2 = registry.get(domain_name.as_str()).await.unwrap_or(None);

        assert!(result.is_some());
        assert_eq!(result.unwrap(), domain);
        assert!(!result1.is_empty());
        assert_eq!(result1[0], domain);
        assert!(result2.is_none());
        assert!(delete);
    }

    #[tokio::test]
    #[ignore]
    pub async fn test_redis_registry() {
        let config = RedisConfig {
            key_prefix: "domain_test:".to_string(),
            database: 1,
            ..Default::default()
        };

        let registry = RedisApiDomainRepo::new(&config).await.unwrap();

        let account_id = AccountId::from("a1");

        let project_id = "15d70aa5-2e23-4ee3-b65c-4e1d702836a3"
            .parse::<ProjectId>()
            .unwrap();

        let domain_name = "my-domain.com".to_string();
        let domain = AccountApiDomain::new(
            &account_id,
            &ApiDomain {
                project_id,
                domain_name: domain_name.clone(),
                name_servers: vec!["ns1.com".to_string(), "ns2.com".to_string()],
            },
        );

        let _ = registry.create_or_update(&domain).await;

        let result = registry.get(domain_name.as_str()).await.unwrap_or(None);

        let result1 = registry
            .get_by_id(&domain.account_id, &domain.domain.project_id)
            .await
            .unwrap_or(vec![]);

        let delete = registry.delete(domain_name.as_str()).await.unwrap_or(false);

        let result2 = registry.get(domain_name.as_str()).await.unwrap_or(None);

        assert!(result.is_some());
        assert_eq!(result.unwrap(), domain);
        assert!(!result1.is_empty());
        assert_eq!(result1[0], domain);
        assert!(result2.is_none());
        assert!(delete);
    }

    #[test]
    pub fn test_get_api_domain_name_redis_key() {
        assert_eq!(
            get_api_domain_name_redis_key("my-domain.com"),
            "apidefinition:domain:my-domain.com"
        );
    }

    #[test]
    pub fn test_get_api_domain_redis_key() {
        let account_id = AccountId::from("a1");
        let project_id = "15d70aa5-2e23-4ee3-b65c-4e1d702836a3"
            .parse::<ProjectId>()
            .unwrap();

        assert_eq!(
            get_api_domain_redis_key(&account_id, &project_id),
            "apidefinition:domain:a1:15d70aa5-2e23-4ee3-b65c-4e1d702836a3"
        );
    }
}
