use crate::cloud::auth::{Auth, AuthLive};
use crate::cloud::clients::account::{AccountClient, AccountClientLive};
use crate::cloud::clients::api_definition::ApiDefinitionClientLive;
use crate::cloud::clients::api_deployment::ApiDeploymentClientLive;
use crate::cloud::clients::api_security::ApiSecurityClientLive;
use crate::cloud::clients::certificate::{CertificateClient, CertificateClientLive};
use crate::cloud::clients::component::ComponentClientLive;
use crate::cloud::clients::domain::{DomainClient, DomainClientLive};
use crate::cloud::clients::grant::{GrantClient, GrantClientLive};
use crate::cloud::clients::login::{LoginClient, LoginClientLive};
use crate::cloud::clients::plugin::PluginClientLive;
use crate::cloud::clients::policy::{ProjectPolicyClient, ProjectPolicyClientLive};
use crate::cloud::clients::project::{ProjectClient, ProjectClientLive};
use crate::cloud::clients::project_grant::{ProjectGrantClient, ProjectGrantClientLive};
use crate::cloud::clients::token::{TokenClient, TokenClientLive};
use crate::cloud::clients::worker::WorkerClientLive;
use crate::cloud::clients::CloudAuthentication;
use crate::cloud::model::{PluginDefinition, PluginDefinitionWithoutOwner, ProjectRef};
use crate::cloud::service::account::{AccountService, AccountServiceLive};
use crate::cloud::service::certificate::{CertificateService, CertificateServiceLive};
use crate::cloud::service::domain::{DomainService, DomainServiceLive};
use crate::cloud::service::grant::{GrantService, GrantServiceLive};
use crate::cloud::service::policy::{ProjectPolicyService, ProjectPolicyServiceLive};
use crate::cloud::service::project::{CloudProjectResolver, ProjectService, ProjectServiceLive};
use crate::cloud::service::project_grant::{ProjectGrantService, ProjectGrantServiceLive};
use crate::cloud::service::token::{TokenService, TokenServiceLive};
use async_trait::async_trait;
use golem_cli::clients::api_definition::ApiDefinitionClient;
use golem_cli::clients::api_deployment::ApiDeploymentClient;
use golem_cli::clients::api_security::ApiSecurityClient;
use golem_cli::clients::component::ComponentClient;
use golem_cli::clients::file_download::{FileDownloadClient, FileDownloadClientLive};
use golem_cli::clients::health_check::HealthCheckClient;
use golem_cli::clients::plugin::PluginClient;
use golem_cli::clients::worker::WorkerClient;
use golem_cli::cloud::{CloudAuthenticationConfig, ProjectId};
use golem_cli::config::{CloudProfile, Config, HttpClientConfig, Profile, ProfileName};
use golem_cli::factory::ServiceFactory;
use golem_cli::init::ProfileAuth;
use golem_cli::model::GolemError;
use golem_cli::oss::factory::{make_reqwest_client, OssServiceFactoryConfig};
use golem_cli::service::project::ProjectResolver;
use golem_cloud_client::{CloudPluginScope, Context, Security};
use itertools::Itertools;
use std::path::Path;
use std::sync::Arc;
use tracing::warn;
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct CloudServiceFactoryServiceConfig {
    pub oss_config: OssServiceFactoryConfig,
    pub cloud_url: Url,
}

pub struct CloudServiceFactorAuthConfig<'a> {
    manual_auth_token: Option<Uuid>,
    profile_name: &'a ProfileName,
    auth_config: &'a Option<CloudAuthenticationConfig>,
    config_dir: &'a Path,
}

#[derive(Debug, Clone)]
pub struct CloudServiceFactory {
    config: CloudServiceFactoryServiceConfig,
    pub auth: CloudAuthentication,
    http_client_service: reqwest::Client,
    http_client_health_check: reqwest::Client,
    http_client_file_download: reqwest::Client,
}

