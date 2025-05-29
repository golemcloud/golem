use crate::aws_config::AwsConfig;
use crate::aws_load_balancer::AwsLoadBalancer;
use crate::config::DomainRecordsConfig;
use crate::model::{CertificateId, CertificateRequest};
use crate::repo::api_certificate::{ApiCertificateRepo, CertificateRecord};
use crate::service::api_certificate::CertificateServiceError::InternalConversionError;
use crate::service::auth::AuthService;
use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use cloud_common::auth::{CloudAuthCtx, CloudNamespace};
use cloud_common::clients::auth::AuthServiceError;
use cloud_common::model::ProjectAction;
use derive_more::Display;
use golem_common::model::AccountId;
use golem_common::model::ProjectId;
use golem_common::model::RetryConfig;
use golem_common::retries::with_retries;
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use rusoto_acm::{
    Acm, AcmClient, DeleteCertificateError, DeleteCertificateRequest, DescribeCertificateRequest,
    ImportCertificateRequest, ListTagsForCertificateRequest, Tag,
};
use rusoto_core::RusotoError;
use rusoto_elbv2::{
    AddListenerCertificatesInput, Certificate, Elb, ElbClient, RemoveListenerCertificatesInput,
};
use std::error::Error;
use std::fmt::Debug;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tracing::info;
use x509_certificate::X509Certificate;

const AWS_ACCOUNT_TAG_NAME: &str = "account";
const AWS_WORKSPACE_TAG_NAME: &str = "workspace";
const AWS_ENVIRONMENT_TAG_NAME: &str = "environment";

#[derive(Debug, thiserror::Error)]
pub enum CertificateServiceError {
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Certificate Not Found: {0}")]
    CertificateNotFound(CertificateId),
    #[error("Certificate Not Available: {0}")]
    CertificateNotAvailable(String),
    #[error("Internal certificate manager error: {0}")]
    InternalCertificateManagerError(String),
    #[error("Internal auth client error: {0}")]
    InternalAuthClientError(String),
    #[error("Internal repository error: {0}")]
    InternalRepoError(RepoError),
    #[error("Internal error: {0}")]
    InternalConversionError(String),
}

impl SafeDisplay for CertificateServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            CertificateServiceError::Unauthorized(_) => self.to_string(),
            CertificateServiceError::CertificateNotFound(_) => self.to_string(),
            CertificateServiceError::CertificateNotAvailable(_) => self.to_string(),
            CertificateServiceError::InternalCertificateManagerError(_) => self.to_string(),
            CertificateServiceError::InternalAuthClientError(_) => self.to_string(),
            CertificateServiceError::InternalRepoError(inner) => inner.to_safe_string(),
            InternalConversionError(_) => self.to_string(),
        }
    }
}

impl From<CertificateManagerError> for CertificateServiceError {
    fn from(value: CertificateManagerError) -> Self {
        match value {
            CertificateManagerError::NotAvailable(error) => {
                CertificateServiceError::CertificateNotAvailable(error)
            }
            CertificateManagerError::Internal(error) => {
                CertificateServiceError::InternalCertificateManagerError(error)
            }
        }
    }
}

impl From<AuthServiceError> for CertificateServiceError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::Unauthorized(error) => CertificateServiceError::Unauthorized(error),
            AuthServiceError::Forbidden(error) => CertificateServiceError::Unauthorized(error),
            AuthServiceError::InternalClientError(error) => {
                CertificateServiceError::InternalAuthClientError(error)
            }
        }
    }
}

impl From<RepoError> for CertificateServiceError {
    fn from(value: RepoError) -> Self {
        CertificateServiceError::InternalRepoError(value)
    }
}

#[async_trait]
pub trait CertificateService {
    async fn create(
        &self,
        request: &CertificateRequest,
        auth: &CloudAuthCtx,
    ) -> Result<crate::model::Certificate, CertificateServiceError>;

    async fn delete(
        &self,
        project_id: &ProjectId,
        certificate_id: &CertificateId,
        auth: &CloudAuthCtx,
    ) -> Result<(), CertificateServiceError>;

    async fn get(
        &self,
        project_id: ProjectId,
        certificate_id: Option<CertificateId>,
        auth: &CloudAuthCtx,
    ) -> Result<Vec<crate::model::Certificate>, CertificateServiceError>;
}

