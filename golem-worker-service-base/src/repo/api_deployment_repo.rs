use std::collections::HashMap;
use std::sync::Mutex;

use crate::api_definition::{ApiDefinitionId, ApiDeployment, ApiSite, ApiSiteString};
use async_trait::async_trait;

use bytes::Bytes;

use golem_common::config::RedisConfig;

use golem_common::redis::{RedisError, RedisPool};
use tracing::{debug, info};

use crate::repo::api_namespace::ApiNamespace;
use crate::service::api_definition::ApiDefinitionInfo;

const API_DEFINITION_REDIS_NAMESPACE: &str = "apidefinition";

#[async_trait]
pub trait ApiDeploymentRepo<Namespace: ApiNamespace> {
    async fn deploy(
        &self,
        deployment: &ApiDeployment<Namespace>,
    ) -> Result<(), ApiDeploymentRepoError>;

    async fn get(
        &self,
        host: &ApiSiteString,
    ) -> Result<Option<ApiDeployment<Namespace>>, ApiDeploymentRepoError>;

    async fn delete(&self, host: &ApiSiteString) -> Result<bool, ApiDeploymentRepoError>;

    async fn get_by_id(
        &self,
        namespace: &Namespace,
        api_id: &ApiDefinitionId,
    ) -> Result<Vec<ApiDeployment<Namespace>>, ApiDeploymentRepoError>;
}

pub enum ApiDeploymentRepoError {
    Internal(anyhow::Error),
}
impl From<RedisError> for ApiDeploymentRepoError {
    fn from(err: RedisError) -> Self {
        ApiDeploymentRepoError::Internal(anyhow::Error::new(err))
    }
}

pub struct InMemoryDeployment<Namespace> {
    deployments: Mutex<HashMap<ApiSiteString, ApiDeployment<Namespace>>>,
}

