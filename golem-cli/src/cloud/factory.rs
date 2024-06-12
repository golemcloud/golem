// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::clients::api_definition::ApiDefinitionClient;
use crate::clients::api_deployment::ApiDeploymentClient;
use crate::clients::component::ComponentClient;
use crate::clients::health_check::HealthCheckClient;
use crate::clients::worker::WorkerClient;
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
use crate::cloud::model::{ProjectId, ProjectRef};
use crate::cloud::service::account::{AccountService, AccountServiceLive};
use crate::cloud::service::certificate::{CertificateService, CertificateServiceLive};
use crate::cloud::service::domain::{DomainService, DomainServiceLive};
use crate::cloud::service::grant::{GrantService, GrantServiceLive};
use crate::cloud::service::policy::{ProjectPolicyService, ProjectPolicyServiceLive};
use crate::cloud::service::project::{CloudProjectResolver, ProjectService, ProjectServiceLive};
use crate::cloud::service::project_grant::{ProjectGrantService, ProjectGrantServiceLive};
use crate::cloud::service::token::{TokenService, TokenServiceLive};
use crate::config::CloudProfile;
use crate::factory::{FactoryWithAuth, ServiceFactory};
use crate::model::GolemError;
use crate::service::project::ProjectResolver;
use golem_cloud_client::{Context, Security};
use url::Url;

#[derive(Debug, Clone)]
pub struct CloudServiceFactory {
    pub url: Url,
    pub worker_url: Url,
    pub allow_insecure: bool,
}

fn default_url() -> Url {
    Url::parse("https://release.api.golem.cloud/").unwrap()
}

impl CloudServiceFactory {
    pub fn from_profile(profile: &CloudProfile) -> Self {
        let url = profile.custom_url.clone().unwrap_or_else(default_url);
        let worker_url = profile
            .custom_worker_url
            .clone()
            .unwrap_or_else(|| url.clone());
        let allow_insecure = profile.allow_insecure;

        CloudServiceFactory {
            url,
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
            base_url: self.url.clone(),
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

    fn context(&self, auth: &CloudAuthentication) -> Result<Context, GolemError> {
        Ok(Context {
            base_url: self.url.clone(),
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
                context: self.context(auth)?,
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
                context: self.context(auth)?,
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
                context: self.context(auth)?,
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
                context: self.context(auth)?,
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
                context: self.context(auth)?,
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
                context: self.context(auth)?,
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
        Box<dyn ProjectResolver<Self::ProjectRef, Self::ProjectContext> + Send + Sync>,
        GolemError,
    > {
        Ok(Box::new(CloudProjectResolver {
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
                context: self.context(auth)?,
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
                    context: self.context(auth)?,
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
