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

use async_trait::async_trait;
use clap::Subcommand;

use crate::clients::policy::ProjectPolicyClient;
use crate::model::{GolemError, GolemResult, ProjectAction, ProjectPolicyId};

#[derive(Subcommand, Debug)]
#[command()]
pub enum ProjectPolicySubcommand {
    /// Creates a new project sharing policy
    #[command()]
    Add {
        /// Name of the policy
        #[arg(long)]
        project_policy_name: String,

        /// List of actions allowed by the policy
        #[arg(value_name = "Actions")]
        project_actions: Vec<ProjectAction>,
    },

    /// Gets the existing project sharing policies
    #[command()]
    Get {
        #[arg(value_name = "ID")]
        project_policy_id: ProjectPolicyId,
    },
}

#[async_trait]
pub trait ProjectPolicyHandler {
    async fn handle(&self, subcommand: ProjectPolicySubcommand) -> Result<GolemResult, GolemError>;
}

pub struct ProjectPolicyHandlerLive<C: ProjectPolicyClient + Send + Sync> {
    pub client: C,
}

#[async_trait]
impl<C: ProjectPolicyClient + Send + Sync> ProjectPolicyHandler for ProjectPolicyHandlerLive<C> {
    async fn handle(&self, subcommand: ProjectPolicySubcommand) -> Result<GolemResult, GolemError> {
        match subcommand {
            ProjectPolicySubcommand::Add {
                project_actions,
                project_policy_name,
            } => {
                let policy = self
                    .client
                    .create(project_policy_name, project_actions)
                    .await?;

                Ok(GolemResult::Ok(Box::new(policy)))
            }
            ProjectPolicySubcommand::Get { project_policy_id } => {
                let policy = self.client.get(project_policy_id).await?;

                Ok(GolemResult::Ok(Box::new(policy)))
            }
        }
    }
}
