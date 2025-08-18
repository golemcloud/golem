// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::command::cloud::project::policy::PolicySubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::model::text::project::{ProjectPolicyGetView, ProjectPolicyNewView};
use crate::model::ProjectPolicyId;
use golem_client::api::ProjectPolicyClient;
use golem_client::model::{ProjectActions, ProjectPolicyData};
use golem_common::model::auth::ProjectPermission;
use std::collections::HashSet;
use std::sync::Arc;

pub struct CloudProjectPolicyCommandHandler {
    ctx: Arc<Context>,
}

impl CloudProjectPolicyCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handler_command(&self, subcommand: PolicySubcommand) -> anyhow::Result<()> {
        match subcommand {
            PolicySubcommand::New {
                policy_name,
                actions,
            } => self.cmd_new(policy_name, actions).await,
            PolicySubcommand::Get { policy_id } => self.cmd_get(policy_id).await,
        }
    }

    async fn cmd_new(
        &self,
        policy_name: String,
        actions: Vec<ProjectPermission>,
    ) -> anyhow::Result<()> {
        let policy = self
            .ctx
            .golem_clients()
            .await?
            .project_policy
            .create_project_policy(&ProjectPolicyData {
                name: policy_name,
                project_actions: ProjectActions {
                    actions: HashSet::from_iter(actions),
                },
            })
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_view(&ProjectPolicyNewView(policy));

        Ok(())
    }

    async fn cmd_get(&self, policy_id: ProjectPolicyId) -> anyhow::Result<()> {
        let policy = self
            .ctx
            .golem_clients()
            .await?
            .project_policy
            .get_project_policies(&policy_id.0)
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_view(&ProjectPolicyGetView(policy));

        Ok(())
    }
}