pub struct CertificateServiceDefault {
    auth_service: Arc<dyn AuthService + Sync + Send>,
    certificate_manager: Arc<dyn CertificateManager + Sync + Send>,
    certificate_repo: Arc<dyn ApiCertificateRepo + Sync + Send>,
}

impl CertificateServiceDefault {
    pub fn new(
        auth_service: Arc<dyn AuthService + Sync + Send>,
        certificate_manager: Arc<dyn CertificateManager + Sync + Send>,
        certificate_repo: Arc<dyn ApiCertificateRepo + Sync + Send>,
    ) -> Self {
        Self {
            auth_service,
            certificate_manager,
            certificate_repo,
        }
    }

    async fn is_authorized(
        &self,
        project_id: &ProjectId,
        permission: ProjectAction,
        auth: &CloudAuthCtx,
    ) -> Result<CloudNamespace, CertificateServiceError> {
        self.auth_service
            .authorize_project_action(project_id, permission, auth)
            .await
            .map_err(|e| e.into())
    }
}

#[async_trait]
impl CertificateService for CertificateServiceDefault {
    async fn create(
        &self,
        request: &CertificateRequest,
        auth: &CloudAuthCtx,
    ) -> Result<crate::model::Certificate, CertificateServiceError> {
        let project_id = &request.project_id;
        let namespace = self
            .is_authorized(project_id, ProjectAction::CreateApiDefinition, auth)
            .await?;
        info!(
            namespace = %namespace,
            "Create API certificate - domain name: {}",
            request.domain_name
        );

        let created_at = Utc::now();
        let account_id = namespace.account_id;
        let certificate_body = request.certificate_body.clone().leak();
        let certificate_private_key = request.certificate_private_key.clone().leak();

        let certificate_id = self
            .certificate_manager
            .import(
                &account_id,
                &request.domain_name,
                certificate_body,
                certificate_private_key,
                None,
            )
            .await?;

        let record =
            CertificateRecord::new(account_id, request.clone(), certificate_id, created_at);

        self.certificate_repo.create_or_update(&record).await?;

        let certificate = record.try_into().map_err(|e| {
            InternalConversionError(format!("Failed to convert Certificate record: {e}"))
        })?;

        Ok(certificate)
    }

    async fn delete(
        &self,
        project_id: &ProjectId,
        certificate_id: &CertificateId,
        auth: &CloudAuthCtx,
    ) -> Result<(), CertificateServiceError> {
        let namespace = self
            .is_authorized(project_id, ProjectAction::DeleteApiDefinition, auth)
            .await?;

        info!(
            namespace = %namespace,
            "Delete API certificate -id: {}",
            certificate_id
        );
        let account_id = namespace.account_id.clone();

        let data = self
            .certificate_repo
            .get(&namespace.to_string(), &certificate_id.0)
            .await?;

        if let Some(certificate) = data {
            let _ = self
                .certificate_manager
                .delete(&account_id, &certificate.external_id)
                .await?;

            let _ = self
                .certificate_repo
                .delete(&namespace.to_string(), &certificate_id.0)
                .await?;
            Ok(())
        } else {
            Err(CertificateServiceError::CertificateNotFound(
                certificate_id.clone(),
            ))
        }
    }

    async fn get(
        &self,
        project_id: ProjectId,
        certificate_id: Option<CertificateId>,
        auth: &CloudAuthCtx,
    ) -> Result<Vec<crate::model::Certificate>, CertificateServiceError> {
        let namespace = self
            .is_authorized(&project_id, ProjectAction::ViewApiDefinition, auth)
            .await?;
        let records = if let Some(certificate_id) = certificate_id {
            info!(
                namespace = %namespace,
                "Get API certificate - id: {}",
                certificate_id
            );

            let data = self
                .certificate_repo
                .get(&namespace.to_string(), &certificate_id.0)
                .await?;

            match data {
                Some(d) => vec![d],
                None => vec![],
            }
        } else {
            info!(
                namespace = %namespace,
                "Get API certificates"
            );

            self.certificate_repo
                .get_all(&namespace.to_string())
                .await?
        };

        let values: Vec<crate::model::Certificate> = records
            .iter()
            .map(|d| d.clone().try_into())
            .collect::<Result<Vec<crate::model::Certificate>, _>>()
            .map_err(|e| {
                InternalConversionError(format!("Failed to convert Certificate record: {e}"))
            })?;

        Ok(values)
    }
}

