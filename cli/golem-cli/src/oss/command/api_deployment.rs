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

use crate::model::{ApiDefinitionId, ApiDefinitionVersion, GolemError, GolemResult};
use crate::oss::model::OssContext;
use crate::service::api_deployment::ApiDeploymentService;
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

impl ApiDeploymentSubcommand {
    pub async fn handle(
        self,
        service: &(dyn ApiDeploymentService<ProjectContext = OssContext> + Send + Sync),
    ) -> Result<GolemResult, GolemError> {
        let ctx = &OssContext::EMPTY;

        match self {
            ApiDeploymentSubcommand::Deploy {
                id,
                version,
                host,
                subdomain,
            } => service.deploy(id, version, host, subdomain, ctx).await,
            ApiDeploymentSubcommand::Get { site } => service.get(site).await,
            ApiDeploymentSubcommand::List { id } => service.list(id, ctx).await,
            ApiDeploymentSubcommand::Delete { site } => service.delete(site).await,
        }
    }
}