impl CloudServiceFactory {
    pub async fn new(
        auth_config: &CloudServiceFactorAuthConfig<'_>,
        config: CloudServiceFactoryServiceConfig,
    ) -> Result<Self, GolemError> {
        let service_http_client =
            make_reqwest_client(&config.oss_config.service_http_client_config)?;
        let health_check_http_client =
            make_reqwest_client(&config.oss_config.health_check_http_client_config)?;
        let file_download_http_client =
            make_reqwest_client(&config.oss_config.file_download_http_client_config)?;

        let auth = Self::authenticate(
            auth_config,
            config.cloud_url.clone(),
            service_http_client.clone(),
        )
        .await?;

        Ok(Self {
            config,
            auth,
            http_client_service: service_http_client,
            http_client_health_check: health_check_http_client,
            http_client_file_download: file_download_http_client.clone(),
        })
    }

    pub async fn from_profile(
        profile_name: &ProfileName,
        profile: &CloudProfile,
        config_dir: &Path,
        manual_auth_token: Option<Uuid>,
    ) -> Result<Self, GolemError> {
        let component_url = profile.custom_url.clone().unwrap_or_else(Self::default_url);
        let cloud_url = profile
            .custom_cloud_url
            .clone()
            .unwrap_or_else(|| component_url.clone());
        let worker_url = profile
            .custom_worker_url
            .clone()
            .unwrap_or_else(|| component_url.clone());
        let allow_insecure = profile.allow_insecure;

        Self::new(
            &CloudServiceFactorAuthConfig {
                manual_auth_token,
                profile_name,
                auth_config: &profile.auth,
                config_dir,
            },
            CloudServiceFactoryServiceConfig {
                oss_config: OssServiceFactoryConfig {
                    component_url,
                    worker_url,
                    service_http_client_config: HttpClientConfig::new_for_service_calls(
                        allow_insecure,
                    ),
                    health_check_http_client_config: HttpClientConfig::new_for_service_calls(
                        allow_insecure,
                    ),
                    file_download_http_client_config: HttpClientConfig::new_for_file_download(
                        allow_insecure,
                    ),
                    allow_insecure,
                },
                cloud_url,
            },
        )
        .await
    }

    fn security_token(&self) -> Security {
        Security::Bearer(self.auth.0.secret.value.to_string())
    }

    async fn authenticate(
        config: &CloudServiceFactorAuthConfig<'_>,
        base_url: Url,
        client: reqwest::Client,
    ) -> Result<CloudAuthentication, GolemError> {
        let auth = Self::auth(base_url, client)
            .authenticate(
                config.manual_auth_token,
                config.profile_name,
                config.auth_config,
                config.config_dir,
            )
            .await?;

        Ok(auth)
    }

    fn default_url() -> Url {
        Url::parse("https://release.api.golem.cloud/").unwrap()
    }

    fn login_context(base_url: Url, client: reqwest::Client) -> Context {
        Context {
            base_url,
            client,
            security_token: Security::Empty,
        }
    }

    fn login_client(base_url: Url, client: reqwest::Client) -> Box<dyn LoginClient + Send + Sync> {
        let context = Self::login_context(base_url, client);
        Box::new(LoginClientLive {
            client: golem_cloud_client::api::LoginClientLive {
                context: context.clone(),
            },
            context,
        })
    }

    fn auth(base_url: Url, client: reqwest::Client) -> Box<dyn Auth + Send + Sync> {
        Box::new(AuthLive {
            login: Self::login_client(base_url, client),
        })
    }

    fn cloud_context(&self) -> Context {
        Context {
            base_url: self.config.cloud_url.clone(),
            client: self.http_client_service.clone(),
            security_token: self.security_token(),
        }
    }

    fn cloud_context_health_check(&self) -> Context {
        Context {
            base_url: self.config.cloud_url.clone(),
            client: self.http_client_health_check.clone(),
            security_token: self.security_token(),
        }
    }

    fn component_context(&self) -> Context {
        Context {
            base_url: self.config.oss_config.component_url.clone(),
            client: self.http_client_service.clone(),
            security_token: self.security_token(),
        }
    }

    fn component_context_health_check(&self) -> Context {
        Context {
            base_url: self.config.oss_config.component_url.clone(),
            client: self.http_client_health_check.clone(),
            security_token: self.security_token(),
        }
    }

    fn worker_context(&self) -> Context {
        Context {
            base_url: self.config.oss_config.worker_url.clone(),
            client: self.http_client_service.clone(),
            security_token: self.security_token(),
        }
    }