#[derive(Debug, Clone)]
pub struct CertificateDetail {
    pub id: String,
    pub domain_name: String,
    pub in_use_by: Vec<String>,
    pub account_id: Option<AccountId>,
    pub environment: Option<String>,
    pub workspace: Option<String>,
}

impl CertificateDetail {
    pub fn is_for(&self, account_id: &AccountId, environment: &str, workspace: &str) -> bool {
        self.account_id
            .clone()
            .is_some_and(|v| v.value == account_id.value)
            && self.environment.clone().is_some_and(|v| v == environment)
            && self.workspace.clone().is_some_and(|v| v == workspace)
    }
}

#[derive(Debug, Clone, PartialEq, Display)]
pub enum CertificateManagerError {
    NotAvailable(String),
    Internal(String),
}

impl CertificateManagerError {
    pub fn internal(error: Box<dyn Error>) -> Self {
        CertificateManagerError::Internal(error.to_string())
    }
}

#[async_trait]
pub trait CertificateManager {
    async fn import(
        &self,
        account_id: &AccountId,
        domain_name: &str,
        certificate_body: &'static str,
        certificate_private_key: &'static str,
        external_id: Option<String>,
    ) -> Result<String, CertificateManagerError>;

    async fn unregister(
        &self,
        account_id: &AccountId,
        certificate_id: &str,
    ) -> Result<bool, CertificateManagerError>;

    async fn get(
        &self,
        account_id: &AccountId,
        certificate_id: &str,
    ) -> Result<Option<CertificateDetail>, CertificateManagerError>;

    async fn delete(
        &self,
        account_id: &AccountId,
        certificate_id: &str,
    ) -> Result<bool, CertificateManagerError>;
}

#[derive(Default)]
pub struct InMemoryCertificateManager {
    pub domain_records_config: DomainRecordsConfig,
}

#[async_trait]
impl CertificateManager for InMemoryCertificateManager {
    async fn import(
        &self,
        _account_id: &AccountId,
        domain_name: &str,
        _certificate_body: &'static str,
        _certificate_private_key: &'static str,
        _external_id: Option<String>,
    ) -> Result<String, CertificateManagerError> {
        info!("Import certificate - domain name: {}", domain_name);

        if !self
            .domain_records_config
            .is_domain_available_for_registration(domain_name)
        {
            return Err(CertificateManagerError::NotAvailable(
                "API domain is not available".to_string(),
            ));
        }
        Ok("".to_string())
    }

    async fn unregister(
        &self,
        _account_id: &AccountId,
        certificate_id: &str,
    ) -> Result<bool, CertificateManagerError> {
        info!("Unregister certificate - id: {}", certificate_id);

        Ok(false)
    }

    async fn get(
        &self,
        _account_id: &AccountId,
        certificate_id: &str,
    ) -> Result<Option<CertificateDetail>, CertificateManagerError> {
        info!("Get certificate - id: {}", certificate_id);

        Ok(None)
    }

    async fn delete(
        &self,
        _account_id: &AccountId,
        certificate_id: &str,
    ) -> Result<bool, CertificateManagerError> {
        info!("Delete certificate - id: {}", certificate_id);

        Ok(false)
    }
}

#[derive(Debug, Clone)]
pub struct AwsCertificateManager {
    pub load_balancer: AwsLoadBalancer,
    pub aws_config: AwsConfig,
    pub domain_records_config: DomainRecordsConfig,
    pub environment: String,
    pub workspace: String,
}

impl AwsCertificateManager {
    pub async fn new(
        environment: &str,
        workspace: &str,
        config: &AwsConfig,
        domain_records_config: &DomainRecordsConfig,
    ) -> Result<AwsCertificateManager, Box<dyn Error>> {
        let load_balancer = AwsLoadBalancer::new(environment, workspace, config).await?;

        Ok(AwsCertificateManager {
            load_balancer,
            aws_config: config.clone(),
            domain_records_config: domain_records_config.clone(),
            environment: environment.to_string(),
            workspace: workspace.to_string(),
        })
    }
}

