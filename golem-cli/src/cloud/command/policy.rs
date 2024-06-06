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

use clap::Subcommand;

use crate::cloud::model::{ProjectAction, ProjectPolicyId};
use crate::cloud::service::policy::ProjectPolicyService;
use crate::model::{GolemError, GolemResult};

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

impl ProjectPolicySubcommand {
    pub async fn handle(
        self,
        service: &(dyn ProjectPolicyService + Send + Sync),
    ) -> Result<GolemResult, GolemError> {
        match self {
            ProjectPolicySubcommand::Add {
                project_actions,
                project_policy_name,
            } => service.add(project_policy_name, project_actions).await,
            ProjectPolicySubcommand::Get { project_policy_id } => {
                service.get(project_policy_id).await
            }
        }
    }
}