    fn worker_context_health_check(&self) -> Context {
        Context {
            base_url: self.config.oss_config.worker_url.clone(),
            client: self.http_client_health_check.clone(),
            security_token: self.security_token(),
        }
    }

    fn project_client(&self) -> Box<dyn ProjectClient + Send + Sync> {
        Box::new(ProjectClientLive {
            client: golem_cloud_client::api::ProjectClientLive {
                context: self.cloud_context(),
            },
        })
    }

    pub fn project_service(&self) -> Arc<dyn ProjectService + Send + Sync + 'static> {
        Arc::new(ProjectServiceLive {
            account_id: self.auth.account_id(),
            client: self.project_client(),
        })
    }

    fn account_client(&self) -> Box<dyn AccountClient + Send + Sync> {
        Box::new(AccountClientLive {
            client: golem_cloud_client::api::AccountClientLive {
                context: self.cloud_context(),
            },
        })
    }

    pub fn account_service(&self) -> Box<dyn AccountService + Send + Sync> {
        Box::new(AccountServiceLive {
            account_id: self.auth.account_id(),
            client: self.account_client(),
        })
    }

    fn grant_client(&self) -> Box<dyn GrantClient + Send + Sync> {
        Box::new(GrantClientLive {
            client: golem_cloud_client::api::GrantClientLive {
                context: self.cloud_context(),
            },
        })
    }

    pub fn grant_service(&self) -> Box<dyn GrantService + Send + Sync> {
        Box::new(GrantServiceLive {
            account_id: self.auth.account_id(),
            client: self.grant_client(),
        })
    }

    fn token_client(&self) -> Box<dyn TokenClient + Send + Sync> {
        Box::new(TokenClientLive {
            client: golem_cloud_client::api::TokenClientLive {
                context: self.cloud_context(),
            },
        })
    }

    pub fn token_service(&self) -> Box<dyn TokenService + Send + Sync> {
        Box::new(TokenServiceLive {
            account_id: self.auth.account_id(),
            client: self.token_client(),
        })
    }

    fn project_grant_client(&self) -> Box<dyn ProjectGrantClient + Send + Sync> {
        Box::new(ProjectGrantClientLive {
            client: golem_cloud_client::api::ProjectGrantClientLive {
                context: self.cloud_context(),
            },
        })
    }

    pub fn project_grant_service(&self) -> Box<dyn ProjectGrantService + Send + Sync> {
        Box::new(ProjectGrantServiceLive {
            client: self.project_grant_client(),
            projects: self.project_service(),
        })
    }

    fn project_policy_client(&self) -> Box<dyn ProjectPolicyClient + Send + Sync> {
        Box::new(ProjectPolicyClientLive {
            client: golem_cloud_client::api::ProjectPolicyClientLive {
                context: self.cloud_context(),
            },
        })
    }

    pub fn project_policy_service(&self) -> Box<dyn ProjectPolicyService + Send + Sync> {
        Box::new(ProjectPolicyServiceLive {
            client: self.project_policy_client(),
        })
    }

    fn certificate_client(&self) -> Box<dyn CertificateClient + Send + Sync> {
        Box::new(CertificateClientLive {
            client: golem_cloud_client::api::ApiCertificateClientLive {
                context: self.worker_context(),
            },
        })
    }

    pub fn certificate_service(&self) -> Box<dyn CertificateService + Send + Sync> {
        Box::new(CertificateServiceLive {
            client: self.certificate_client(),
            projects: self.project_service(),
        })
    }

    fn domain_client(&self) -> Box<dyn DomainClient + Send + Sync> {
        Box::new(DomainClientLive {
            client: golem_cloud_client::api::ApiDomainClientLive {
                context: self.worker_context(),
            },
        })
    }

    pub fn domain_service(&self) -> Box<dyn DomainService + Send + Sync> {
        Box::new(DomainServiceLive {
            client: self.domain_client(),
            projects: self.project_service(),
        })
    }
}

impl ServiceFactory for CloudServiceFactory {
    type ProjectRef = ProjectRef;
    type ProjectContext = ProjectId;
    type PluginDefinition = PluginDefinition;
    type PluginDefinitionWithoutOwner = PluginDefinitionWithoutOwner;
    type PluginScope = CloudPluginScope;

