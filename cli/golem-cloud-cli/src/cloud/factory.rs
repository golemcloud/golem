use crate::cloud::auth::{Auth, AuthLive};
use crate::cloud::clients::account::{AccountClient, AccountClientLive};
use crate::cloud::clients::api_definition::ApiDefinitionClientLive;
use crate::cloud::clients::api_deployment::ApiDeploymentClientLive;
use crate::cloud::clients::certificate::{CertificateClient, CertificateClientLive};
use crate::cloud::clients::component::ComponentClientLive;
use crate::cloud::clients::domain::{DomainClient, DomainClientLive};
use crate::cloud::clients::grant::{GrantClient, GrantClientLive};
use crate::cloud::clients::login::{LoginClient, LoginClientLive};
use crate::cloud::clients::policy::{ProjectPolicyClient, ProjectPolicyClientLive};
use crate::cloud::clients::project::{ProjectClient, ProjectClientLive};
use crate::cloud::clients::project_grant::{ProjectGrantClient, ProjectGrantClientLive};
use crate::cloud::clients::token::{TokenClient, TokenClientLive};
use crate::cloud::clients::worker::WorkerClientLive;
use crate::cloud::clients::CloudAuthentication;
use crate::cloud::main::get_auth;
use crate::cloud::model::ProjectRef;
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
use golem_cli::clients::component::ComponentClient;
use golem_cli::clients::health_check::HealthCheckClient;
use golem_cli::clients::worker::WorkerClient;
use golem_cli::cloud::ProjectId;
use golem_cli::config::{CloudProfile, Config, Profile, ProfileName};
use golem_cli::factory::{FactoryWithAuth, ServiceFactory};
use golem_cli::init::ProfileAuth;
use golem_cli::model::GolemError;
use golem_cli::service::project::ProjectResolver;
use golem_cloud_client::{Context, Security};
use std::path::Path;
use std::sync::Arc;
use url::Url;

#[derive(Debug, Clone)]
pub struct CloudServiceFactory {
    pub component_url: Url,
    pub cloud_url: Url,
    pub worker_url: Url,
    pub allow_insecure: bool,
}

fn default_url() -> Url {
    Url::parse("https://release.api.golem.cloud/").unwrap()
}

impl CloudServiceFactory {
    pub fn from_profile(profile: &CloudProfile) -> Self {
        let url = profile.custom_url.clone().unwrap_or_else(default_url);
        let cloud_url = profile
            .custom_cloud_url
            .clone()
            .unwrap_or_else(|| url.clone());
        let worker_url = profile
            .custom_worker_url
            .clone()
            .unwrap_or_else(|| url.clone());
        let allow_insecure = profile.allow_insecure;

        CloudServiceFactory {
            component_url: url,
            cloud_url,
            worker_url,
            allow_insecure,
        }
    }

    fn client(&self) -> Result<reqwest::Client, GolemError> {
        let mut builder = reqwest::Client::builder();
        if self.allow_insecure {
            builder = builder.danger_accept_invalid_certs(true);
        }

        Ok(builder.connection_verbose(true).build()?)
    }

    fn login_context(&self) -> Result<Context, GolemError> {
        Ok(Context {
            base_url: self.cloud_url.clone(),
            client: self.client()?,
            security_token: Security::Empty,
        })
    }

    fn login_client(&self) -> Result<Box<dyn LoginClient + Send + Sync>, GolemError> {
        Ok(Box::new(LoginClientLive {
            client: golem_cloud_client::api::LoginClientLive {
                context: self.login_context()?,
            },
            context: self.login_context()?,
        }))
    }

    pub fn auth(&self) -> Result<Box<dyn Auth + Send + Sync>, GolemError> {
        Ok(Box::new(AuthLive {
            login: self.login_client()?,
        }))
    }

    fn cloud_context(&self, auth: &CloudAuthentication) -> Result<Context, GolemError> {
        Ok(Context {
            base_url: self.cloud_url.clone(),
            client: self.client()?,
            security_token: Security::Bearer(auth.0.secret.value.to_string()),
        })
    }

    fn component_context(&self, auth: &CloudAuthentication) -> Result<Context, GolemError> {
        Ok(Context {
            base_url: self.component_url.clone(),
            client: self.client()?,
            security_token: Security::Bearer(auth.0.secret.value.to_string()),
        })
    }

    fn worker_context(&self, auth: &CloudAuthentication) -> Result<Context, GolemError> {
        Ok(Context {
            base_url: self.worker_url.clone(),
            client: self.client()?,
            security_token: Security::Bearer(auth.0.secret.value.to_string()),
        })
    }

