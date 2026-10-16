// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::command::account::{PermissionShareGrantArgs, PermissionShareSubcommand};
use crate::command::shared_args::AccountScopeOptionalArgs;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::MapServiceError;
use crate::model::account::{
    PermissionShareDeleteView, PermissionShareGetView, PermissionShareListView,
    PermissionShareNewView, PermissionShareUpdateView,
};
use golem_client::api::PermissionSharesClient;
use golem_client::model::{PermissionShare, PermissionShareCreation, PermissionShareUpdate};
use golem_common::model::account::AccountEmail;
use golem_common::model::permission_share::{
    PermissionShareData, PermissionShareId, PermissionShareName,
};
use std::sync::Arc;

pub struct AccountPermissionShareCommandHandler {
    ctx: Arc<Context>,
}

impl AccountPermissionShareCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(
        &self,
        subcommand: PermissionShareSubcommand,
    ) -> anyhow::Result<()> {
        match subcommand {
            PermissionShareSubcommand::List { account, received } => {
                self.cmd_list(account, received).await
            }
            PermissionShareSubcommand::Get {
                permission_share_id,
            } => self.cmd_get(permission_share_id).await,
            PermissionShareSubcommand::GetByName { account, name } => {
                self.cmd_get_by_name(account, name).await
            }
            PermissionShareSubcommand::New {
                account,
                target_account_email,
                name,
                grants,
            } => {
                self.cmd_new(account, target_account_email, name, grants)
                    .await
            }
            PermissionShareSubcommand::Update {
                permission_share_id,
                name,
                grants,
            } => self.cmd_update(permission_share_id, name, grants).await,
            PermissionShareSubcommand::Delete {
                permission_share_id,
            } => self.cmd_delete(permission_share_id).await,
        }
    }

    async fn cmd_list(
        &self,
        account: AccountScopeOptionalArgs,
        received: bool,
    ) -> anyhow::Result<()> {
        let account_id = self
            .ctx
            .account_handler()
            .select_account_id_or_err(account)
            .await?;
        let shares = if received {
            self.ctx
                .golem_clients()
                .await?
                .permission_shares
                .list_received_permission_shares(&account_id.0)
                .await
                .map_service_error()?
                .values
        } else {
            self.ctx
                .golem_clients()
                .await?
                .permission_shares
                .list_owned_permission_shares(&account_id.0)
                .await
                .map_service_error()?
                .values
        };

        self.ctx.log_handler().log_output(PermissionShareListView {
            permission_shares: shares,
        })?;

        Ok(())
    }

    async fn cmd_get(&self, permission_share_id: PermissionShareId) -> anyhow::Result<()> {
        let share = self.get_permission_share(permission_share_id).await?;
        self.ctx
            .log_handler()
            .log_output(PermissionShareGetView(share))?;

        Ok(())
    }

    async fn cmd_get_by_name(
        &self,
        account: AccountScopeOptionalArgs,
        name: String,
    ) -> anyhow::Result<()> {
        let account_id = self
            .ctx
            .account_handler()
            .select_account_id_or_err(account)
            .await?;
        let share = self
            .ctx
            .golem_clients()
            .await?
            .permission_shares
            .get_permission_share_by_name(&account_id.0, &name)
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_output(PermissionShareGetView(share))?;

        Ok(())
    }

    async fn cmd_new(
        &self,
        account: AccountScopeOptionalArgs,
        target_account_email: String,
        name: String,
        grants: PermissionShareGrantArgs,
    ) -> anyhow::Result<()> {
        let account_id = self
            .ctx
            .account_handler()
            .select_account_id_or_err(account)
            .await?;
        let share = self
            .ctx
            .golem_clients()
            .await?
            .permission_shares
            .create_permission_share(
                &account_id.0,
                &PermissionShareCreation {
                    target_account_email: AccountEmail::new(target_account_email),
                    name: PermissionShareName(name),
                    data: permission_share_data(grants),
                },
            )
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_output(PermissionShareNewView(share))?;

        Ok(())
    }

    async fn cmd_update(
        &self,
        permission_share_id: PermissionShareId,
        name: Option<String>,
        grants: PermissionShareGrantArgs,
    ) -> anyhow::Result<()> {
        let current = self.get_permission_share(permission_share_id).await?;
        let data = permission_share_data_update(grants, current.data);
        let share = self
            .ctx
            .golem_clients()
            .await?
            .permission_shares
            .update_permission_share(
                &permission_share_id.0,
                &PermissionShareUpdate {
                    current_revision: current.revision,
                    name: name.map(PermissionShareName).unwrap_or(current.name),
                    data,
                },
            )
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_output(PermissionShareUpdateView(share))?;

        Ok(())
    }

    async fn cmd_delete(&self, permission_share_id: PermissionShareId) -> anyhow::Result<()> {
        let share = self.get_permission_share(permission_share_id).await?;
        self.ctx
            .golem_clients()
            .await?
            .permission_shares
            .delete_permission_share(&permission_share_id.0, share.revision.into())
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_output(PermissionShareDeleteView {
                deleted: true,
                permission_share_id,
            })?;

        Ok(())
    }

    async fn get_permission_share(
        &self,
        permission_share_id: PermissionShareId,
    ) -> anyhow::Result<PermissionShare> {
        Ok(self
            .ctx
            .golem_clients()
            .await?
            .permission_shares
            .get_permission_share(&permission_share_id.0)
            .await
            .map_service_error()?)
    }
}

fn permission_share_data(grants: PermissionShareGrantArgs) -> PermissionShareData {
    PermissionShareData {
        lower_positive: grants.lower_positive.unwrap_or_default(),
        lower_negative: grants.lower_negative.unwrap_or_default(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
    }
}

fn permission_share_data_update(
    grants: PermissionShareGrantArgs,
    current: PermissionShareData,
) -> PermissionShareData {
    PermissionShareData {
        lower_positive: grants.lower_positive.unwrap_or(current.lower_positive),
        lower_negative: grants.lower_negative.unwrap_or(current.lower_negative),
        upper_positive: current.upper_positive,
        upper_negative: current.upper_negative,
    }
}
