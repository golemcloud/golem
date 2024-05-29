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

mod certificate;
mod definition;
mod deployment;
mod domain;
mod healthcheck;

use async_trait::async_trait;
use clap::Subcommand;
use golem_cloud_worker_client::{Context, Security};

use crate::clients::gateway::certificate::CertificateClientLive;
use crate::clients::gateway::definition::DefinitionClientLive;
use crate::clients::gateway::deployment::DeploymentClientLive;
use crate::clients::gateway::domain::DomainClientLive;
use crate::clients::gateway::healthcheck::HealthcheckClientLive;
use crate::clients::project::ProjectClient;
use crate::clients::CloudAuthentication;
use crate::gateway::certificate::{
    CertificateHandler, CertificateHandlerLive, CertificateSubcommand,
};
use crate::gateway::definition::{DefinitionHandler, DefinitionHandlerLive, DefinitionSubcommand};
use crate::gateway::deployment::{DeploymentHandler, DeploymentHandlerLive, DeploymentSubcommand};
use crate::gateway::domain::{DomainHandler, DomainHandlerLive, DomainSubcommand};
use crate::gateway::healthcheck::{HealthcheckHandler, HealthcheckHandlerLive};
use crate::model::{Format, GolemError, GolemResult};

#[derive(Subcommand, Debug)]
#[command()]
pub enum GatewaySubcommand {
    #[command()]
    Certificate {
        #[command(subcommand)]
        subcommand: CertificateSubcommand,
    },
    #[command()]
    Definition {
        #[command(subcommand)]
        subcommand: DefinitionSubcommand,
    },
    #[command()]
    Deployment {
        #[command(subcommand)]
        subcommand: DeploymentSubcommand,
    },
    #[command()]
    Domain {
        #[command(subcommand)]
        subcommand: DomainSubcommand,
    },
    #[command()]
    Healthcheck {},
}

#[async_trait]
pub trait GatewayHandler {
    async fn handle(
        &self,
        format: Format,
        token: &CloudAuthentication,
        subcommand: GatewaySubcommand,
    ) -> Result<GolemResult, GolemError>;
}

pub struct GatewayHandlerLive<'p, P: ProjectClient + Sync + Send> {
    pub base_url: reqwest::Url,
    pub client: reqwest::Client,
    pub projects: &'p P,
}

#[async_trait]
impl<'p, P: ProjectClient + Sync + Send> GatewayHandler for GatewayHandlerLive<'p, P> {
    async fn handle(
        &self,
        format: Format,
        auth: &CloudAuthentication,
        subcommand: GatewaySubcommand,
    ) -> Result<GolemResult, GolemError> {
        let context = Context {
            base_url: self.base_url.clone(),
            client: self.client.clone(),
            security_token: Security::Bearer(auth.0.secret.value.to_string()),
        };

        let healthcheck_client = HealthcheckClientLive {
            client: golem_cloud_worker_client::api::HealthCheckClientLive {
                context: context.clone(),
            },
        };
        let healthcheck_srv = HealthcheckHandlerLive {
            healthcheck: healthcheck_client,
        };

        let deployment_client = DeploymentClientLive {
            client: golem_cloud_worker_client::api::ApiDeploymentClientLive {
                context: context.clone(),
            },
        };
        let deployment_srv = DeploymentHandlerLive {
            client: deployment_client,
            projects: self.projects,
        };

        let definition_client = DefinitionClientLive {
            client: golem_cloud_worker_client::api::ApiDefinitionClientLive {
                context: context.clone(),
            },
        };
        let definition_srv = DefinitionHandlerLive {
            client: definition_client,
            projects: self.projects,
        };

        let certificate_client = CertificateClientLive {
            client: golem_cloud_worker_client::api::ApiCertificateClientLive {
                context: context.clone(),
            },
        };
        let certificate_srv = CertificateHandlerLive {
            client: certificate_client,
            projects: self.projects,
        };

        let domain_client = DomainClientLive {
            client: golem_cloud_worker_client::api::ApiDomainClientLive {
                context: context.clone(),
            },
        };
        let domain_srv = DomainHandlerLive {
            client: domain_client,
            projects: self.projects,
        };

        match subcommand {
            GatewaySubcommand::Certificate { subcommand } => {
                certificate_srv.handle(subcommand).await
            }
            GatewaySubcommand::Definition { subcommand } => {
                definition_srv.handle(format, subcommand).await
            }
            GatewaySubcommand::Deployment { subcommand } => deployment_srv.handle(subcommand).await,
            GatewaySubcommand::Domain { subcommand } => domain_srv.handle(subcommand).await,
            GatewaySubcommand::Healthcheck {} => healthcheck_srv.handle().await,
        }
    }
}