    fn project_resolver(
        &self,
    ) -> Arc<dyn ProjectResolver<Self::ProjectRef, Self::ProjectContext> + Send + Sync> {
        Arc::new(CloudProjectResolver {
            service: self.project_service(),
        })
    }

    fn file_download_client(&self) -> Box<dyn FileDownloadClient + Send + Sync> {
        Box::new(FileDownloadClientLive {
            client: self.http_client_file_download.clone(),
        })
    }

    fn component_client(
        &self,
    ) -> Box<dyn ComponentClient<ProjectContext = Self::ProjectContext> + Send + Sync> {
        Box::new(ComponentClientLive {
            client: golem_cloud_client::api::ComponentClientLive {
                context: self.component_context(),
            },
        })
    }

    fn worker_client(&self) -> Arc<dyn WorkerClient + Send + Sync> {
        Arc::new(WorkerClientLive {
            client: golem_cloud_client::api::WorkerClientLive {
                context: self.worker_context(),
            },
            context: self.worker_context(),
            allow_insecure: self.config.oss_config.allow_insecure,
        })
    }

    fn api_definition_client(
        &self,
    ) -> Box<dyn ApiDefinitionClient<ProjectContext = Self::ProjectContext> + Send + Sync> {
        Box::new(ApiDefinitionClientLive {
            client: golem_cloud_client::api::ApiDefinitionClientLive {
                context: self.worker_context(),
            },
        })
    }

    fn api_deployment_client(
        &self,
    ) -> Box<dyn ApiDeploymentClient<ProjectContext = Self::ProjectContext> + Send + Sync> {
        Box::new(ApiDeploymentClientLive {
            client: golem_cloud_client::api::ApiDeploymentClientLive {
                context: self.worker_context(),
            },
        })
    }

    fn api_security_scheme_client(
        &self,
    ) -> Box<dyn ApiSecurityClient<ProjectContext = Self::ProjectContext> + Send + Sync> {
        Box::new(ApiSecurityClientLive {
            client: golem_cloud_client::api::ApiSecurityClientLive {
                context: self.worker_context(),
            },
        })
    }

    fn health_check_clients(&self) -> Vec<Arc<dyn HealthCheckClient + Send + Sync>> {
        let contexts = vec![
            self.cloud_context_health_check(),
            self.component_context_health_check(),
            self.worker_context_health_check(),
        ];
        let contexts_count = contexts.len();

        let unique_contexts: Vec<_> = contexts
            .into_iter()
            .unique_by(|context| context.base_url.clone())
            .collect();

        if contexts_count != unique_contexts.len() {
            warn!(
                "Health check client contexts are not unique, contexts count: {}, unique count: {}",
                contexts_count,
                unique_contexts.len()
            )
        }

        unique_contexts
            .into_iter()
            .map(|context| -> Arc<dyn HealthCheckClient + Send + Sync> {
                Arc::new(crate::cloud::clients::health_check::HealthCheckClientLive {
                    client: golem_cloud_client::api::HealthCheckClientLive { context },
                })
            })
            .collect()
    }

    fn plugin_client(
        &self,
    ) -> Arc<
        dyn PluginClient<
                PluginDefinition = Self::PluginDefinition,
                PluginDefinitionWithoutOwner = Self::PluginDefinitionWithoutOwner,
                PluginScope = Self::PluginScope,
                ProjectContext = Self::ProjectContext,
            > + Send
            + Sync,
    > {
        Arc::new(PluginClientLive {
            client: golem_cloud_client::api::PluginClientLive {
                context: self.component_context(),
            },
        })
    }
}

pub struct CloudProfileAuth();

#[async_trait]
impl ProfileAuth for CloudProfileAuth {
    fn auth_enabled(&self) -> bool {
        true
    }

    async fn auth(&self, profile_name: &ProfileName, config_dir: &Path) -> Result<(), GolemError> {
        let profile = Config::get_profile(profile_name, config_dir)
            .ok_or(GolemError(format!("Can't find profile '{profile_name}'")))?;

        match profile {
            Profile::Golem(_) => Ok(()),
            Profile::GolemCloud(profile) => {
                let _ = CloudServiceFactory::from_profile(profile_name, &profile, config_dir, None)
                    .await?;
                Ok(())
            }
        }
    }
}
