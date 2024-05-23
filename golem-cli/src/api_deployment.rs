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

use crate::clients::api_deployment::ApiDeploymentClient;
use crate::model::{ApiDefinitionId, ApiDefinitionVersion, GolemError, GolemResult};
use async_trait::async_trait;
use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command()]
pub enum ApiDeploymentSubcommand {
    /// Create or update deployment
    #[command()]
    Deploy {
        /// Api definition id
        #[arg(short, long)]
        id: ApiDefinitionId,

        /// Api definition version
        #[arg(short = 'V', long)]
        version: ApiDefinitionVersion,

        #[arg(short = 'H', long)]
        host: String,

        #[arg(short, long)]
        subdomain: Option<String>,
    },

    /// Get api deployment
    #[command()]
    Get {
        /// Deployment site
        #[arg(value_name = "subdomain.host")]
        site: String,
    },

    /// List api deployment for api definition
    #[command()]
    List {
        /// Api definition id
        #[arg(short, long)]
        id: ApiDefinitionId,
    },

    /// Delete api deployment
    #[command()]
    Delete {
        /// Deployment site
        #[arg(value_name = "subdomain.host")]
        site: String,
    },
}

#[async_trait]
pub trait ApiDeploymentHandler {
    async fn handle(&self, subcommand: ApiDeploymentSubcommand) -> Result<GolemResult, GolemError>;
}

pub struct ApiDeploymentHandlerLive<C: ApiDeploymentClient + Send + Sync> {
    pub client: C,
}

#[async_trait]
impl<C: ApiDeploymentClient + Send + Sync> ApiDeploymentHandler for ApiDeploymentHandlerLive<C> {
    async fn handle(&self, subcommand: ApiDeploymentSubcommand) -> Result<GolemResult, GolemError> {
        match subcommand {
            ApiDeploymentSubcommand::Deploy {
                id,
                version,
                host,
                subdomain,
            } => {
                let deployment = self.client.deploy(&id, &version, &host, subdomain).await?;

                Ok(GolemResult::Ok(Box::new(deployment)))
            }
            ApiDeploymentSubcommand::Get { site } => {
                let deployment = self.client.get(&site).await?;

                Ok(GolemResult::Ok(Box::new(deployment)))
            }
            ApiDeploymentSubcommand::List { id } => {
                let deployments = self.client.list(&id).await?;

                Ok(GolemResult::Ok(Box::new(deployments)))
            }
            ApiDeploymentSubcommand::Delete { site } => {
                let res = self.client.delete(&site).await?;

                Ok(GolemResult::Str(res))
            }
        }
    }
}
