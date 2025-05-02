extern crate rusoto_core;
extern crate rusoto_route53;

use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Debug, Display};
use std::sync::{Arc, Mutex};

use crate::aws_config::AwsConfig;
use crate::aws_load_balancer::AwsLoadBalancer;
use crate::config::DomainRecordsConfig;
use crate::model::{AccountApiDomain, ApiDomain, DomainRequest};
use crate::repo::api_domain::{ApiDomainRecord, ApiDomainRepo};
use crate::service::auth::AuthService;
use async_trait::async_trait;
use chrono::Utc;
use cloud_common::auth::{CloudAuthCtx, CloudNamespace};
use cloud_common::clients::auth::AuthServiceError;
use cloud_common::model::ProjectAction;
use golem_common::model::AccountId;
use golem_common::model::ProjectId;
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use rusoto_route53::{
    AliasTarget, Change, ChangeBatch, ChangeResourceRecordSetsRequest, CreateHostedZoneRequest,
    DeleteHostedZoneRequest, GetHostedZoneRequest, ListHostedZonesRequest, ResourceRecordSet,
    Route53, Route53Client,
};
use tap::TapFallible;
use tracing::{error, info};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ApiDomainServiceError {
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Domain Not Found: {0}")]
    NotFound(String),
    #[error("Domain Already Exists: {0}")]
    AlreadyExists(String),
    #[error("Internal auth client error: {0}")]
    InternalAuthClientError(String),
    #[error("Internal repository error: {0}")]
    InternalRepoError(RepoError),
    #[error("Internal AWS error: {context}: {error}")]
    InternalAWSError {
        context: String,
        error: Box<dyn Error>,
    },
    #[error("Internal error: {0}")]
    InternalConversionError(String),
}

impl SafeDisplay for ApiDomainServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            ApiDomainServiceError::Unauthorized(_) => self.to_string(),
            ApiDomainServiceError::NotFound(_) => self.to_string(),
            ApiDomainServiceError::AlreadyExists(_) => self.to_string(),
            ApiDomainServiceError::InternalAuthClientError(_) => self.to_string(),
            ApiDomainServiceError::InternalRepoError(inner) => inner.to_safe_string(),
            ApiDomainServiceError::InternalAWSError { context, .. } => context.clone(),
            ApiDomainServiceError::InternalConversionError(_) => self.to_string(),
        }
    }
}

impl From<AuthServiceError> for ApiDomainServiceError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::Unauthorized(error) => ApiDomainServiceError::Unauthorized(error),
            AuthServiceError::Forbidden(error) => ApiDomainServiceError::Unauthorized(error),
            AuthServiceError::InternalClientError(error) => {
                ApiDomainServiceError::InternalAuthClientError(error)
            }
        }
    }
}

impl ApiDomainServiceError {
    pub fn unauthorized<M>(error: M) -> Self
    where
        M: Display,
    {
        Self::Unauthorized(error.to_string())
    }

    pub fn already_exists<M>(error: M) -> Self
    where
        M: Display,
    {
        Self::AlreadyExists(error.to_string())
    }

    pub fn not_found<M>(error: M) -> Self
    where
        M: Display,
    {
        Self::NotFound(error.to_string())
    }
}

impl From<RegisterDomainError> for ApiDomainServiceError {
    fn from(error: RegisterDomainError) -> Self {
        match error {
            RegisterDomainError::AWSError { context, error } => {
                ApiDomainServiceError::InternalAWSError { context, error }
            }
            RegisterDomainError::NotAvailable(error) => {
                ApiDomainServiceError::already_exists(error)
            }
        }
    }
}

impl From<RepoError> for ApiDomainServiceError {
    fn from(value: RepoError) -> Self {
        ApiDomainServiceError::InternalRepoError(value)
    }
}

#[async_trait]
pub trait ApiDomainService {
    async fn create_or_update(
        &self,
        payload: &DomainRequest,
        auth: &CloudAuthCtx,
    ) -> Result<ApiDomain, ApiDomainServiceError>;