impl<Namespace> Default for InMemoryDeployment<Namespace> {
    fn default() -> Self {
        InMemoryDeployment {
            deployments: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl<Namespace: ApiNamespace> ApiDeploymentRepo<Namespace> for InMemoryDeployment<Namespace> {
    async fn deploy(
        &self,
        deployment: &ApiDeployment<Namespace>,
    ) -> Result<(), ApiDeploymentRepoError> {
        debug!(
            "Deploy API site: {}, ids: {}",
            deployment.site,
            deployment
                .api_definition_keys
                .iter()
                .map(|def| def.id.to_string())
                .collect::<Vec<String>>()
                .join(", ")
        );

        let key = deployment.site.clone();

        let mut deployments = self.deployments.lock().unwrap();

        let api_site_string = ApiSiteString::from(&key);

        let existing_deployment = deployments.get(&api_site_string);

        let new_deployment = if let Some(existing_deployment) = existing_deployment {
            let mut keys = existing_deployment.clone().api_definition_keys;
            keys.extend(deployment.api_definition_keys.clone());
            ApiDeployment {
                namespace: deployment.namespace.clone(),
                api_definition_keys: keys,
                site: deployment.site.clone(),
            }
        } else {
            deployment.clone()
        };

        deployments.insert(ApiSiteString::from(&key), new_deployment);

        Ok(())
    }

    async fn get(
        &self,
        host: &ApiSiteString,
    ) -> Result<Option<ApiDeployment<Namespace>>, ApiDeploymentRepoError> {
        debug!("Get API site: {}", host);
        let deployments = self.deployments.lock().unwrap();

        let deployment = deployments.get(host).cloned();

        Ok(deployment)
    }

    async fn delete(&self, host: &ApiSiteString) -> Result<bool, ApiDeploymentRepoError> {
        debug!("Delete API site: {}", host);
        let mut deployments = self.deployments.lock().unwrap();

        let deployment = deployments.remove(host);

        Ok(deployment.is_some())
    }

    async fn get_by_id(
        &self,
        namespace: &Namespace,
        api_id: &ApiDefinitionId,
    ) -> Result<Vec<ApiDeployment<Namespace>>, ApiDeploymentRepoError> {
        let registry = self.deployments.lock().unwrap();

        let result: Vec<ApiDeployment<Namespace>> = registry
            .values()
            .filter(|x| {
                x.namespace == *namespace
                    && x.api_definition_keys.iter().any(|key| &key.id == api_id)
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
    pub async fn new(config: &RedisConfig) -> Result<RedisApiDeploy, ApiDeploymentRepoError> {
        let pool = RedisPool::configured(config).await?;
        Ok(Self { pool })
    }
}

#[derive(
    Eq, Hash, PartialEq, Clone, Debug, serde::Deserialize, bincode::Encode, bincode::Decode,
)]
struct SiteMetadata<Namespace> {
    site: ApiSite,
    namespace: Namespace,
}

#[async_trait]
impl<Namespace: ApiNamespace> ApiDeploymentRepo<Namespace> for RedisApiDeploy {
    async fn deploy(
        &self,
        deployment: &ApiDeployment<Namespace>,
    ) -> Result<(), ApiDeploymentRepoError> {
        debug!(
            "Deploy API site: {}, id: {}",
            &deployment.site,
            &deployment
                .api_definition_keys
                .iter()
                .map(|def| def.id.to_string())
                .collect::<Vec<String>>()
                .join(", ")
        );

        // Store all definition ids for the site
        let site_key = redis_keys::api_deployment_redis_key(&ApiSiteString::from(&deployment.site));

        let mut api_definition_redis_values = vec![];

        for api_key in &deployment.api_definition_keys {
            let value = self
                .pool
                .serialize(api_key)
                .map_err(|e| ApiDeploymentRepoError::Internal(anyhow::Error::msg(e)))?;

            api_definition_redis_values.push((1.0, value));
        }

        self.pool
            .with("persistence", "deploy_deployment")
            .zadd(
                site_key,
                None,
                None,
                false,
                false,
                api_definition_redis_values,
            )
            .await?;

        // Store the metadata of the site
        let site_metadata_key =
            redis_keys::site_metadata_redis_key(&ApiSiteString::from(&deployment.site));

        let site_metadata_value = self
            .pool
            .serialize(&SiteMetadata {
                site: deployment.site.clone(),
                namespace: deployment.namespace.clone(),
            })
            .map_err(|e| ApiDeploymentRepoError::Internal(anyhow::Error::msg(e)))?;

        self.pool
            .with("persistence", "deploy_deployment")
            .set(site_metadata_key, site_metadata_value, None, None, false)
            .await?;

        // Store the reverse direction: API definition id to the list of sites it is deployed
        for api_definition_key in &deployment.api_definition_keys {
            let sites_key = redis_keys::api_deployments_redis_key(
                &deployment.namespace,
                &api_definition_key.id,
            );

            let site_value = self
                .pool
                .serialize(&deployment.site.to_string())
                .map_err(|e| ApiDeploymentRepoError::Internal(anyhow::Error::msg(e)))?;

            self.pool
                .with("persistence", "deploy_deployment")
                .zadd(sites_key, None, None, false, false, (1.0, site_value))
                .await
                .map_err(ApiDeploymentRepoError::from)?;
        }

        Ok(())
    }

    async fn get(
        &self,
        host: &ApiSiteString,
    ) -> Result<Option<ApiDeployment<Namespace>>, ApiDeploymentRepoError> {
        info!("Get host id: {}", host);

        let site_key = redis_keys::api_deployment_redis_key(host);

        let site_values: Vec<Bytes> = self
            .pool
            .with("persistence", "get_deployment")
            .zrange(&site_key, 0, -1, None, false, None, false)
            .await?;

        // Retrieve all the API definitions for the keys
        let mut api_definition_keys = Vec::new();

        for value in site_values {
            let api_definition_key: Result<ApiDefinitionInfo, ApiDeploymentRepoError> = self
                .pool
                .deserialize(&value)
                .map_err(|e| ApiDeploymentRepoError::Internal(anyhow::Error::msg(e.to_string())));

            api_definition_keys.push(api_definition_key?);
        }

        // Retrieve site metadata
        let site_metadata_key = redis_keys::site_metadata_redis_key(host);
        let site_metadata_value: Option<Bytes> = self
            .pool
            .with("persistence", "get_deployment")
            .get(site_metadata_key)
            .await?;

        // Deserialize site metadata to ApiSite
        let site_metadata: SiteMetadata<Namespace> = match site_metadata_value {
            Some(value) => self
                .pool
                .deserialize(&value)
                .map_err(|e| ApiDeploymentRepoError::Internal(anyhow::Error::msg(e)))?,
            None => return Ok(None),
        };

        // If zero API definitions found then it implies such a deployment never existed
        if api_definition_keys.is_empty() {
            return Ok(None);
        } else {
            Ok(Some(ApiDeployment {
                namespace: site_metadata.namespace,
                api_definition_keys,
                site: site_metadata.site,
            }))
        }
    }

    async fn delete(&self, host: &ApiSiteString) -> Result<bool, ApiDeploymentRepoError> {
        debug!("Delete API site: {}", host);

        let api_deployment: Option<ApiDeployment<Namespace>> = self.get(host).await?;

        let key = redis_keys::api_deployment_redis_key(host);

        match api_deployment {
            Some(value) => {
                for api_definition_key in &value.api_definition_keys {
                    let sites_key = redis_keys::api_deployments_redis_key(
                        &value.namespace,
                        &api_definition_key.id,
                    );

                    let site_value = self
                        .pool
                        .serialize(&value.site.to_string())
                        .map_err(|e| ApiDeploymentRepoError::Internal(anyhow::Error::msg(e)))?;

                    // Delete the site from the list of sites for the definition
                    let _ = self
                        .pool
                        .with("persistence", "delete_deployment")
                        .zrem(sites_key, site_value)
                        .await?;
                }

                let definition_delete: u32 = self
                    .pool
                    .with("persistence", "delete_deployment")
                    .del(key)
                    .await?;
                Ok(definition_delete > 0)
            }
            None => Ok(false),
        }
    }

    async fn get_by_id(
        &self,
        namespace: &Namespace,
        api_id: &ApiDefinitionId,
    ) -> Result<Vec<ApiDeployment<Namespace>>, ApiDeploymentRepoError> {
        let sites_key = redis_keys::api_deployments_redis_key(namespace, api_id);

        let site_values: Vec<Bytes> = self
            .pool
            .with("persistence", "get_deployment")
            .zrange(&sites_key, 0, -1, None, false, None, false)
            .await?;

        let mut sites = Vec::new();

        for value in site_values {
            let site: Result<String, ApiDeploymentRepoError> = self
                .pool
                .deserialize(&value)
                .map_err(|e| ApiDeploymentRepoError::Internal(anyhow::Error::msg(e.to_string())));

            sites.push(site?);
        }

        let mut deployments = Vec::new();

        for site in sites {
            if let Some(deployment) = self.get(&ApiSiteString(site)).await? {
                deployments.push(deployment);
            }
        }

        Ok(deployments)
    }
}

mod redis_keys {
    use crate::api_definition::{ApiDefinitionId, ApiSiteString};
    use crate::repo::api_deployment_repo::API_DEFINITION_REDIS_NAMESPACE;
    use crate::repo::api_namespace::ApiNamespace;

    pub(crate) fn api_deployment_redis_key(api_site: &ApiSiteString) -> String {
        format!("{}:deployment:{}", API_DEFINITION_REDIS_NAMESPACE, api_site)
    }

    pub(crate) fn site_metadata_redis_key(api_site: &ApiSiteString) -> String {
        format!(
            "{}:site_metadata:{}",
            API_DEFINITION_REDIS_NAMESPACE, api_site
        )
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
    use crate::api_definition::{
        ApiDefinitionId, ApiDeployment, ApiSite, ApiSiteString, ApiVersion,
    };

    use crate::auth::CommonNamespace;
    use crate::repo::api_deployment_repo::redis_keys::{
        api_deployment_redis_key, api_deployments_redis_key,
    };
    use crate::repo::api_deployment_repo::{ApiDeploymentRepo, InMemoryDeployment};
    use crate::service::api_definition::ApiDefinitionInfo;

    #[tokio::test]
    pub async fn test_in_memory_deploy() {
        let registry = InMemoryDeployment::default();

        let namespace = CommonNamespace::default();

        let site = ApiSite {
            host: "dev-api.golem.cloud".to_string(),
            subdomain: Some("test".to_string()),
        };

        let site_str = ApiSiteString::from(&site);

        let api_definition_id = ApiDefinitionId("api1".to_string());
        let version = ApiVersion("0.0.1".to_string());

        let deployment = ApiDeployment {
            namespace: namespace.clone(),
            api_definition_keys: vec![ApiDefinitionInfo {
                id: api_definition_id.clone(),
                version: version.clone(),
            }],
            site: site.clone(),
        };

        let _ = registry.deploy(&deployment).await;

        let result = registry.get(&site_str).await.unwrap_or(None);

        let result1 = registry
            .get_by_id(
                &CommonNamespace::default(),
                &deployment.api_definition_keys.first().unwrap().id,
            )
            .await
            .unwrap_or(vec![]);

        let delete = registry.delete(&site_str).await.unwrap_or(false);

        let result2 = registry.get(&site_str).await.unwrap_or(None);

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
            api_deployment_redis_key(&ApiSiteString("foo.dev-api.golem.cloud".to_string())),
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
