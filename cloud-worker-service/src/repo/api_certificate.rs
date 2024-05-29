use crate::model::{AccountCertificate, CertificateId};
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
pub trait ApiCertificateRepo {
    async fn create_or_update(
        &self,
        certificate: &AccountCertificate,
    ) -> Result<(), Box<dyn Error>>;

    async fn get(
        &self,
        account_id: &AccountId,
        project_id: &ProjectId,
        certificate_id: &CertificateId,
    ) -> Result<Option<AccountCertificate>, Box<dyn Error>>;

    async fn delete(
        &self,
        account_id: &AccountId,
        project_id: &ProjectId,
        certificate_id: &CertificateId,
    ) -> Result<bool, Box<dyn Error>>;

    async fn get_all(
        &self,
        account_id: &AccountId,
        project_id: &ProjectId,
    ) -> Result<Vec<AccountCertificate>, Box<dyn Error>>;
}

pub struct InMemoryApiCertificateRepo {
    registry: Mutex<HashMap<(AccountId, ProjectId, CertificateId), AccountCertificate>>,
}

impl Default for InMemoryApiCertificateRepo {
    fn default() -> Self {
        InMemoryApiCertificateRepo {
            registry: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl ApiCertificateRepo for InMemoryApiCertificateRepo {
    async fn create_or_update(
        &self,
        certificate: &AccountCertificate,
    ) -> Result<(), Box<dyn Error>> {
        let mut registry = self.registry.lock().unwrap();

        let key: (AccountId, ProjectId, CertificateId) = (
            certificate.account_id.clone(),
            certificate.certificate.project_id.clone(),
            certificate.certificate.id.clone(),
        );

        registry.insert(key, certificate.clone());

        Ok(())
    }

    async fn get(
        &self,
        account_id: &AccountId,
        project_id: &ProjectId,
        certificate_id: &CertificateId,
    ) -> Result<Option<AccountCertificate>, Box<dyn Error>> {
        let key: (AccountId, ProjectId, CertificateId) = (
            account_id.clone(),
            project_id.clone(),
            certificate_id.clone(),
        );
        let registry = self.registry.lock().unwrap();

        Ok(registry.get(&key).cloned())
    }

    async fn delete(
        &self,
        account_id: &AccountId,
        project_id: &ProjectId,
        certificate_id: &CertificateId,
    ) -> Result<bool, Box<dyn Error>> {
        let key: (AccountId, ProjectId, CertificateId) = (
            account_id.clone(),
            project_id.clone(),
            certificate_id.clone(),
        );

        let mut registry = self.registry.lock().unwrap();

        let result = registry.remove(&key);

        Ok(result.is_some())
    }

    async fn get_all(
        &self,
        account_id: &AccountId,
        project_id: &ProjectId,
    ) -> Result<Vec<AccountCertificate>, Box<dyn Error>> {
        let registry = self.registry.lock().unwrap();

        let result: Vec<AccountCertificate> = registry
            .values()
            .filter(|x| &x.account_id == account_id && &x.certificate.project_id == project_id)
            .cloned()
            .collect();

        Ok(result)
    }
}

pub struct RedisApiCertificateRepo {
    pool: RedisPool,
}

impl RedisApiCertificateRepo {
    pub async fn new(config: &RedisConfig) -> Result<RedisApiCertificateRepo, Box<dyn Error>> {
        let pool = golem_common::redis::RedisPool::configured(config).await?;
        Ok(Self { pool })
    }
}

#[async_trait]
impl ApiCertificateRepo for RedisApiCertificateRepo {
    async fn create_or_update(
        &self,
        certificate: &AccountCertificate,
    ) -> Result<(), Box<dyn Error>> {
        debug!(
            "Create or update - account: {}, project: {}, id: {}",
            certificate.account_id, certificate.certificate.project_id, certificate.certificate.id
        );
        let certificate_key = get_certificate_redis_key(
            &certificate.account_id,
            &certificate.certificate.project_id,
            &certificate.certificate.id,
        );

        let certificate_value = self
            .pool
            .serialize(certificate)
            .map_err(|e| e.to_string())?;

        self.pool
            .with("persistence", "register_certificate")
            .set(certificate_key, certificate_value, None, None, false)
            .await
            .map_err(|e| e.to_string())?;

        let project_key = get_project_certificate_redis_key(
            &certificate.account_id,
            &certificate.certificate.project_id,
        );

        let certificate_id_value = self
            .pool
            .serialize(&certificate.certificate.id.to_string())
            .map_err(|e| e.to_string())?;

        self.pool
            .with("persistence", "register_project_certificate")
            .sadd(project_key, certificate_id_value)
            .await
            .map_err(|e| e.to_string().into())
    }

    async fn get(
        &self,
        account_id: &AccountId,
        project_id: &ProjectId,
        certificate_id: &CertificateId,
    ) -> Result<Option<AccountCertificate>, Box<dyn Error>> {
        info!(
            "Get - account: {}, project: {}, id: {}",
            account_id, project_id, certificate_id
        );
        let key = get_certificate_redis_key(account_id, project_id, certificate_id);
        let value: Option<Bytes> = self
            .pool
            .with("persistence", "get_certificate")
            .get(key)
            .await
            .map_err(|e| e.to_string())?;

        match value {
            Some(value) => {
                let value: Result<AccountCertificate, Box<dyn Error>> = self
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
    ) -> Result<Vec<AccountCertificate>, Box<dyn Error>> {
        info!("Get - account: {}, project: {}", account_id, project_id);

        let project_key = get_project_certificate_redis_key(account_id, project_id);

        let project_ids: Vec<Bytes> = self
            .pool
            .with("persistence", "get_project_certificate_ids")
            .smembers(&project_key)
            .await
            .map_err(|e| e.to_string())?;

        let mut certificate_ids = Vec::new();

        for certificate_id_value in project_ids {
            let certificate_id_str: Result<String, Box<dyn Error>> = self
                .pool
                .deserialize(&certificate_id_value)
                .map_err(|e| e.to_string().into());

            let certificate_id: CertificateId = certificate_id_str
                .and_then(|c| c.parse::<CertificateId>().map_err(|e| e.to_string().into()))?;

            certificate_ids.push(certificate_id);
        }

        let mut certificates = Vec::new();

        for certificate_id in certificate_ids {
            let key = get_certificate_redis_key(account_id, project_id, &certificate_id);

            let value: Option<Bytes> = self
                .pool
                .with("persistence", "get_certificate")
                .get(&key)
                .await
                .map_err(|e| e.to_string())?;

            if let Some(value) = value {
                let certificate: Result<AccountCertificate, Box<dyn Error>> = self
                    .pool
                    .deserialize(&value)
                    .map_err(|e| e.to_string().into());

                certificates.push(certificate?);
            }
        }

        Ok(certificates)
    }

    async fn delete(
        &self,
        account_id: &AccountId,
        project_id: &ProjectId,
        certificate_id: &CertificateId,
    ) -> Result<bool, Box<dyn Error>> {
        debug!(
            "Delete - account: {}, project: {}, id: {}",
            account_id, project_id, certificate_id
        );
        let certificate_key = get_certificate_redis_key(account_id, project_id, certificate_id);

        let project_key = get_project_certificate_redis_key(account_id, project_id);

        let certificate_id_value = self
            .pool
            .serialize(&certificate_id.to_string())
            .map_err(|e| e.to_string())?;

        let _ = self
            .pool
            .with("persistence", "delete_project_certificate")
            .srem(project_key, certificate_id_value)
            .await
            .map_err(|e| e.to_string())?;

        let certificate_delete: u32 = self
            .pool
            .with("persistence", "delete_certificate")
            .del(certificate_key)
            .await
            .map_err(|e| e.to_string())?;

        Ok(certificate_delete > 0)
    }
}

fn get_certificate_redis_key(
    account_id: &AccountId,
    project_id: &ProjectId,
    certificate_id: &CertificateId,
) -> String {
    format!(
        "{}:certificate:{}:{}:{}",
        API_DEFINITION_REDIS_NAMESPACE, account_id, project_id, certificate_id
    )
}

fn get_project_certificate_redis_key(account_id: &AccountId, project_id: &ProjectId) -> String {
    format!(
        "{}:certificate:{}:{}",
        API_DEFINITION_REDIS_NAMESPACE, account_id, project_id
    )
}

#[cfg(test)]
mod tests {
    use golem_common::config::RedisConfig;
    use golem_common::model::AccountId;
    use golem_common::model::ProjectId;

    use crate::model::{AccountCertificate, Certificate, CertificateId};
    use crate::repo::api_certificate::{
        get_certificate_redis_key, ApiCertificateRepo, InMemoryApiCertificateRepo,
        RedisApiCertificateRepo,
    };

    #[tokio::test]
    pub async fn test_in_memory_registry() {
        let registry = InMemoryApiCertificateRepo::default();

        let account_id = AccountId::from("a1");

        let project_id = "15d70aa5-2e23-4ee3-b65c-4e1d702836a3"
            .parse::<ProjectId>()
            .unwrap();

        let certificate_id1 = "25d70aa5-2e23-4ee3-b65c-4e1d702836a3"
            .parse::<CertificateId>()
            .unwrap();

        let certificate1 = Certificate {
            id: certificate_id1.clone(),
            project_id: project_id.clone(),
            domain_name: "*.golem.test1".to_string(),
        };

        let certificate_id2 = "35d70aa5-2e23-4ee3-b65c-4e1d702836a3"
            .parse::<CertificateId>()
            .unwrap();

        let certificate2 = Certificate {
            id: certificate_id2.clone(),
            project_id: project_id.clone(),
            domain_name: "*.golem.test2".to_string(),
        };

        let account_certificate1 = AccountCertificate::new(&account_id, &certificate1, "arn1");

        registry
            .create_or_update(&account_certificate1)
            .await
            .unwrap();

        let account_certificate2 = AccountCertificate::new(&account_id, &certificate2, "arn2");

        registry
            .create_or_update(&account_certificate2)
            .await
            .unwrap();

        let certificate1_result1 = registry
            .get(&account_id, &project_id, &certificate_id1)
            .await
            .unwrap_or(None);

        let certificate2_result1 = registry
            .get(&account_id, &project_id, &certificate_id2)
            .await
            .unwrap_or(None);

        let certificate_result2 = registry
            .get_all(&account_id, &project_id)
            .await
            .unwrap_or(vec![]);

        let delete1_result = registry
            .delete(&account_id, &project_id, &certificate_id1)
            .await
            .unwrap_or(false);

        let certificate1_result3 = registry
            .get(&account_id, &project_id, &certificate_id1)
            .await
            .unwrap_or(None);

        let certificate_result3 = registry
            .get_all(&account_id, &project_id)
            .await
            .unwrap_or(vec![]);

        let delete2_result = registry
            .delete(&account_id, &project_id, &certificate_id2)
            .await
            .unwrap_or(false);

        let certificate2_result3 = registry
            .get(&account_id, &project_id, &certificate_id2)
            .await
            .unwrap_or(None);

        let certificate_result4 = registry
            .get_all(&account_id, &project_id)
            .await
            .unwrap_or(vec![]);

        assert!(certificate1_result1.is_some());
        assert!(!certificate_result2.is_empty());
        assert!(certificate2_result1.is_some());
        assert_eq!(certificate1_result1.unwrap(), account_certificate1);
        assert_eq!(certificate_result2.len(), 2);
        assert!(delete1_result);
        assert!(delete2_result);
        assert!(certificate1_result3.is_none());
        assert!(certificate2_result3.is_none());
        assert_eq!(certificate_result3[0], account_certificate2);
        assert!(certificate_result4.is_empty());
    }

    #[tokio::test]
    #[ignore]
    pub async fn test_redis_registry() {
        let config = RedisConfig {
            key_prefix: "registry_test:".to_string(),
            database: 1,
            ..Default::default()
        };

        let registry = RedisApiCertificateRepo::new(&config).await.unwrap();

        let account_id = AccountId::from("a1");

        let project_id = "15d70aa5-2e23-4ee3-b65c-4e1d702836a3"
            .parse::<ProjectId>()
            .unwrap();

        let certificate_id1 = "25d70aa5-2e23-4ee3-b65c-4e1d702836a3"
            .parse::<CertificateId>()
            .unwrap();

        let certificate1 = Certificate {
            id: certificate_id1.clone(),
            project_id: project_id.clone(),
            domain_name: "*.golem.test1".to_string(),
        };

        let certificate_id2 = "35d70aa5-2e23-4ee3-b65c-4e1d702836a3"
            .parse::<CertificateId>()
            .unwrap();

        let certificate2 = Certificate {
            id: certificate_id2.clone(),
            project_id: project_id.clone(),
            domain_name: "*.golem.test2".to_string(),
        };

        let account_certificate1 = AccountCertificate::new(&account_id, &certificate1, "arn1");

        registry
            .create_or_update(&account_certificate1)
            .await
            .unwrap();

        let account_certificate2 = AccountCertificate::new(&account_id, &certificate2, "arn2");

        registry
            .create_or_update(&account_certificate2)
            .await
            .unwrap();

        let certificate1_result1 = registry
            .get(&account_id, &project_id, &certificate_id1)
            .await
            .unwrap_or(None);

        let certificate2_result1 = registry
            .get(&account_id, &project_id, &certificate_id2)
            .await
            .unwrap_or(None);

        let certificate_result2 = registry
            .get_all(&account_id, &project_id)
            .await
            .unwrap_or(vec![]);

        let delete1_result = registry
            .delete(&account_id, &project_id, &certificate_id1)
            .await
            .unwrap_or(false);

        let certificate1_result3 = registry
            .get(&account_id, &project_id, &certificate_id1)
            .await
            .unwrap_or(None);

        let certificate_result3 = registry
            .get_all(&account_id, &project_id)
            .await
            .unwrap_or(vec![]);

        let delete2_result = registry
            .delete(&account_id, &project_id, &certificate_id2)
            .await
            .unwrap_or(false);

        let certificate2_result3 = registry
            .get(&account_id, &project_id, &certificate_id2)
            .await
            .unwrap_or(None);

        let certificate_result4 = registry
            .get_all(&account_id, &project_id)
            .await
            .unwrap_or(vec![]);

        assert!(certificate1_result1.is_some());
        assert!(!certificate_result2.is_empty());
        assert!(certificate2_result1.is_some());
        assert_eq!(certificate1_result1.unwrap(), account_certificate1);
        assert_eq!(certificate_result2.len(), 2);
        assert!(delete1_result);
        assert!(delete2_result);
        assert!(certificate1_result3.is_none());
        assert!(certificate2_result3.is_none());
        assert_eq!(certificate_result3[0], account_certificate2);
        assert!(certificate_result4.is_empty());
    }

    #[test]
    pub fn test_get_certificate_redis_key() {
        let account_id = AccountId::from("a1");
        let project_id = "15d70aa5-2e23-4ee3-b65c-4e1d702836a3"
            .parse::<ProjectId>()
            .unwrap();
        let certificate_id = "25d70aa5-2e23-4ee3-b65c-4e1d702836a3"
            .parse::<CertificateId>()
            .unwrap();

        assert_eq!(
            get_certificate_redis_key(&account_id, &project_id, &certificate_id),
            "apidefinition:certificate:a1:15d70aa5-2e23-4ee3-b65c-4e1d702836a3:25d70aa5-2e23-4ee3-b65c-4e1d702836a3"
        );
    }
}