    fn project_client(
        &self,
        auth: &CloudAuthentication,
    ) -> Result<Box<dyn ProjectClient + Send + Sync>, GolemError> {
        Ok(Box::new(ProjectClientLive {
            client: golem_cloud_client::api::ProjectClientLive {
                context: self.cloud_context(auth)?,
            },
        }))
    }

    pub fn project_service(
        &self,
        auth: &CloudAuthentication,
    ) -> Result<Box<dyn ProjectService + Send + Sync + 'static>, GolemError> {
        Ok(Box::new(ProjectServiceLive {
            account_id: auth.account_id(),
            client: self.project_client(auth)?,
        }))
    }

    fn account_client(
        &self,
        auth: &CloudAuthentication,
    ) -> Result<Box<dyn AccountClient + Send + Sync>, GolemError> {
        Ok(Box::new(AccountClientLive {
            client: golem_cloud_client::api::AccountClientLive {
                context: self.cloud_context(auth)?,
            },
        }))
    }

    pub fn account_service(
        &self,
        auth: &CloudAuthentication,
    ) -> Result<Box<dyn AccountService + Send + Sync>, GolemError> {
        Ok(Box::new(AccountServiceLive {
            account_id: auth.account_id(),
            client: self.account_client(auth)?,
        }))
    }

    fn grant_client(
        &self,
        auth: &CloudAuthentication,
    ) -> Result<Box<dyn GrantClient + Send + Sync>, GolemError> {
        Ok(Box::new(GrantClientLive {
            client: golem_cloud_client::api::GrantClientLive {
                context: self.cloud_context(auth)?,
            },
        }))
    }

    pub fn grant_service(
        &self,
        auth: &CloudAuthentication,
    ) -> Result<Box<dyn GrantService + Send + Sync>, GolemError> {
        Ok(Box::new(GrantServiceLive {
            account_id: auth.account_id(),
            client: self.grant_client(auth)?,
        }))
    }

    fn token_client(
        &self,
        auth: &CloudAuthentication,
    ) -> Result<Box<dyn TokenClient + Send + Sync>, GolemError> {
        Ok(Box::new(TokenClientLive {
            client: golem_cloud_client::api::TokenClientLive {
                context: self.cloud_context(auth)?,
            },
        }))
    }

    pub fn token_service(
        &self,
        auth: &CloudAuthentication,
    ) -> Result<Box<dyn TokenService + Send + Sync>, GolemError> {
        Ok(Box::new(TokenServiceLive {
            account_id: auth.account_id(),
            client: self.token_client(auth)?,
        }))
    }

    fn project_grant_client(
        &self,
        auth: &CloudAuthentication,
    ) -> Result<Box<dyn ProjectGrantClient + Send + Sync>, GolemError> {
        Ok(Box::new(ProjectGrantClientLive {
            client: golem_cloud_client::api::ProjectGrantClientLive {
                context: self.cloud_context(auth)?,
            },
        }))
    }

    pub fn project_grant_service(
        &self,
        auth: &CloudAuthentication,
    ) -> Result<Box<dyn ProjectGrantService + Send + Sync>, GolemError> {
        Ok(Box::new(ProjectGrantServiceLive {
            client: self.project_grant_client(auth)?,
            projects: self.project_service(auth)?,
        }))
    }

    fn project_policy_client(
        &self,
        auth: &CloudAuthentication,
    ) -> Result<Box<dyn ProjectPolicyClient + Send + Sync>, GolemError> {
        Ok(Box::new(ProjectPolicyClientLive {
            client: golem_cloud_client::api::ProjectPolicyClientLive {
                context: self.cloud_context(auth)?,
            },
        }))
    }

    pub fn project_policy_service(
        &self,
        auth: &CloudAuthentication,
    ) -> Result<Box<dyn ProjectPolicyService + Send + Sync>, GolemError> {
        Ok(Box::new(ProjectPolicyServiceLive {
            client: self.project_policy_client(auth)?,
        }))
    }

    fn certificate_client(
        &self,
        auth: &CloudAuthentication,
    ) -> Result<Box<dyn CertificateClient + Send + Sync>, GolemError> {
        Ok(Box::new(CertificateClientLive {
            client: golem_cloud_client::api::ApiCertificateClientLive {
                context: self.worker_context(auth)?,
            },
        }))
    }

    pub fn certificate_service(
        &self,
        auth: &CloudAuthentication,
    ) -> Result<Box<dyn CertificateService + Send + Sync>, GolemError> {
        Ok(Box::new(CertificateServiceLive {
            client: self.certificate_client(auth)?,
            projects: self.project_service(auth)?,
        }))
    }

    fn domain_client(
        &self,
        auth: &CloudAuthentication,
    ) -> Result<Box<dyn DomainClient + Send + Sync>, GolemError> {
        Ok(Box::new(DomainClientLive {
            client: golem_cloud_client::api::ApiDomainClientLive {
                context: self.worker_context(auth)?,
            },
        }))
    }

    pub fn domain_service(
        &self,
        auth: &CloudAuthentication,
    ) -> Result<Box<dyn DomainService + Send + Sync>, GolemError> {
        Ok(Box::new(DomainServiceLive {
            client: self.domain_client(auth)?,
            projects: self.project_service(auth)?,
        }))
    }
}

