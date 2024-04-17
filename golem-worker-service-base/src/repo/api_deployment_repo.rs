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
use crate::api_definition::{ApiDefinitionId, ApiDeployment, ApiVersion, HasApiDefinitionId, Host};

use crate::repo::api_definition_repo::{ApiDefinitionRepo, InMemoryRegistry};
use crate::repo::api_namespace::ApiNamespace;
use crate::api_definition::HasHost;

const API_DEFINITION_REDIS_NAMESPACE: &str = "apidefinition";

#[async_trait]
pub trait ApiDeploymentRepo<Namespace: ApiNamespace> {
    async fn deploy(&self, deployment: &ApiDeployment<Namespace>) -> Result<(), Box<dyn Error>>;

    async fn get(&self, host:&Host) -> Result<Option<ApiDeployment<Namespace>>, Box<dyn Error>>;

    async fn delete(&self, host: &Host) -> Result<bool, Box<dyn Error>>;

    async fn get_by_id(
        &self,
        namespace: Namespace,
        api_id: &ApiDefinitionId,
    ) -> Result<Vec<ApiDeployment<Namespace>>, Box<dyn Error>>;
}

pub struct InMemoryDeployment<Namespace> {
    deployments: Mutex<HashMap<Host, ApiDeployment<Namespace>>>,
}