    async fn get(
        &self,
        project_id: &ProjectId,
        auth: &CloudAuthCtx,
    ) -> Result<Vec<ApiDomain>, ApiDomainServiceError>;

    async fn delete(
        &self,
        project_id: &ProjectId,
        domain_name: &str,
        auth: &CloudAuthCtx,
    ) -> Result<(), ApiDomainServiceError>;
}

pub struct ApiDomainServiceDefault {
    auth_service: Arc<dyn AuthService + Send + Sync>,
    register_domain: Arc<dyn RegisterDomain + Sync + Send>,
    domain_repo: Arc<dyn ApiDomainRepo + Sync + Send>,
}

impl ApiDomainServiceDefault {
    pub fn new(
        auth_service: Arc<dyn AuthService + Send + Sync>,
        register_domain: Arc<dyn RegisterDomain + Sync + Send>,
        domain_repo: Arc<dyn ApiDomainRepo + Sync + Send>,
    ) -> Self {
        Self {
            auth_service,
            register_domain,
            domain_repo,
        }
    }

    async fn is_authorized(
        &self,
        project_id: &ProjectId,
        permission: ProjectAction,
        auth: &CloudAuthCtx,
    ) -> Result<CloudNamespace, ApiDomainServiceError> {
        self.auth_service
            .authorize_project_action(project_id, permission, auth)
            .await
            .map_err(|e| e.into())
    }
}

#[async_trait]
impl ApiDomainService for ApiDomainServiceDefault {
    async fn create_or_update(
        &self,
        payload: &DomainRequest,
        auth: &CloudAuthCtx,
    ) -> Result<ApiDomain, ApiDomainServiceError> {
        let project_id = &payload.project_id;

        let namespace = self
            .is_authorized(project_id, ProjectAction::UpsertApiDomain, auth)
            .await?;

        let account_id = namespace.account_id.clone();

        let current_registration = self.domain_repo.get(&payload.domain_name).await?;

        if let Some(current) = current_registration {
            let current: AccountApiDomain = current.try_into().map_err(|e| {
                ApiDomainServiceError::InternalConversionError(format!(
                    "Failed to convert API Domain record: {e}"
                ))
            })?;

            if current.account_id != account_id || current.domain.project_id != namespace.project_id
            {
                error!(
                    namespace = %namespace,
                    "Register API domain - domain: {} - already used",
                    &payload.domain_name
                );

                return Err(ApiDomainServiceError::already_exists(
                    "API domain already used".to_string(),
                ));
            }
        }

        let current_name_servers = self
            .register_domain
            .get(&payload.domain_name)
            .await
            .tap_err(|e| {
                error!(
                    namespace = %namespace,
                    "Domain registration - domain: {} - register error: {}",
                    payload.domain_name, e
                );
            })?;

        let name_servers = match current_name_servers {
            Some(ns) => ns,
            None => {
                let domain_config = DomainConfig {
                    project_id: payload.project_id.clone(),
                    account_id: account_id.clone(),
                    domain_name: payload.domain_name.clone(),
                };

                self.register_domain
                    .register(&domain_config)
                    .await
                    .tap_err(|e| {
                        error!(
                            namespace = %namespace,
                            "Domain registration - domain: {} - register error: {}",
                             payload.domain_name, e
                        );
                    })?
            }
        };

        let record = ApiDomainRecord::new(
            account_id.clone(),
            payload.clone(),
            name_servers,
            Utc::now(),
        );

        self.domain_repo
            .create_or_update(&record)
            .await
            .map_err(|e| {
                error!(
                    namespace = %namespace,
                    "Domain registration - domain: {} - register error: {}",
                    payload.domain_name, e
                );
                ApiDomainServiceError::from(e)
            })?;

        let domain = record.try_into().map_err(|e| {
            ApiDomainServiceError::InternalConversionError(format!(
                "Failed to convert API Domain record: {e}"
            ))
        })?;

        Ok(domain)
    }

