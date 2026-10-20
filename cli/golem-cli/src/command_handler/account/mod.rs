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

pub mod limits;
pub mod permission_share;
pub mod usage;

use crate::command::account::AccountSubcommand;
use crate::command::shared_args::AccountScopeOptionalArgs;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::NonSuccessfulExit;
use crate::error::service::MapServiceError;
use crate::model::account::{AccountDeleteView, AccountGetView, AccountNewView, AccountUpdateView};
use anyhow::bail;
use golem_client::api::AccountClient;
use golem_client::model::{Account, AccountCreation, AccountUpdate};
use golem_common::model::account::{AccountEmail, AccountId};
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
            AccountSubcommand::Get { account } => self.cmd_get(account).await,
            AccountSubcommand::Update {
                account,
                account_name,
            } => self.cmd_update(account, account_name).await,
            AccountSubcommand::New {
                account_name,
                account_email,
            } => self.cmd_new(account_name, account_email).await,
            AccountSubcommand::Delete { account } => self.cmd_delete(account).await,
            AccountSubcommand::Usage { subcommand } => {
                self.ctx
                    .account_usage_handler()
                    .handle_command(subcommand)
                    .await
            }
            AccountSubcommand::Limits { subcommand } => {
                self.ctx
                    .account_limits_handler()
                    .handle_command(subcommand)
                    .await
            }
            AccountSubcommand::PermissionShare { subcommand } => {
                self.ctx
                    .account_permission_share_handler()
                    .handle_command(subcommand)
                    .await
            }
        }
    }

    async fn cmd_get(&self, account: AccountScopeOptionalArgs) -> anyhow::Result<()> {
        let account = self.get(account).await?;
        self.ctx.log_handler().log_output(AccountGetView(account))?;

        Ok(())
    }

    async fn cmd_update(
        &self,
        account: AccountScopeOptionalArgs,
        account_name: String,
    ) -> anyhow::Result<()> {
        let account = self.get(account).await?;
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

    async fn cmd_delete(&self, account: AccountScopeOptionalArgs) -> anyhow::Result<()> {
        let account = self.get(account).await?;
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

    async fn get(&self, account: AccountScopeOptionalArgs) -> anyhow::Result<Account> {
        self.select_account_or_err(account).await
    }

    pub async fn account_id_or_err(&self) -> anyhow::Result<AccountId> {
        Ok(*self.ctx.golem_clients().await?.account_id())
    }

    pub async fn select_account_id_or_err(
        &self,
        account: AccountScopeOptionalArgs,
    ) -> anyhow::Result<AccountId> {
        match (account.account, account.account_id) {
            (Some(email), None) => Ok(self
                .ctx
                .golem_clients()
                .await?
                .account
                .get_account_by_email(&email)
                .await
                .map_service_error()?
                .id),
            (None, Some(account_id)) => Ok(account_id),
            (None, None) => Ok(self.account_id_or_err().await?),
            (Some(_), Some(_)) => unreachable!("clap rejects conflicting account scope flags"),
        }
    }

    /// Resolves the account scope *without* turning an email into an id up front.
    ///
    /// Commands backed by a resource endpoint that also accepts the owner email (e.g. the
    /// by-email plugin lookup) should use this and dispatch on the result, so that
    /// `--account <email>` does not require `AccountVerb::View` the way resolving through
    /// `get_account_by_email` would — keeping it on par with `--account-id`.
    pub async fn select_account_scope_or_err(
        &self,
        account: AccountScopeOptionalArgs,
    ) -> anyhow::Result<AccountScope> {
        match (account.account, account.account_id) {
            (Some(email), None) => Ok(AccountScope::Email(email)),
            (None, Some(account_id)) => Ok(AccountScope::Id(account_id)),
            (None, None) => Ok(AccountScope::Id(self.account_id_or_err().await?)),
            (Some(_), Some(_)) => unreachable!("clap rejects conflicting account scope flags"),
        }
    }

    pub async fn select_account_or_err(
        &self,
        account: AccountScopeOptionalArgs,
    ) -> anyhow::Result<Account> {
        let clients = self.ctx.golem_clients().await?;
        match (account.account, account.account_id) {
            (Some(email), None) => Ok(clients
                .account
                .get_account_by_email(&email)
                .await
                .map_service_error()?),
            (None, Some(account_id)) => Ok(clients
                .account
                .get_account(&account_id.0)
                .await
                .map_service_error()?),
            (None, None) => Ok(clients
                .account
                .get_account(&clients.account_id().0)
                .await
                .map_service_error()?),
            (Some(_), Some(_)) => unreachable!("clap rejects conflicting account scope flags"),
        }
    }
}

/// An account scope that has not been collapsed to an id, so callers can pick a by-email or
/// by-id resource endpoint. See [`AccountCommandHandler::select_account_scope_or_err`].
pub enum AccountScope {
    Email(String),
    Id(AccountId),
}