#[async_trait]
impl CertificateManager for AwsCertificateManager {
    async fn import(
        &self,
        account_id: &AccountId,
        domain_name: &str,
        certificate_body: &'static str,
        certificate_private_key: &'static str,
        external_id: Option<String>,
    ) -> Result<String, CertificateManagerError> {
        info!("Import certificate - domain name: {}", domain_name);

        if !self
            .domain_records_config
            .is_domain_available_for_registration(domain_name)
        {
            return Err(CertificateManagerError::NotAvailable(
                "API domain is not available".to_string(),
            ));
        }

        check_certificate(domain_name, certificate_body)
            .map_err(CertificateManagerError::internal)?;

        let client: AcmClient = self
            .aws_config
            .clone()
            .try_into()
            .map_err(CertificateManagerError::internal)?;

        let certificate_id = import_certificate(
            account_id,
            certificate_body,
            certificate_private_key,
            external_id,
            &self.environment,
            &self.workspace,
            &client,
        )
        .await
        .map_err(CertificateManagerError::internal)?;

        info!(
            "Import certificate - domain name: {}, id: {}",
            domain_name, certificate_id
        );

        let elb_client: ElbClient = self
            .aws_config
            .clone()
            .try_into()
            .map_err(CertificateManagerError::internal)?;

        add_certificate_to_load_balancer(&certificate_id, &self.load_balancer, &elb_client)
            .await
            .map_err(CertificateManagerError::internal)?;

        Ok(certificate_id)
    }

    async fn unregister(
        &self,
        account_id: &AccountId,
        certificate_id: &str,
    ) -> Result<bool, CertificateManagerError> {
        info!("Unregister certificate - id: {}", certificate_id);

        let client: AcmClient = self
            .aws_config
            .clone()
            .try_into()
            .map_err(CertificateManagerError::internal)?;

        let certificate = get_certificate(certificate_id, &client)
            .await
            .map_err(CertificateManagerError::internal)?;

        if certificate
            .clone()
            .is_some_and(|c| c.is_for(account_id, &self.environment, &self.workspace))
        {
            let elb_client: ElbClient = self
                .aws_config
                .clone()
                .try_into()
                .map_err(CertificateManagerError::internal)?;

            remove_certificate_from_load_balancer(certificate_id, &self.load_balancer, &elb_client)
                .await
                .map_err(CertificateManagerError::internal)?;

            return Ok(true);
        }

        Ok(false)
    }

    async fn get(
        &self,
        account_id: &AccountId,
        certificate_id: &str,
    ) -> Result<Option<CertificateDetail>, CertificateManagerError> {
        info!("Get certificate - id: {}", certificate_id);

        let client: AcmClient = self
            .aws_config
            .clone()
            .try_into()
            .map_err(CertificateManagerError::internal)?;

        let certificate = get_certificate(certificate_id, &client)
            .await
            .map_err(CertificateManagerError::internal)?;

        if certificate
            .clone()
            .is_some_and(|c| c.is_for(account_id, &self.environment, &self.workspace))
        {
            return Ok(certificate);
        }

        Ok(None)
    }

    async fn delete(
        &self,
        account_id: &AccountId,
        certificate_id: &str,
    ) -> Result<bool, CertificateManagerError> {
        info!("Delete certificate - id: {}", certificate_id);

        let client: AcmClient = self
            .aws_config
            .clone()
            .try_into()
            .map_err(CertificateManagerError::internal)?;

        let certificate = get_certificate(certificate_id, &client)
            .await
            .map_err(CertificateManagerError::internal)?;

        if certificate
            .clone()
            .is_some_and(|c| c.is_for(account_id, &self.environment, &self.workspace))
        {
            let elb_client: ElbClient = self
                .aws_config
                .clone()
                .try_into()
                .map_err(CertificateManagerError::internal)?;

            remove_certificate_from_load_balancer(certificate_id, &self.load_balancer, &elb_client)
                .await
                .map_err(CertificateManagerError::internal)?;

            // look like there is eventual consistency - need to wait before delete
            sleep(Duration::from_secs(3)).await;

            let retry_config = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(800),
                max_delay: Duration::from_secs(2),
                multiplier: 2.0,
                max_jitter_factor: Some(0.15),
            };

            let _ = with_retries(
                "certificate",
                "delete",
                Some(format!("{account_id} - {certificate_id}")),
                &retry_config,
                &(client.clone(), certificate_id),
                |(client, certificate_id)| {
                    Box::pin(async move {
                        client
                            .delete_certificate(DeleteCertificateRequest {
                                certificate_arn: certificate_id.to_string(),
                            })
                            .await
                    })
                },
                |e| {
                    matches!(
                        e,
                        RusotoError::Service(DeleteCertificateError::ResourceInUse { .. })
                    )
                },
            )
            .await
            .map_err(|e| CertificateManagerError::internal(e.into()))?;

            return Ok(true);
        }