    async fn get(
        &self,
        project_id: &ProjectId,
        auth: &CloudAuthCtx,
    ) -> Result<Vec<ApiDomain>, ApiDomainServiceError> {
        let namespace = self
            .is_authorized(project_id, ProjectAction::ViewApiDomain, auth)
            .await?;

        info!(
            namespace = %namespace,
            "Get API domains"
        );

        let data = self.domain_repo.get_all(&namespace.to_string()).await?;

        let values: Vec<ApiDomain> = data
            .iter()
            .map(|d| d.clone().try_into())
            .collect::<Result<Vec<ApiDomain>, _>>()
            .map_err(|e| {
                ApiDomainServiceError::InternalConversionError(format!(
                    "Failed to convert API Domain record: {e}"
                ))
            })?;
        Ok(values)
    }

    async fn delete(
        &self,
        project_id: &ProjectId,
        domain_name: &str,
        auth: &CloudAuthCtx,
    ) -> Result<(), ApiDomainServiceError> {
        let namespace = self
            .is_authorized(project_id, ProjectAction::DeleteApiDomain, auth)
            .await?;

        info!(
            namespace = %namespace,
            "Delete API domain - domain: {}",
            domain_name
        );

        let data = self.domain_repo.get(domain_name).await?;

        if let Some(record) = data {
            let account_id = namespace.account_id;

            let domain: AccountApiDomain = record.try_into().map_err(|e| {
                ApiDomainServiceError::InternalConversionError(format!(
                    "Failed to convert API Domain record: {e}"
                ))
            })?;

            if domain.account_id == account_id
                && domain.domain.project_id == *project_id
                && domain.domain.domain_name == *domain_name
            {
                self.register_domain.unregister(domain_name).await?;

                self.domain_repo.delete(domain_name).await?;

                return Ok(());
            }
        }

        Err(ApiDomainServiceError::not_found(domain_name.to_string()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegisterDomainError {
    #[error("Not available: {0}")]
    NotAvailable(String),
    #[error("Internal error: {context}: {error}")]
    AWSError {
        context: String,
        error: Box<dyn Error>,
    },
}

impl RegisterDomainError {
    pub fn aws_error(context: impl AsRef<str>, error: Box<dyn Error>) -> Self {
        RegisterDomainError::AWSError {
            context: context.as_ref().to_string(),
            error,
        }
    }
}

impl SafeDisplay for RegisterDomainError {
    fn to_safe_string(&self) -> String {
        match self {
            RegisterDomainError::NotAvailable(_) => self.to_string(),
            RegisterDomainError::AWSError { context, .. } => context.clone(),
        }
    }
}

#[async_trait]
pub trait RegisterDomain {
    // Register domain is specifically registering domain with golem cloud, and
    // assumes the domain_name is pre-registered with appropriate registrars (godaddy)
    async fn register(
        &self,
        domain_config: &DomainConfig,
    ) -> Result<Vec<String>, RegisterDomainError>;

    async fn unregister(&self, domain_name: &str) -> Result<bool, RegisterDomainError>;

    async fn get(&self, domain_name: &str) -> Result<Option<Vec<String>>, RegisterDomainError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainConfig {
    pub project_id: ProjectId,
    pub account_id: AccountId,
    pub domain_name: String,
}

#[derive(Debug, Clone)]
pub struct AwsRegisterDomain {
    pub aws_config: AwsConfig,
    pub domain_records_config: DomainRecordsConfig,
}

impl AwsRegisterDomain {
    pub fn new(
        aws_config: &AwsConfig,
        domain_records_config: &DomainRecordsConfig,
    ) -> AwsRegisterDomain {
        AwsRegisterDomain {
            aws_config: aws_config.clone(),
            domain_records_config: domain_records_config.clone(),
        }
    }
}

#[async_trait]
impl RegisterDomain for AwsRegisterDomain {
    async fn register(
        &self,
        domain_config: &DomainConfig,
    ) -> Result<Vec<String>, RegisterDomainError> {
        info!("Register - domain name: {}", domain_config.domain_name);
        if self
            .domain_records_config
            .is_domain_available_for_registration(&domain_config.domain_name)
        {
            let client: Route53Client = self
                .aws_config
                .clone()
                .try_into()
                .map_err(|e: Box<dyn Error>| RegisterDomainError::aws_error("Client error", e))?;

            register_hosted_zone(domain_config, client)
                .await
                .map_err(|e: Box<dyn Error>| {
                    RegisterDomainError::aws_error("Register domain error", e)
                })
        } else {
            Err(RegisterDomainError::NotAvailable(
                "API domain is not available".to_string(),
            ))
        }
    }

    async fn unregister(&self, domain_name: &str) -> Result<bool, RegisterDomainError> {
        info!("Unregister - domain name: {}", domain_name);
        if self
            .domain_records_config
            .is_domain_available_for_registration(domain_name)
        {
            let client: Route53Client = self
                .aws_config
                .clone()
                .try_into()
                .map_err(|e: Box<dyn Error>| RegisterDomainError::aws_error("Client error", e))?;
            unregister_hosted_zone(domain_name, client)
                .await
                .map_err(|e: Box<dyn Error>| {
                    RegisterDomainError::aws_error("Unregister domain error", e)
                })
        } else {
            Err(RegisterDomainError::NotAvailable(
                "API domain is not available".to_string(),
            ))
        }
    }

    async fn get(&self, domain_name: &str) -> Result<Option<Vec<String>>, RegisterDomainError> {
        info!("Get - domain name: {}", domain_name);
        let client: Route53Client = self
            .aws_config
            .clone()
            .try_into()
            .map_err(|e: Box<dyn Error>| RegisterDomainError::aws_error("Client error", e))?;
        get_name_servers(domain_name, client)
            .await
            .map_err(|e: Box<dyn Error>| RegisterDomainError::aws_error("Get domain error", e))
    }
}

async fn register_hosted_zone(
    domain_config: &DomainConfig,
    client: Route53Client,
) -> Result<Vec<String>, Box<dyn Error>> {
    let caller_reference = Uuid::new_v4().to_string();

    // Create a client zone
    let request = CreateHostedZoneRequest {
        // TODO; Check if it already exists
        name: domain_config.domain_name.to_string(),
        caller_reference: caller_reference.to_string(),
        ..Default::default()
    };

    match client.create_hosted_zone(request).await {
        Ok(response) => {
            info!(
                "Register - domain name: {}, zone id: {}, name servers: {:?}",
                domain_config.domain_name,
                response.hosted_zone.id,
                response.delegation_set.name_servers
            );
            let name_servers = response.delegation_set.name_servers;

            Ok(name_servers)
        }
        Err(e) => {
            error!(
                "Register - domain name: {} - error: {:?}",
                domain_config.domain_name, e
            );
            Err(Box::new(e))
        }
    }
}

async fn unregister_hosted_zone(
    domain_name: &str,
    client: Route53Client,
) -> Result<bool, Box<dyn Error>> {
    let zones = client
        .list_hosted_zones(ListHostedZonesRequest::default())
        .await?;

    let target_zone_name = format!("{}.", domain_name); // appends a dot

    let zone = zones
        .hosted_zones
        .iter()
        .find(|x| x.name.clone() == target_zone_name);

    if let Some(zone) = zone {
        let _ = client
            .delete_hosted_zone(DeleteHostedZoneRequest {
                id: zone.id.clone(),
            })
            .await?;
        Ok(true)
    } else {
        Ok(false)
    }
}

async fn get_name_servers(
    domain_name: &str,
    client: Route53Client,
) -> Result<Option<Vec<String>>, Box<dyn Error>> {
    let zones = client
        .list_hosted_zones(ListHostedZonesRequest::default())
        .await?;

    let target_zone_name = format!("{}.", domain_name); // appends a dot

    let zone = zones
        .hosted_zones
        .iter()
        .find(|x| x.name.clone() == target_zone_name);

    let ns = if let Some(zone) = zone {
        let hosted_zone = client
            .get_hosted_zone(GetHostedZoneRequest {
                id: zone.id.clone(),
            })
            .await?;
        hosted_zone.delegation_set.map(|d| d.name_servers)
    } else {
        None
    };

    Ok(ns)
}

pub struct InMemoryRegisterDomain {
    domains: Mutex<HashMap<String, DomainConfig>>,
    default_name_servers: Vec<String>,
    domain_records_config: DomainRecordsConfig,
}

impl InMemoryRegisterDomain {
    pub fn new(
        default_name_servers: Vec<String>,
        domain_records_config: DomainRecordsConfig,
    ) -> Self {
        InMemoryRegisterDomain {
            domains: Mutex::new(HashMap::new()),
            default_name_servers,
            domain_records_config,
        }
    }
}

impl Default for InMemoryRegisterDomain {
    fn default() -> Self {
        InMemoryRegisterDomain {
            domains: Mutex::new(HashMap::new()),
            default_name_servers: vec![],
            domain_records_config: DomainRecordsConfig::default(),
        }
    }
}

#[async_trait]
impl RegisterDomain for InMemoryRegisterDomain {
    async fn register(
        &self,
        domain_config: &DomainConfig,
    ) -> Result<Vec<String>, RegisterDomainError> {
        info!(
            "Registering - domain {} in local is disabled",
            domain_config.domain_name
        );

        if self
            .domain_records_config
            .is_domain_available_for_registration(&domain_config.domain_name)
        {
            let key = domain_config.domain_name.to_string().clone();

            let mut domains = self.domains.lock().unwrap();

            domains.insert(key, domain_config.clone());

            Ok(self.default_name_servers.clone())
        } else {
            Err(RegisterDomainError::NotAvailable(
                "API domain is not available".to_string(),
            ))
        }
    }

    async fn unregister(&self, domain_name: &str) -> Result<bool, RegisterDomainError> {
        info!("Unregister - domain name: {}", domain_name);
        if self
            .domain_records_config
            .is_domain_available_for_registration(domain_name)
        {
            let mut domains = self.domains.lock().unwrap();
            let result = domains.remove(domain_name);
            Ok(result.is_some())
        } else {
            Err(RegisterDomainError::NotAvailable(
                "API domain is not available".to_string(),
            ))
        }
    }

    async fn get(&self, domain_name: &str) -> Result<Option<Vec<String>>, RegisterDomainError> {
        info!("Get - domain name: {}", domain_name);
        let domains = self.domains.lock().unwrap();

        Ok(domains
            .get(domain_name)
            .map(|_| self.default_name_servers.clone()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegisterDomainRouteError {
    #[error("Not available: {0}")]
    NotAvailable(String),
    #[error("Internal error: {context}: {error}")]
    AWSError {
        context: String,
        error: Box<dyn Error>,
    },
}

impl RegisterDomainRouteError {
    pub fn aws_error(context: impl AsRef<str>, error: Box<dyn Error>) -> Self {
        RegisterDomainRouteError::AWSError {
            context: context.as_ref().to_string(),
            error,
        }
    }
}

impl SafeDisplay for RegisterDomainRouteError {
    fn to_safe_string(&self) -> String {
        match self {
            RegisterDomainRouteError::NotAvailable(_) => self.to_string(),
            RegisterDomainRouteError::AWSError { context, .. } => context.clone(),
        }
    }
}

// Register user specified api-site in Route Infrastructure
#[async_trait]
pub trait RegisterDomainRoute {
    async fn register(
        &self,
        domain: &str,
        subdomain: Option<&str>,
    ) -> Result<(), RegisterDomainRouteError>;

    async fn unregister(
        &self,
        domain: &str,
        subdomain: Option<&str>,
    ) -> Result<(), RegisterDomainRouteError>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct AwsRoute53HostedZone {
    pub id: String,
    pub name: String,
}

impl AwsRoute53HostedZone {
    pub async fn new(
        config: &AwsConfig,
        domain: &str,
    ) -> Result<AwsRoute53HostedZone, Box<dyn Error>> {
        let client: Route53Client = config.clone().try_into()?;

        AwsRoute53HostedZone::with_client(&client, domain).await
    }

    pub async fn with_client(
        client: &Route53Client,
        domain: &str,
    ) -> Result<AwsRoute53HostedZone, Box<dyn Error>> {
        let zones = client
            .list_hosted_zones(ListHostedZonesRequest::default())
            .await?;

        let target_zone_name = format!("{}.", domain); // appends a dot

        let zone = zones
            .hosted_zones
            .iter()
            .find(|x| x.name.clone() == target_zone_name)
            .map(move |x| AwsRoute53HostedZone {
                id: x
                    .id
                    .clone()
                    .strip_prefix("/hostedzone/")
                    .unwrap()
                    .to_string(),
                name: target_zone_name,
            });

        zone.ok_or("Not found".to_string().into())
    }
}

#[derive(Debug, Clone)]
pub struct AwsDomainRoute {
    pub environment: String,
    pub load_balancer: AwsLoadBalancer,
    pub aws_config: AwsConfig,
    pub domain_records_config: DomainRecordsConfig,
}

impl AwsDomainRoute {
    pub async fn new(
        environment: &str,
        workspace: &str,
        config: &AwsConfig,
        domain_records_config: &DomainRecordsConfig,
    ) -> Result<AwsDomainRoute, Box<dyn Error>> {
        let load_balancer = AwsLoadBalancer::new(environment, workspace, config).await?;

        Ok(AwsDomainRoute {
            environment: environment.to_string(),
            load_balancer,
            aws_config: config.clone(),
            domain_records_config: domain_records_config.clone(),
        })
    }
}

#[async_trait]
impl RegisterDomainRoute for AwsDomainRoute {
    async fn register(
        &self,
        domain: &str,
        sub_domain_opt: Option<&str>,
    ) -> Result<(), RegisterDomainRouteError> {
        let api_site = match sub_domain_opt {
            Some(subdomain) => format!("{}.{}", subdomain, domain),
            None => domain.to_string(),
        };

        info!("Register - API site: {}", api_site);

        if let Some(sub_domain) = sub_domain_opt {
            if sub_domain.contains('.') {
                return Err(RegisterDomainRouteError::NotAvailable(
                    "API site subdomain cannot be multi-level".to_string(),
                ));
            }
        }

        if !self.domain_records_config.is_domain_available(domain) {
            return Err(RegisterDomainRouteError::NotAvailable(
                "API domain is not available".to_string(),
            ));
        }

        let client: Route53Client = self
            .aws_config
            .clone()
            .try_into()
            .map_err(|e: Box<dyn Error>| RegisterDomainRouteError::aws_error("Client error", e))?;

        let hosted_zone = AwsRoute53HostedZone::with_client(&client, domain)
            .await
            .map_err(|e: Box<dyn Error>| {
                RegisterDomainRouteError::aws_error("Register domain error", e)
            })?;

        if !self
            .domain_records_config
            .is_site_available(api_site.as_str(), &hosted_zone.name)
        {
            return Err(RegisterDomainRouteError::NotAvailable(
                "API site is not available".to_string(),
            ));
        }

        let change_batch = ChangeBatch {
            changes: vec![Change {
                action: "UPSERT".to_string(),
                resource_record_set: ResourceRecordSet {
                    name: api_site.to_string(),
                    type_: "A".to_string(),
                    alias_target: Some(AliasTarget {
                        dns_name: self.load_balancer.dns_name.clone(),
                        evaluate_target_health: false,
                        hosted_zone_id: self.load_balancer.hosted_zone.clone(),
                    }),
                    ..Default::default()
                },
            }],
            ..Default::default()
        };

        let request = ChangeResourceRecordSetsRequest {
            hosted_zone_id: hosted_zone.id.clone(),
            change_batch,
        };

        client
            .change_resource_record_sets(request)
            .await
            .map_err(|e| {
                RegisterDomainRouteError::aws_error("Failed to register domain route", Box::new(e))
            })?;

        Ok(())
    }

    async fn unregister(
        &self,
        domain: &str,
        subdomain: Option<&str>,
    ) -> Result<(), RegisterDomainRouteError> {
        let api_site = match subdomain {
            Some(subdomain) => format!("{}.{}", subdomain, domain),
            None => domain.to_string(),
        };

        info!("Unregister - API site: {}", api_site);

        let client: Route53Client = self
            .aws_config
            .clone()
            .try_into()
            .map_err(|e: Box<dyn Error>| RegisterDomainRouteError::aws_error("Client error", e))?;

        let hosted_zone = AwsRoute53HostedZone::with_client(&client, domain)
            .await
            .map_err(|e: Box<dyn Error>| {
                RegisterDomainRouteError::aws_error("Unregister domain error", e)
            })?;

        let change_batch = ChangeBatch {
            changes: vec![Change {
                action: "DELETE".to_string(),
                resource_record_set: ResourceRecordSet {
                    name: api_site,
                    type_: "A".to_string(),
                    alias_target: Some(AliasTarget {
                        dns_name: self.load_balancer.dns_name.clone(),
                        evaluate_target_health: false,
                        hosted_zone_id: self.load_balancer.hosted_zone.clone(),
                    }),
                    ..Default::default()
                },
            }],
            ..Default::default()
        };

        let request = ChangeResourceRecordSetsRequest {
            hosted_zone_id: hosted_zone.id.clone(),
            change_batch,
        };

        client
            .change_resource_record_sets(request)
            .await
            .map_err(|e| {
                RegisterDomainRouteError::aws_error(
                    "Failed to unregister domain route",
                    Box::new(e),
                )
            })?;

        Ok(())
    }
}

pub struct InMemoryRegisterDomainRoute {
    pub environment: String,
    pub hosted_zone: String,
    pub domain_records_config: DomainRecordsConfig,
}

impl InMemoryRegisterDomainRoute {
    pub fn new(
        environment: &str,
        hosted_zone: &str,
        domain_records_config: &DomainRecordsConfig,
    ) -> InMemoryRegisterDomainRoute {
        InMemoryRegisterDomainRoute {
            environment: environment.to_string(),
            hosted_zone: hosted_zone.to_string(),
            domain_records_config: domain_records_config.clone(),
        }
    }
}

#[async_trait]
impl RegisterDomainRoute for InMemoryRegisterDomainRoute {
    async fn register(
        &self,
        domain: &str,
        sub_domain: Option<&str>,
    ) -> Result<(), RegisterDomainRouteError> {
        let api_site = match sub_domain {
            Some(subdomain) => format!("{}.{}", subdomain, domain),
            None => domain.to_string(),
        };

        info!("Register API site: {}", api_site);

        if let Some(sub_domain) = sub_domain {
            if sub_domain.contains('.') {
                return Err(RegisterDomainRouteError::NotAvailable(
                    "API site subdomain cannot be multi-level".to_string(),
                ));
            }
        }

        if !self.domain_records_config.is_domain_available(domain) {
            return Err(RegisterDomainRouteError::NotAvailable(
                "API domain is not available".to_string(),
            ));
        }

        if self
            .domain_records_config
            .is_site_available(api_site.as_str(), &self.hosted_zone)
        {
            Ok(())
        } else {
            Err(RegisterDomainRouteError::NotAvailable(
                "API site is not available".to_string(),
            ))
        }
    }

    async fn unregister(
        &self,
        _domain: &str,
        _subdomain: Option<&str>,
    ) -> Result<(), RegisterDomainRouteError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::aws_config::AwsConfig;
    use crate::config::DomainRecordsConfig;
    use crate::service::api_domain::{
        get_name_servers, register_hosted_zone, unregister_hosted_zone, DomainConfig,
        InMemoryRegisterDomainRoute,
    };
    use crate::service::api_domain::{AwsDomainRoute, AwsRoute53HostedZone, RegisterDomainRoute};
    use golem_common::model::AccountId;
    use golem_common::model::ProjectId;

    use rusoto_core::Region;
    use rusoto_route53::Route53Client;

    #[test]
    #[ignore]
    pub async fn test_get_name_servers() {
        let client = Route53Client::new(Region::default());

        let result = get_name_servers("dev-api.golem.cloud", client)
            .await
            .unwrap();

        assert!(result.is_some());
        assert!(!result.unwrap_or_default().is_empty());
    }

    #[test]
    #[ignore]
    pub async fn test_register_and_unregister_hosted_zone() {
        let account_id = AccountId::from("a1");
        let project_id = "15d70aa5-2e23-4ee3-b65c-4e1d702836a3"
            .parse::<ProjectId>()
            .unwrap();

        let test_domain = DomainConfig {
            project_id,
            account_id,
            domain_name: "test.golem.cloud.test".to_string(),
        };

        let client = Route53Client::new(Region::default());

        let register_result = register_hosted_zone(&test_domain, client.clone())
            .await
            .unwrap();

        let unregister_result = unregister_hosted_zone(&test_domain.domain_name, client.clone())
            .await
            .unwrap();

        let get_result = get_name_servers(&test_domain.domain_name, client)
            .await
            .unwrap();

        assert!(!register_result.is_empty());

        assert!(unregister_result);

        assert!(get_result.is_none());
    }

    #[test]
    pub async fn test_domain_route() {
        let domain_records_config = DomainRecordsConfig {
            domain_allow_list: vec!["dev-api.golem.cloud".to_string()],
            ..Default::default()
        };

        let domain_route =
            InMemoryRegisterDomainRoute::new("dev", "dev-api.golem.cloud", &domain_records_config);

        let reg_result = domain_route
            .register("dev-api.golem.cloud", Some("some-test-domain"))
            .await;
        let unreg_result = domain_route
            .unregister("dev-api.golem.cloud", Some("some-test-domain"))
            .await;
        let reg_subdomain_error_result = domain_route
            .register("dev-api.golem.cloud", Some("some-test-domain.xxx"))
            .await;
        let reg_domain_error_result = domain_route
            .register("api.golem.cloud", Some("some-test-domain"))
            .await;

        assert!(reg_result.is_ok());
        assert!(unreg_result.is_ok());
        assert!(reg_subdomain_error_result.is_err());
        assert!(reg_domain_error_result.is_err());
    }

    fn aws_config() -> AwsConfig {
        AwsConfig::new("TOKEN", "ARN")
    }

    #[test]
    #[ignore]
    pub async fn test_aws_domain_route() {
        let domain_config = DomainRecordsConfig {
            domain_allow_list: vec!["dev-api.golem.cloud".to_string()],
            ..Default::default()
        };

        let config = aws_config();
        let domain_route = AwsDomainRoute::new("dev", "release", &config, &domain_config)
            .await
            .unwrap();

        let reg_result = domain_route
            .register("dev-api.golem.cloud", Some("some-test-domain"))
            .await;
        let unreg_result = domain_route
            .unregister("dev-api.golem.cloud", Some("some-test-domain"))
            .await;
        let reg_subdomain_error_result = domain_route
            .register("dev-api.golem.cloud", Some("some-test-domain.xxx"))
            .await;
        let reg_domain_error_result = domain_route
            .register("api.golem.cloud", Some("some-test-domain"))
            .await;

        assert!(reg_result.is_ok());
        assert!(unreg_result.is_ok());
        assert!(reg_subdomain_error_result.is_err());
        assert!(reg_domain_error_result.is_err());
    }

    #[test]
    #[ignore]
    pub async fn test_aws_route53_hosted_zone() {
        let client = Route53Client::new(Region::default());
        let result = AwsRoute53HostedZone::with_client(&client, "myapplication.com")
            .await
            .unwrap();

        assert_eq!(result.name, "myapplication.com.");
    }
}