impl ServiceFactory for CloudServiceFactory {
    type SecurityContext = CloudAuthentication;
    type ProjectRef = ProjectRef;
    type ProjectContext = ProjectId;

    fn with_auth(
        &self,
        auth: &Self::SecurityContext,
    ) -> FactoryWithAuth<Self::ProjectRef, Self::ProjectContext, Self::SecurityContext> {
        FactoryWithAuth {
            auth: auth.clone(),
            factory: Box::new(self.clone()),
        }
    }

    fn project_resolver(
        &self,
        auth: &Self::SecurityContext,
    ) -> Result<
        Arc<dyn ProjectResolver<Self::ProjectRef, Self::ProjectContext> + Send + Sync>,
        GolemError,
    > {
        Ok(Arc::new(CloudProjectResolver {
            service: self.project_service(auth)?,
        }))
    }

    fn component_client(
        &self,
        auth: &Self::SecurityContext,
    ) -> Result<
        Box<dyn ComponentClient<ProjectContext = Self::ProjectContext> + Send + Sync>,
        GolemError,
    > {
        Ok(Box::new(ComponentClientLive {
            client: golem_cloud_client::api::ComponentClientLive {
                context: self.component_context(auth)?,
            },
        }))
    }

    fn worker_client(
        &self,
        auth: &Self::SecurityContext,
    ) -> Result<Box<dyn WorkerClient + Send + Sync>, GolemError> {
        Ok(Box::new(WorkerClientLive {
            client: golem_cloud_client::api::WorkerClientLive {
                context: self.worker_context(auth)?,
            },
            context: self.worker_context(auth)?,
            allow_insecure: self.allow_insecure,
        }))
    }

    fn api_definition_client(
        &self,
        auth: &Self::SecurityContext,
    ) -> Result<
        Box<dyn ApiDefinitionClient<ProjectContext = Self::ProjectContext> + Send + Sync>,
        GolemError,
    > {
        Ok(Box::new(ApiDefinitionClientLive {
            client: golem_cloud_client::api::ApiDefinitionClientLive {
                context: self.worker_context(auth)?,
            },
        }))
    }

    fn api_deployment_client(
        &self,
        auth: &Self::SecurityContext,
    ) -> Result<
        Box<dyn ApiDeploymentClient<ProjectContext = Self::ProjectContext> + Send + Sync>,
        GolemError,
    > {
        Ok(Box::new(ApiDeploymentClientLive {
            client: golem_cloud_client::api::ApiDeploymentClientLive {
                context: self.worker_context(auth)?,
            },
        }))
    }

    fn health_check_clients(
        &self,
        auth: &Self::SecurityContext,
    ) -> Result<Vec<Box<dyn HealthCheckClient + Send + Sync>>, GolemError> {
        Ok(vec![
            Box::new(crate::cloud::clients::health_check::HealthCheckClientLive {
                client: golem_cloud_client::api::HealthCheckClientLive {
                    context: self.cloud_context(auth)?,
                },
            }),
            Box::new(crate::cloud::clients::health_check::HealthCheckClientLive {
                client: golem_cloud_client::api::HealthCheckClientLive {
                    context: self.component_context(auth)?,
                },
            }),
            Box::new(crate::cloud::clients::health_check::HealthCheckClientLive {
                client: golem_cloud_client::api::HealthCheckClientLive {
                    context: self.worker_context(auth)?,
                },
            }),
        ])
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
                let factory = CloudServiceFactory::from_profile(&profile);

                let _ = get_auth(None, profile_name, &profile, config_dir, &factory).await?;
                Ok(())
            }
        }
    }
}