        Ok(false)
    }
}

fn check_certificate(
    domain_name: &str,
    certificate_body: &'static str,
) -> Result<(), Box<dyn Error>> {
    let cert = X509Certificate::from_pem(certificate_body.as_bytes());

    match cert {
        Ok(cert) => {
            if let Some(cn) = cert.subject_common_name() {
                if cn != domain_name {
                    let detail = format!("Certificate does not match domain name (domain name: {}, certificate common name: {})", domain_name, cn);
                    return Err(detail.into());
                }
            }
        }
        Err(err) => {
            let detail = format!("Certificate decode error: {}", err);
            return Err(detail.into());
        }
    }
    Ok(())
}

async fn import_certificate(
    account_id: &AccountId,
    certificate_body: &'static str,
    certificate_private_key: &'static str,
    certificate_arn: Option<String>,
    environment: &str,
    workspace: &str,
    client: &AcmClient,
) -> Result<String, Box<dyn Error>> {
    let tags = vec![
        Tag {
            key: AWS_ENVIRONMENT_TAG_NAME.to_string(),
            value: Some(environment.to_string()),
        },
        Tag {
            key: AWS_WORKSPACE_TAG_NAME.to_string(),
            value: Some(workspace.to_string()),
        },
        Tag {
            key: AWS_ACCOUNT_TAG_NAME.to_string(),
            value: Some(account_id.value.clone()),
        },
    ];

    let result = client
        .import_certificate(ImportCertificateRequest {
            certificate: Bytes::from(certificate_body),
            private_key: Bytes::from(certificate_private_key),
            certificate_arn,
            tags: Some(tags),
            ..Default::default()
        })
        .await?;

    let arn = result.certificate_arn.unwrap();

    Ok(arn)
}

async fn get_certificate(
    certificate_arn: &str,
    client: &AcmClient,
) -> Result<Option<CertificateDetail>, Box<dyn Error>> {
    let cert_result = client
        .describe_certificate(DescribeCertificateRequest {
            certificate_arn: certificate_arn.to_string(),
        })
        .await?;

    if let Some(cert) = cert_result.certificate {
        let tags_result = client
            .list_tags_for_certificate(ListTagsForCertificateRequest {
                certificate_arn: certificate_arn.to_string(),
            })
            .await?;

        let mut account_id: Option<AccountId> = None;
        let mut environment: Option<String> = None;
        let mut workspace: Option<String> = None;

        if let Some(tags) = tags_result.tags {
            for tag in tags {
                if tag.key == AWS_ACCOUNT_TAG_NAME {
                    account_id = tag.value.map(|v| AccountId::from(v.as_str()));
                } else if tag.key == AWS_ENVIRONMENT_TAG_NAME {
                    environment = tag.value;
                } else if tag.key == AWS_WORKSPACE_TAG_NAME {
                    workspace = tag.value;
                }
            }
        }

        let cert_detail = CertificateDetail {
            id: cert.certificate_arn.unwrap(),
            domain_name: cert.domain_name.unwrap_or("N/A".to_string()),
            in_use_by: cert.in_use_by.unwrap_or(vec![]),
            account_id,
            environment,
            workspace,
        };

        Ok(Some(cert_detail))
    } else {
        Ok(None)
    }
}

async fn add_certificate_to_load_balancer(
    certificate_arn: &str,
    load_balancer: &AwsLoadBalancer,
    client: &ElbClient,
) -> Result<(), Box<dyn Error>> {
    let https_listener = load_balancer
        .get_https_listener()
        .ok_or("HTTPS listener not found")?;
    let _ = client
        .add_listener_certificates(AddListenerCertificatesInput {
            listener_arn: https_listener.arn,
            certificates: vec![Certificate {
                certificate_arn: Some(certificate_arn.to_string()),
                is_default: None,
            }],
        })
        .await?;
    Ok(())
}

