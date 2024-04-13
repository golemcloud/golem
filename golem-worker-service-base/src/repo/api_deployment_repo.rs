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
use crate::api_definition::{ApiDefinitionId, ApiVersion, Host};
use crate::repo::api_namespace::ApiNamespace;


const API_DEFINITION_REDIS_NAMESPACE: &str = "apidefinition";

#[async_trait]
pub trait DeployApiDefinition<Namespace: ApiNamespace, ApiDeployment> {
    async fn deploy(&self, deployment: &ApiDeployment) -> Result<(), Box<dyn Error>>;

    async fn get(&self, host: &str) -> Result<Option<ApiDeployment>, Box<dyn Error>>;

    async fn delete(&self, host: &str) -> Result<bool, Box<dyn Error>>;

    async fn get_by_id(
        &self,
        account_id: &AccountId,
        project_id: &ProjectId,
        api_id: &ApiDefinitionId,
    ) -> Result<Vec<ApiDeployment>, Box<dyn Error>>;
}

pub struct ApiDeploymentKey<Namespace> {
    pub namespace: Namespace,
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
    pub host: Host
}

pub struct InMemoryDeployment {
    deployments: Mutex<HashMap<String, ApiDeployment>>,
}

impl Default for InMemoryDeployment {
    fn default() -> Self {
        InMemoryDeployment {
            deployments: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl DeployApiDefinition for InMemoryDeployment {
    async fn deploy(&self, deployment: &AccountApiDeployment) -> Result<(), Box<dyn Error>> {
        debug!(
            "Deploy API site: {}, id: {}",
            deployment.deployment.site, deployment.deployment.api_definition_id
        );

        let key = deployment.deployment.site.to_string().clone();

        let mut deployments = self.deployments.lock().unwrap();

        deployments.insert(key, deployment.clone());

        Ok(())
    }

    async fn get(&self, host: &str) -> Result<Option<AccountApiDeployment>, Box<dyn Error>> {
        debug!("Get API site: {}", host);
        let deployments = self.deployments.lock().unwrap();

        let deployment = deployments.get(host).cloned();

        Ok(deployment)
    }

    async fn delete(&self, host: &str) -> Result<bool, Box<dyn Error>> {
        debug!("Delete API site: {}", host);
        let mut deployments = self.deployments.lock().unwrap();

        let deployment = deployments.remove(host);

        Ok(deployment.is_some())
    }

    async fn get_by_id(
        &self,
        account_id: &AccountId,
        project_id: &ProjectId,
        api_id: &ApiDefinitionId,
    ) -> Result<Vec<AccountApiDeployment>, Box<dyn Error>> {
        debug!(
            "Get account: {}, project: {}, id: {}",
            account_id, project_id, api_id
        );
        let registry = self.deployments.lock().unwrap();

        let result: Vec<AccountApiDeployment> = registry
            .values()
            .filter(|x| {
                &x.account_id == account_id
                    && &x.deployment.project_id == project_id
                    && &x.deployment.api_definition_id == api_id
            })
            .cloned()
            .collect();

        Ok(result)
    }
}

pub struct RedisApiDeploy {
    pool: RedisPool,
}

impl RedisApiDeploy {
    pub async fn new(config: &RedisConfig) -> Result<RedisApiDeploy, Box<dyn Error>> {
        let pool = golem_common::redis::RedisPool::configured(config).await?;
        Ok(Self { pool })
    }
}

#[async_trait]
impl DeployApiDefinition for RedisApiDeploy {
    async fn deploy(&self, deployment: &AccountApiDeployment) -> Result<(), Box<dyn Error>> {
        debug!(
            "Deploy API site: {}, id: {}",
            deployment.deployment.site, deployment.deployment.api_definition_id
        );

        let key = get_api_deployment_redis_key(deployment.deployment.site.to_string().as_str());

        let value = self.pool.serialize(deployment).map_err(|e| e.to_string())?;

        let sites_key = get_api_deployments_redis_key(
            &deployment.account_id,
            &deployment.deployment.project_id,
            &deployment.deployment.api_definition_id,
        );

        self.pool
            .with("persistence", "deploy_deployment")
            .set(key, value, None, None, false)
            .await
            .map_err(|e| e.to_string())?;

        let site_value = self
            .pool
            .serialize(&deployment.deployment.site.to_string())
            .map_err(|e| e.to_string())?;
        let score: f64 = 1.0;

        self.pool
            .with("persistence", "deploy_deployment")
            .zadd(sites_key, None, None, false, false, (score, site_value))
            .await
            .map_err(|e| e.to_string().into())
    }

    async fn get(&self, host: &str) -> Result<Option<AccountApiDeployment>, Box<dyn Error>> {
        info!("Get host id: {}", host);
        let key = get_api_deployment_redis_key(host);
        let value: Option<Bytes> = self
            .pool
            .with("persistence", "get_deployment")
            .get(key)
            .await
            .map_err(|e| e.to_string())?;

        match value {
            Some(value) => {
                let value: Result<AccountApiDeployment, Box<dyn Error>> = self
                    .pool
                    .deserialize(&value)
                    .map_err(|e| e.to_string().into());
                value.map(Some)
            }
            None => Ok(None),
        }
    }

    async fn delete(&self, host: &str) -> Result<bool, Box<dyn Error>> {
        debug!("Delete API site: {}", host);
        let key = get_api_deployment_redis_key(host);
        let value: Option<Bytes> = self
            .pool
            .with("persistence", "delete_deployment")
            .get(key.clone())
            .await
            .map_err(|e| e.to_string())?;

        match value {
            Some(value) => {
                let deployment: AccountApiDeployment =
                    self.pool.deserialize(&value).map_err(|e| e.to_string())?;

                let sites_key = get_api_deployments_redis_key(
                    &deployment.account_id,
                    &deployment.deployment.project_id,
                    &deployment.deployment.api_definition_id,
                );

                let site_value = self
                    .pool
                    .serialize(&deployment.deployment.site.to_string())
                    .map_err(|e| e.to_string())?;

                let _ = self
                    .pool
                    .with("persistence", "delete_deployment")
                    .zrem(sites_key, site_value)
                    .await
                    .map_err(|e| e.to_string())?;

                let definition_delete: u32 = self
                    .pool
                    .with("persistence", "delete_deployment")
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
        api_id: &ApiDefinitionId,
    ) -> Result<Vec<AccountApiDeployment>, Box<dyn Error>> {
        debug!(
            "Get account: {}, project: {}, id: {}",
            account_id, project_id, api_id
        );
        let sites_key = get_api_deployments_redis_key(account_id, project_id, api_id);

        let site_values: Vec<Bytes> = self
            .pool
            .with("persistence", "get_deployment")
            .zrange(&sites_key, 0, -1, None, false, None, false)
            .await
            .map_err(|e| e.to_string())?;

        let mut sites = Vec::new();

        for value in site_values {
            let site: Result<String, Box<dyn Error>> = self
                .pool
                .deserialize(&value)
                .map_err(|e| e.to_string().into());

            sites.push(site?);
        }

        let mut deployments = Vec::new();

        for site in sites {
            let key = get_api_deployment_redis_key(site.as_str());

            let value: Option<Bytes> = self
                .pool
                .with("persistence", "get_deployment")
                .get(&key)
                .await
                .map_err(|e| e.to_string())?;

            if let Some(value) = value {
                let deployment: Result<AccountApiDeployment, Box<dyn Error>> = self
                    .pool
                    .deserialize(&value)
                    .map_err(|e| e.to_string().into());
                deployments.push(deployment?);
            }
        }

        Ok(deployments)
    }
}

fn get_api_deployment_redis_key(api_site: &str) -> String {
    format!("{}:deployment:{}", API_DEFINITION_REDIS_NAMESPACE, api_site)
}

fn get_api_deployments_redis_key(
    account_id: &AccountId,
    project_id: &ProjectId,
    api_id: &ApiDefinitionId,
) -> String {
    format!(
        "{}:deployments:{}:{}:{}",
        API_DEFINITION_REDIS_NAMESPACE, account_id, project_id, api_id
    )
}

#[cfg(test)]
mod tests {
    use golem_common::config::RedisConfig;
    use golem_common::model::AccountId;
    use golem_common::model::ProjectId;
    use golem_worker_service_base::api_definition::{ApiDefinitionId, ApiVersion};

    use crate::apispec::{AccountApiDeployment, ApiDeployment, ApiSite};
    use crate::deploy::{
        get_api_deployment_redis_key, get_api_deployments_redis_key, DeployApiDefinition,
        InMemoryDeployment, RedisApiDeploy,
    };

    #[tokio::test]
    pub async fn test_in_memory_deploy() {
        let registry = InMemoryDeployment::default();

        let account_id = AccountId::from("a1");

        let project_id = "15d70aa5-2e23-4ee3-b65c-4e1d702836a3"
            .parse::<ProjectId>()
            .unwrap();

        let site = ApiSite {
            host: "dev-api.golem.cloud".to_string(),
            subdomain: "test".to_string(),
        };
        let api_definition_id = ApiDefinitionId("api1".to_string());
        let version = ApiVersion("0.0.1".to_string());

        let deployment = AccountApiDeployment::new(
            &account_id,
            &ApiDeployment {
                api_definition_id,
                version,
                project_id,
                site: site.clone(),
            },
        );

        let _ = registry.deploy(&deployment).await;

        let result = registry
            .get(site.to_string().as_str())
            .await
            .unwrap_or(None);

        let result1 = registry
            .get_by_id(
                &deployment.account_id,
                &deployment.deployment.project_id,
                &deployment.deployment.api_definition_id,
            )
            .await
            .unwrap_or(vec![]);

        let delete = registry
            .delete(site.to_string().as_str())
            .await
            .unwrap_or(false);

        let result2 = registry
            .get(site.to_string().as_str())
            .await
            .unwrap_or(None);

        assert!(result.is_some());
        assert_eq!(result.unwrap(), deployment);
        assert!(!result1.is_empty());
        assert_eq!(result1[0], deployment);
        assert!(result2.is_none());
        assert!(delete);
    }

    #[tokio::test]
    #[ignore]
    pub async fn test_redis_deploy() {
        let config = RedisConfig {
            key_prefix: "deploy_test:".to_string(),
            database: 1,
            ..Default::default()
        };

        let registry = RedisApiDeploy::new(&config).await.unwrap();

        let account_id = AccountId::from("a1");

        let project_id = "15d70aa5-2e23-4ee3-b65c-4e1d702836a3"
            .parse::<ProjectId>()
            .unwrap();

        let site = ApiSite {
            host: "dev-api.golem.cloud".to_string(),
            subdomain: "test".to_string(),
        };
        let api_definition_id = ApiDefinitionId("api1".to_string());
        let version = ApiVersion("0.0.1".to_string());

        let deployment = AccountApiDeployment::new(
            &account_id,
            &ApiDeployment {
                api_definition_id,
                version,
                project_id,
                site: site.clone(),
            },
        );

        let _ = registry.deploy(&deployment).await;

        let result = registry
            .get(site.to_string().as_str())
            .await
            .unwrap_or(None);

        let result1 = registry
            .get_by_id(
                &deployment.account_id,
                &deployment.deployment.project_id,
                &deployment.deployment.api_definition_id,
            )
            .await
            .unwrap_or(vec![]);

        let delete = registry
            .delete(site.to_string().as_str())
            .await
            .unwrap_or(false);

        let result2 = registry
            .get(site.to_string().as_str())
            .await
            .unwrap_or(None);

        assert!(result.is_some());
        assert_eq!(result.unwrap(), deployment);
        assert!(!result1.is_empty());
        assert_eq!(result1[0], deployment);
        assert!(result2.is_none());
        assert!(delete);
    }

    #[test]
    pub fn test_get_api_deployment_redis_key() {
        assert_eq!(
            get_api_deployment_redis_key("foo.dev-api.golem.cloud"),
            "apidefinition:deployment:foo.dev-api.golem.cloud"
        );
    }

    #[test]
    pub fn test_get_api_deployments_redis_key() {
        let account_id = AccountId::from("a1");
        let project_id = "15d70aa5-2e23-4ee3-b65c-4e1d702836a3"
            .parse::<ProjectId>()
            .unwrap();
        let api_id = ApiDefinitionId("api1".to_string());

        assert_eq!(
            get_api_deployments_redis_key(&account_id, &project_id, &api_id),
            "apidefinition:deployments:a1:15d70aa5-2e23-4ee3-b65c-4e1d702836a3:api1"
        );
    }
}
