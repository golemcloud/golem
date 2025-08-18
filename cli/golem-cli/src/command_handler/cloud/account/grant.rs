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

use crate::command::cloud::account::grant::GrantSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::model::text::account::GrantGetView;
use crate::model::AccountId;
use crate::model::Role;

use crate::log::log_action;
use golem_client::api::GrantClient;
use std::sync::Arc;

pub struct CloudAccountGrantCommandHandler {
    ctx: Arc<Context>,
}

impl CloudAccountGrantCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: GrantSubcommand) -> anyhow::Result<()> {
        match subcommand {
            GrantSubcommand::Get { account_id } => self.cmd_get(account_id.account_id).await,
            GrantSubcommand::New { account_id, role } => {
                self.cmd_new(account_id.account_id, role).await
            }
            GrantSubcommand::Delete { account_id, role } => {
                self.cmd_delete(account_id.account_id, role).await
            }
        }
    }

    async fn cmd_get(&self, account_id: Option<AccountId>) -> anyhow::Result<()> {
        let roles = self
            .ctx
            .golem_clients()
            .await?
            .grant
            .get_account_grants(
                &self
                    .ctx
                    .cloud_account_handler()
                    .select_account_id_or_err(account_id)
                    .await?
                    .0,
            )
            .await
            .map_service_error()?;

        self.ctx.log_handler().log_view(&GrantGetView(roles));

        Ok(())
    }

    async fn cmd_new(&self, account_id: Option<AccountId>, role: Role) -> anyhow::Result<()> {
        self.ctx
            .golem_clients()
            .await?
            .grant
            .create_account_grant(
                &self
                    .ctx
                    .cloud_account_handler()
                    .select_account_id_or_err(account_id)
                    .await?
                    .0,
                &role.into(),
            )
            .await
            .map_service_error()?;

        log_action("Granted", format!("role {role}"));

        Ok(())
    }

    async fn cmd_delete(&self, account_id: Option<AccountId>, role: Role) -> anyhow::Result<()> {
        self.ctx
            .golem_clients()
            .await?
            .grant
            .delete_account_grant(
                &self
                    .ctx
                    .cloud_account_handler()
                    .select_account_id_or_err(account_id)
                    .await?
                    .0,
                &role.into(),
            )
            .await
            .map_service_error()?;

        log_action("Deleted", format!("role {role}"));

        Ok(())
    }
}