async fn remove_certificate_from_load_balancer(
    certificate_arn: &str,
    load_balancer: &AwsLoadBalancer,
    client: &ElbClient,
) -> Result<(), Box<dyn Error>> {
    let https_listener = load_balancer
        .get_https_listener()
        .ok_or("HTTPS listener not found")?;
    let _ = client
        .remove_listener_certificates(RemoveListenerCertificatesInput {
            listener_arn: https_listener.arn,
            certificates: vec![Certificate {
                certificate_arn: Some(certificate_arn.to_string()),
                is_default: None,
            }],
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::aws_config::AwsConfig;
    use crate::config::DomainRecordsConfig;
    use crate::service::api_certificate::{AwsCertificateManager, CertificateManager};
    use golem_common::model::AccountId;

    fn get_certificate_body() -> &'static str {
        r#"
-----BEGIN CERTIFICATE-----
MIIEGjCCAwKgAwIBAgIUcop9eRNiTC5N/IAaxLZfLkCwiCEwDQYJKoZIhvcNAQEL
BQAwgYsxCzAJBgNVBAYTAlVTMQswCQYDVQQIDAJOWTEOMAwGA1UECgwFZ29sZW0x
DjAMBgNVBAcMBWdvbGVtMRkwFwYDVQQDDBBnb2xlbS5jbG91ZC50ZXN0MQ4wDAYD
VQQLDAVnb2xlbTEkMCIGCSqGSIb3DQEJARYVcGV0by5rb3R1bGFAeWFob28uY29t
MB4XDTIzMTAxMTIwMzc1NFoXDTMzMTAwODIwMzc1NFowgYsxCzAJBgNVBAYTAlVT
MQswCQYDVQQIDAJOWTEOMAwGA1UECgwFZ29sZW0xDjAMBgNVBAcMBWdvbGVtMRkw
FwYDVQQDDBBnb2xlbS5jbG91ZC50ZXN0MQ4wDAYDVQQLDAVnb2xlbTEkMCIGCSqG
SIb3DQEJARYVcGV0by5rb3R1bGFAeWFob28uY29tMIIBIjANBgkqhkiG9w0BAQEF
AAOCAQ8AMIIBCgKCAQEAnGIhc3W/arfeDzSu4nO6Kq29aLnYch8XCyRq5qq5R5bp
aqrZHtho5zOdML4JcUSkuq3k7eE+yCVz3vTMYvSyQ8UU7j+Dud4y8tg+4oO+RHjI
UmSXi+Jyn7fEaJLBVj0rVHJddcES2WkpwAgkk+/8GqHiu/Zsx+vRTyHYka+lZFiW
ASO8g1nbiGveS9fWwfPbBV/cZVHV3vZtlG7v/TT5clOsnpxhSWsA65ojwjoPMNws
PUPx25enKsSbGiDW0JIqSfOZoUUZ5fD1wwVvRgfSndiVOhUZhVJtPv1KJyUigXLu
a/8+w44mCiPEwnbhDW+uZHCrS1ZrxkfVgyCWi/J1CQIDAQABo3QwcjALBgNVHQ8E
BAMCBDAwEwYDVR0lBAwwCgYIKwYBBQUHAwEwLwYDVR0RBCgwJoIQZ29sZW0uY2xv
dWQudGVzdIISKi5nb2xlbS5jbG91ZC50ZXN0MB0GA1UdDgQWBBQdR9stXKG0q03s
0lszl+yNhGiapDANBgkqhkiG9w0BAQsFAAOCAQEAhTLkV1tm6ynO8ro1W+b5WvWU
EWVd6CNT/5y3Ins3xROsovGYMwWV/NQWyXyOnJR/gvBTQkZga44BVMiPLtrgVQh6
KiWxBh7akT+/EEFpzq1h8P/sSIBwoUEG+IonKQHvaNXyThE/IiHW358tACdsohIa
a+oE58nmxrQbHCPcip1IN1wgIun9+CcMLjYlZL2V6YGVhh8tCfZaOZdzFe1MMh+Q
fkyPNkedMQ8ZvCeVMkeYN4zAohd1Am5oR+imuVZ8V7Moy2AZ0gh/OVbK9fEwvYpd
WCik7I7A/POfxFicLGmOya7GO9xKsEMRv3FPDZ7SOzRx4FgQDWGkqYz13Iv2Bg==
-----END CERTIFICATE-----
        "#
    }

    fn get_certificate_primary_key() -> &'static str {
        r#"
-----BEGIN PRIVATE KEY-----
MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQCcYiFzdb9qt94P
NK7ic7oqrb1oudhyHxcLJGrmqrlHlulqqtke2GjnM50wvglxRKS6reTt4T7IJXPe
9Mxi9LJDxRTuP4O53jLy2D7ig75EeMhSZJeL4nKft8RoksFWPStUcl11wRLZaSnA
CCST7/waoeK79mzH69FPIdiRr6VkWJYBI7yDWduIa95L19bB89sFX9xlUdXe9m2U
bu/9NPlyU6yenGFJawDrmiPCOg8w3Cw9Q/Hbl6cqxJsaINbQkipJ85mhRRnl8PXD
BW9GB9Kd2JU6FRmFUm0+/UonJSKBcu5r/z7DjiYKI8TCduENb65kcKtLVmvGR9WD
IJaL8nUJAgMBAAECggEABTAeOE8yM/n4lHGPfF9m0C6mXM3kN9j/Md/vP0rE+HKH
Di0FJJ2nppqS6ZXhs0/Ef+rv8CerplvTuGmk9C12UoLnGwZMD5j8VgvFfkyXkybx
97By9Jw6aEEM84PW2nkPXG7/hGi3gDNEbs9LrgVlTC7jBOWmczVyuAOBRJ1annW3
8Z16vtKbI6+Sx47FDuEFgC2L+SI/aZFpFyb/+wKFwKhk55uiLSn0Tu0jTTkE+8ud
nE/vmpfaxRLkXM14IDK3S3h/+tEc2HjlluYyDGOnF2U3ZCrh6UWjHd0at32atL3z
Iy3+laJ9qAS1xxH2JRXwuJRCdXSyxVcMFMbMtWTrgQKBgQDJH3gjciubjEvGvBWZ
b1u1UqHqYrBQIVy4FZkp1QLUTjwjCM/LNwB2J+GBBwmblCWq842NP6nxr5zlXLYu
GZB69FG1LeIo8D/RhnWgnGBYq+gAOsbaeJPC5SgzFWTegy1wR1X9AvqfZA2CQTFb
n8prEiH7q2NL63NPkOvYNFm2oQKBgQDHDZQ94FVdpGYisVmRsJ/91lXC6P7Anac2
JT031wJtTFgJEJqg2ZaVGdvkuYrRM/dGy9ENgz74T33PP5DRODFfewV46T1XaJp3
1z8tSA7hJN44Fuu4/0HUCHGVxdNlVWKmLmsnYan2LEtPEiFUAjErFUcgTV8XNy8t
cPlSmeRtaQKBgFzCSNSASaB5+lD0Wjnj5DYioE7LqWmrmWnFfFiQx7dHRfEalUuy
WGImTpkFt+arUxwfLD/jBuxTBFe8hMGKRNqQaEbZnJ8o/yYRj5q9xKngzyWb9i64
wd13dyzoRxdhBMnt/LiucQymRpy2mJ8beW4cdNPv3eIb+5jMzBlxO5dBAoGAcUM1
xdufV3BTOYxmpfK1pu9Nz2Fai+lpGvMnmV17oQue0FGlWr9U4rRbHhPBfHawTpVs
995ld09sDABke9gYp/bNT1aQM+tucaCF71MgPFYJKCtKp/J+15KSZyGwvulN/7dL
+5Wj61Ka63wqgK3aomQyG5xK7l/VNWsiQzET/HkCgYAqw1T3fC1Qm1iBCnvg/9FK
rEO/AaZ/Ppaqjxs+/qtmoY9Y+1ei3nEH2pkrr2km5N5pI/35/7bBLHPBTb7AlAX1
5E0xiH6mtD3qYv96Hr4LWmby256zaGe7OCPFQe19lQ2Wuypcffwjc0f6yFT+YNPV
rnhtC5zQq8F/lo4kJjmvwQ==
-----END PRIVATE KEY-----
        "#
    }

    fn aws_config() -> AwsConfig {
        AwsConfig::new("TOKEN", "ARN")
    }

    #[test]
    #[ignore]
    pub async fn test_aws_certificate_manager() {
        let domain_config = DomainRecordsConfig::default();

        let config = aws_config();

        let manager = AwsCertificateManager::new("dev", "release", &config, &domain_config)
            .await
            .unwrap();

        let account_id = AccountId::from("a1");

        let import_result = manager
            .import(
                &account_id,
                "golem.cloud.test",
                get_certificate_body(),
                get_certificate_primary_key(),
                None,
            )
            .await;

        println!("{:?}", import_result);
        assert!(import_result.is_ok());

        let certificate_arn = import_result.unwrap();

        let delete_result = manager.delete(&account_id, &certificate_arn).await;

        println!("{:?}", delete_result);
        assert!(delete_result.is_ok());
    }
}