impl<Namespace> Default for InMemoryDeployment<Namespace> {
    fn default() -> Self {
        InMemoryDeployment {
            deployments: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl<Namespace: ApiNamespace>
ApiDeploymentRepo<Namespace> for InMemoryDeployment<Namespace> {
    async fn deploy(&self, deployment: &ApiDeployment<Namespace>) -> Result<(), Box<dyn Error>> {
        debug!(
            "Deploy API site: {}, id: {}",
            deployment.site, deployment.get_api_definition_id()
        );

        let key = deployment.site.clone();

        let mut deployments = self.deployments.lock().unwrap();

        deployments.insert(key, deployment.clone());

        Ok(())
    }

    async fn get(&self, host: Host) -> Result<Option<ApiDeployment<Namespace>>, Box<dyn Error>> {
        debug!("Get API site: {}", host);
        let deployments = self.deployments.lock().unwrap();

        let deployment = deployments.get(&host).cloned();

        Ok(deployment)
    }

    async fn delete(&self, host: Host) -> Result<bool, Box<dyn Error>> {
        debug!("Delete API site: {}", host);
        let mut deployments = self.deployments.lock().unwrap();

        let deployment = deployments.remove(&host);

        Ok(deployment.is_some())
    }

    async fn get_by_id(
        &self,
        namespace: Namespace,
        api_id: &ApiDefinitionId,
    ) -> Result<Vec<ApiDeployment<Namespace>>, Box<dyn Error>> {

        let registry = self.deployments.lock().unwrap();

        let result: Vec<ApiDeployment<Namespace>> = registry
            .values()
            .filter(|x| {
                &x.api_definition_id.namespace == namespace
                    && &x.api_definition_id.id == api_id
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
impl<Namespace: ApiNamespace> ApiDeploymentRepo<Namespace> for RedisApiDeploy {
    async fn deploy(&self, deployment: &ApiDeployment<Namespace>) -> Result<(), Box<dyn Error>> {
        debug!(
            "Deploy API site: {}, id: {}",
            deployment.site, deployment.api_definition_id
        );

        let key = redis_keys::api_deployment_redis_key(&deployment.site);

        let value = self.pool.serialize(deployment).map_err(|e| e.to_string())?;

        let sites_key = redis_keys::api_deployments_redis_key(
            &deployment.api_definition_id.namespace,
            &deployment.api_definition_id.id,
        );

        self.pool
            .with("persistence", "deploy_deployment")
            .set(key, value, None, None, false)
            .await
            .map_err(|e| e.to_string())?;

        let site_value = self
            .pool
            .serialize(&deployment.site.to_string())
            .map_err(|e| e.to_string())?;
        let score: f64 = 1.0;

        self.pool
            .with("persistence", "deploy_deployment")
            .zadd(sites_key, None, None, false, false, (score, site_value))
            .await
            .map_err(|e| e.to_string().into())
    }

    async fn get(&self, host: Host) -> Result<Option<ApiDeployment<Namespace>>, Box<dyn Error>> {
        info!("Get host id: {}", host);

        let key = redis_keys::api_deployment_redis_key(&host);

        let value: Option<Bytes> = self
            .pool
            .with("persistence", "get_deployment")
            .get(key)
            .await
            .map_err(|e| e.to_string())?;

        match value {
            Some(value) => {
                let value: Result<ApiDeployment<Namespace>, Box<dyn Error>> = self
                    .pool
                    .deserialize(&value)
                    .map_err(|e| e.to_string().into());
                value.map(Some)
            }
            None => Ok(None),
        }
    }

    async fn delete(&self, host: Host) -> Result<bool, Box<dyn Error>> {
        debug!("Delete API site: {}", host);
        let key = redis_keys::api_deployment_redis_key(&host);
        let value: Option<Bytes> = self
            .pool
            .with("persistence", "delete_deployment")
            .get(key.clone())
            .await
            .map_err(|e| e.to_string())?;

        match value {
            Some(value) => {
                let deployment: ApiDeployment<Namespace> =
                    self.pool.deserialize(&value).map_err(|e| e.to_string())?;

                let sites_key = redis_keys::api_deployments_redis_key(
                    &deployment.api_definition_id.namespace,
                    &deployment.api_definition_id.id,
                );

                let site_value = self
                    .pool
                    .serialize(&deployment.site.to_string())
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
        namespace: Namespace,
        api_id: &ApiDefinitionId,
    ) -> Result<Vec<ApiDeployment<Namespace>>, Box<dyn Error>> {

        let sites_key = redis_keys::api_deployments_redis_key(&namespace, api_id);

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
            let key = redis_keys::api_deployment_redis_key(&Host::from_string(site.as_str()));

            let value: Option<Bytes> = self
                .pool
                .with("persistence", "get_deployment")
                .get(&key)
                .await
                .map_err(|e| e.to_string())?;

            if let Some(value) = value {
                let deployment: Result<ApiDeployment<Namespace>, Box<dyn Error>> = self
                    .pool
                    .deserialize(&value)
                    .map_err(|e| e.to_string().into());
                deployments.push(deployment?);
            }
        }

        Ok(deployments)
    }
}

// Api Deployments
// I should be able to deploy a version of an API to a specific site
// Meaning the ApiDeploymentKey should have versions in it.
// Store them with version as the key
// host --> api_deployment
// api_deployment_key --> api_deployments

mod redis_keys {
    use crate::api_definition::{ApiDefinitionId, Host};
    use crate::repo::api_deployment_repo::API_DEFINITION_REDIS_NAMESPACE;
    use crate::repo::api_namespace::ApiNamespace;

    pub(crate) fn api_deployment_redis_key(api_site: &Host) -> String {
        format!("{}:deployment:{}", API_DEFINITION_REDIS_NAMESPACE, api_site)
    }


    pub(crate) fn api_deployments_redis_key<Namespace: ApiNamespace>(
        namespace: &Namespace,
        api_id: &ApiDefinitionId,
    ) -> String {
        format!(
            "{}:deployments:{}:{}",
            API_DEFINITION_REDIS_NAMESPACE, namespace, api_id
        )
    }
}

#[cfg(test)]
mod tests {
    use golem_common::config::RedisConfig;
    use crate::api_definition::{ApiDefinitionId, ApiVersion, Domain, Host, SubDomain};

    use crate::api_definition::{ApiDeployment, Host};
    use crate::auth::CommonNamespace;
    use crate::repo::api_deployment_repo::{ApiDeploymentRepo,
                                           InMemoryDeployment, RedisApiDeploy,
    };
    use crate::repo::api_deployment_repo::redis_keys::{api_deployment_redis_key, api_deployments_redis_key};
    use crate::service::api_definition::ApiDefinitionKey;

    #[tokio::test]
    pub async fn test_in_memory_deploy() {
        let registry = InMemoryDeployment::default();

        let namespace = CommonNamespace::default();

        let site = Host {
            domain: Domain("dev-api.golem.cloud".to_string()),
            sub_domain: SubDomain("test".to_string()),
        };

        let api_definition_id = ApiDefinitionId("api1".to_string());
        let version = ApiVersion("0.0.1".to_string());

        let deployment = &ApiDeployment {
                api_definition_id: ApiDefinitionKey {
                    namespace: namespace.clone(),
                    id: api_definition_id.clone(),
                    version: version.clone(),
                },
                site: site.clone(),
            };

        let _ = registry.deploy(&deployment).await;

        let result = registry
            .get(site)
            .await
            .unwrap_or(None);

        let result1 = registry
            .get_by_id(
                CommonNamespace::default(),
                &deployment.api_definition_id.id,
            )
            .await
            .unwrap_or(vec![]);

        let delete = registry
            .delete(site)
            .await
            .unwrap_or(false);

        let result2 = registry
            .get(site)
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
            api_deployment_redis_key(&Host::from_string("foo.dev-api.golem.cloud")),
            "apidefinition:deployment:foo.dev-api.golem.cloud"
        );
    }

    #[test]
    pub fn test_get_api_deployments_redis_key() {
        let api_id = ApiDefinitionId("api1".to_string());

        assert_eq!(
            api_deployments_redis_key(&CommonNamespace::default(), &api_id),
            "apidefinition:deployments:common:api1"
        );
    }
}
