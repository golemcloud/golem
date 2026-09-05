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

use crate::command::account::{
    AccountLimitsSubcommand, AccountSubcommand, AccountUsageSubcommand, PermissionShareGrantArgs,
    PermissionShareSubcommand,
};
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::NonSuccessfulExit;
use crate::error::service::MapServiceError;
use crate::model::account::{
    AccountDeleteView, AccountGetView, AccountLimitsView, AccountNewView, AccountUpdateView,
    AccountUsageListView, AccountUsageView, PermissionShareDeleteView, PermissionShareGetView,
    PermissionShareListView, PermissionShareNewView, PermissionShareUpdateView,
};
use anyhow::bail;
use golem_client::api::{AccountClient, PermissionSharesClient};
use golem_client::model::{
    Account, AccountCreation, AccountUpdate, PermissionShare, PermissionShareCreation,
    PermissionShareUpdate,
};
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::account_usage::{AccountUsagePeriod, SetMemoryLimit, SetStorageLimit};
use golem_common::model::permission_share::{
    PermissionShareData, PermissionShareId, PermissionShareName,
};
use std::sync::Arc;

pub struct AccountCommandHandler {
    ctx: Arc<Context>,
}

impl AccountCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: AccountSubcommand) -> anyhow::Result<()> {
        match subcommand {
            AccountSubcommand::Get { account_id } => self.cmd_get(account_id.account_id).await,
            AccountSubcommand::Update {
                account_id,
                account_name,
            } => self.cmd_update(account_id.account_id, account_name).await,
            AccountSubcommand::New {
                account_name,
                account_email,
            } => self.cmd_new(account_name, account_email).await,
            AccountSubcommand::Delete { account_id } => {
                self.cmd_delete(account_id.account_id).await
            }
            AccountSubcommand::Usage { subcommand } => self.handle_usage_command(subcommand).await,
            AccountSubcommand::Limits { subcommand } => {
                self.handle_limits_command(subcommand).await
            }
            AccountSubcommand::PermissionShare { subcommand } => {
                self.handle_permission_share_command(subcommand).await
            }
        }
    }

    async fn handle_usage_command(&self, subcommand: AccountUsageSubcommand) -> anyhow::Result<()> {
        match subcommand {
            AccountUsageSubcommand::Show { account_id, period } => {
                self.cmd_usage_show(account_id.account_id, period).await
            }
            AccountUsageSubcommand::History { account_id, last } => {
                self.cmd_usage_history(account_id.account_id, last).await
            }
        }
    }

    async fn handle_limits_command(
        &self,
        subcommand: AccountLimitsSubcommand,
    ) -> anyhow::Result<()> {
        match subcommand {
            AccountLimitsSubcommand::Show { account_id } => {
                self.cmd_limits_show(account_id.account_id).await
            }
            AccountLimitsSubcommand::Set {
                account_id,
                max_storage_per_agent,
                max_memory_per_agent,
                monthly_memory_gb_seconds,
            } => {
                self.cmd_limits_set(
                    account_id.account_id,
                    max_storage_per_agent,
                    max_memory_per_agent,
                    monthly_memory_gb_seconds,
                )
                .await
            }
            AccountLimitsSubcommand::Unset {
                account_id,
                storage,
                max_memory_per_agent,
                monthly_memory_gb_seconds,
            } => {
                self.cmd_limits_unset(
                    account_id.account_id,
                    storage,
                    max_memory_per_agent,
                    monthly_memory_gb_seconds,
                )
                .await
            }
        }
    }

    async fn handle_permission_share_command(
        &self,
        subcommand: PermissionShareSubcommand,
    ) -> anyhow::Result<()> {
        match subcommand {
            PermissionShareSubcommand::List {
                account_id,
                received,
            } => {
                self.cmd_permission_share_list(account_id.account_id, received)
                    .await
            }
            PermissionShareSubcommand::Get {
                permission_share_id,
            } => self.cmd_permission_share_get(permission_share_id).await,
            PermissionShareSubcommand::GetByName { account_id, name } => {
                self.cmd_permission_share_get_by_name(account_id.account_id, name)
                    .await
            }
            PermissionShareSubcommand::New {
                account_id,
                target_account_email,
                name,
                grants,
            } => {
                self.cmd_permission_share_new(
                    account_id.account_id,
                    target_account_email,
                    name,
                    grants,
                )
                .await
            }
            PermissionShareSubcommand::Update {
                permission_share_id,
                name,
                grants,
            } => {
                self.cmd_permission_share_update(permission_share_id, name, grants)
                    .await
            }
            PermissionShareSubcommand::Delete {
                permission_share_id,
            } => self.cmd_permission_share_delete(permission_share_id).await,
        }
    }

    async fn cmd_get(&self, account_id: Option<AccountId>) -> anyhow::Result<()> {
        let account = self.get(account_id).await?;
        self.ctx.log_handler().log_output(AccountGetView(account))?;

        Ok(())
    }

    async fn cmd_update(
        &self,
        account_id: Option<AccountId>,
        account_name: String,
    ) -> anyhow::Result<()> {
        let account = self.get(account_id).await?;
        let account = self
            .ctx
            .golem_clients()
            .await?
            .account
            .update_account(
                &account.id.0,
                &AccountUpdate {
                    current_revision: account.revision,
                    name: Some(account_name),
                },
            )
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_output(AccountUpdateView(account))?;

        Ok(())
    }

    async fn cmd_new(&self, account_name: String, account_email: String) -> anyhow::Result<()> {
        let account = self
            .ctx
            .golem_clients()
            .await?
            .account
            .create_account(&AccountCreation {
                name: account_name,
                email: AccountEmail::new(account_email),
                roles: Vec::new(),
            })
            .await
            .map_service_error()?;

        self.ctx.log_handler().log_output(AccountNewView(account))?;

        Ok(())
    }

    async fn cmd_delete(&self, account_id: Option<AccountId>) -> anyhow::Result<()> {
        let account = self.get(account_id).await?;
        if !self
            .ctx
            .interactive_handler()
            .confirm_delete_account(&account)?
        {
            bail!(NonSuccessfulExit)
        }

        self.ctx
            .golem_clients()
            .await?
            .account
            .delete_account(&account.id.0, account.revision.into())
            .await
            .map_service_error()?;

        self.ctx.log_handler().log_output(AccountDeleteView {
            deleted: true,
            account_id: account.id,
        })?;

        Ok(())
    }

    async fn cmd_usage_show(
        &self,
        account_id: Option<AccountId>,
        period: Option<AccountUsagePeriod>,
    ) -> anyhow::Result<()> {
        let account_id = self.select_account_id_or_err(account_id).await?;
        let period = period.map(|period| period.to_string());
        let usage = self
            .ctx
            .golem_clients()
            .await?
            .account
            .get_account_usage(&account_id.0, period.as_deref())
            .await
            .map_service_error()?;
        self.ctx
            .log_handler()
            .log_output(AccountUsageView::from(usage))?;
        Ok(())
    }

    async fn cmd_usage_history(
        &self,
        account_id: Option<AccountId>,
        last: usize,
    ) -> anyhow::Result<()> {
        let account_id = self.select_account_id_or_err(account_id).await?;
        let last = last.try_into()?;
        let usage = self
            .ctx
            .golem_clients()
            .await?
            .account
            .get_account_usage_history(&account_id.0, Some(last))
            .await
            .map_service_error()?
            .into_iter()
            .map(AccountUsageView::from)
            .collect();
        self.ctx
            .log_handler()
            .log_output(AccountUsageListView { usage })?;
        Ok(())
    }

    async fn cmd_limits_show(&self, account_id: Option<AccountId>) -> anyhow::Result<()> {
        let account_id = self.select_account_id_or_err(account_id).await?;
        let clients = self.ctx.golem_clients().await?;
        let storage = clients
            .account
            .get_account_storage_override(&account_id.0)
            .await
            .map_service_error()?;
        let max_memory = clients
            .account
            .get_account_max_memory_override(&account_id.0)
            .await
            .map_service_error()?;
        let monthly_memory = clients
            .account
            .get_account_monthly_memory_override(&account_id.0)
            .await
            .map_service_error()?;
        self.ctx.log_handler().log_output(AccountLimitsView::new(
            storage,
            max_memory,
            monthly_memory,
        ))?;
        Ok(())
    }

    async fn cmd_limits_set(
        &self,
        account_id: Option<AccountId>,
        storage: Option<u64>,
        max_memory: Option<u64>,
        monthly_memory: Option<u64>,
    ) -> anyhow::Result<()> {
        if storage.is_none() && max_memory.is_none() && monthly_memory.is_none() {
            bail!("at least one limit must be provided");
        }
        if storage.is_some() as u8 + max_memory.is_some() as u8 + monthly_memory.is_some() as u8 > 1
        {
            bail!("only one limit can be changed per command");
        }
        let account_id = self.select_account_id_or_err(account_id).await?;
        let clients = self.ctx.golem_clients().await?;
        if let Some(value) = storage {
            clients
                .account
                .set_account_storage_override(
                    &account_id.0,
                    &SetStorageLimit {
                        value,
                        expires_at: None,
                    },
                )
                .await
                .map_service_error()?;
        }
        if let Some(value) = max_memory {
            clients
                .account
                .set_account_max_memory_override(
                    &account_id.0,
                    &SetMemoryLimit {
                        value,
                        expires_at: None,
                    },
                )
                .await
                .map_service_error()?;
        }
        if let Some(value) = monthly_memory {
            clients
                .account
                .set_account_monthly_memory_override(
                    &account_id.0,
                    &SetMemoryLimit {
                        value,
                        expires_at: None,
                    },
                )
                .await
                .map_service_error()?;
        }
        self.cmd_limits_show(Some(account_id)).await
    }

    async fn cmd_limits_unset(
        &self,
        account_id: Option<AccountId>,
        storage: bool,
        max_memory: bool,
        monthly_memory: bool,
    ) -> anyhow::Result<()> {
        if storage as u8 + max_memory as u8 + monthly_memory as u8 > 1 {
            bail!("only one limit can be changed per command");
        }
        let account_id = self.select_account_id_or_err(account_id).await?;
        let clients = self.ctx.golem_clients().await?;
        if storage || (!max_memory && !monthly_memory) {
            clients
                .account
                .clear_account_storage_override(&account_id.0)
                .await
                .map_service_error()?;
        }
        if max_memory {
            clients
                .account
                .clear_account_max_memory_override(&account_id.0)
                .await
                .map_service_error()?;
        }
        if monthly_memory {
            clients
                .account
                .clear_account_monthly_memory_override(&account_id.0)
                .await
                .map_service_error()?;
        }
        self.cmd_limits_show(Some(account_id)).await
    }

    async fn cmd_permission_share_list(
        &self,
        account_id: Option<AccountId>,
        received: bool,
    ) -> anyhow::Result<()> {
        let account_id = self.select_account_id_or_err(account_id).await?;
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

    async fn cmd_permission_share_get(
        &self,
        permission_share_id: PermissionShareId,
    ) -> anyhow::Result<()> {
        let share = self.get_permission_share(permission_share_id).await?;
        self.ctx
            .log_handler()
            .log_output(PermissionShareGetView(share))?;

        Ok(())
    }

    async fn cmd_permission_share_get_by_name(
        &self,
        account_id: Option<AccountId>,
        name: String,
    ) -> anyhow::Result<()> {
        let account_id = self.select_account_id_or_err(account_id).await?;
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

    async fn cmd_permission_share_new(
        &self,
        account_id: Option<AccountId>,
        target_account_email: String,
        name: String,
        grants: PermissionShareGrantArgs,
    ) -> anyhow::Result<()> {
        let account_id = self.select_account_id_or_err(account_id).await?;
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

    async fn cmd_permission_share_update(
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

    async fn cmd_permission_share_delete(
        &self,
        permission_share_id: PermissionShareId,
    ) -> anyhow::Result<()> {
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

    async fn get(&self, account_id: Option<AccountId>) -> anyhow::Result<Account> {
        Ok(self
            .ctx
            .golem_clients()
            .await?
            .account
            .get_account(&self.select_account_id_or_err(account_id).await?.0)
            .await
            .map_service_error()?)
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

    pub async fn account_id_or_err(&self) -> anyhow::Result<AccountId> {
        Ok(*self.ctx.golem_clients().await?.account_id())
    }

    pub async fn select_account_id_or_err(
        &self,
        account_id: Option<AccountId>,
    ) -> anyhow::Result<AccountId> {
        match account_id {
            Some(account_id) => Ok(account_id),
            None => Ok(self.account_id_or_err().await?),
        }
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
