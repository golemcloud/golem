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

pub mod grant;

use crate::command::cloud::account::AccountSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::error::NonSuccessfulExit;
use crate::log::log_warn_action;
use crate::model::text::account::{AccountGetView, AccountNewView};
use crate::model::text::fmt::log_error;
use crate::model::AccountId;
use anyhow::bail;
use golem_client::api::AccountClient;
use golem_client::model::{Account, AccountData};
use std::sync::Arc;

pub struct CloudAccountCommandHandler {
    ctx: Arc<Context>,
}

impl CloudAccountCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: AccountSubcommand) -> anyhow::Result<()> {
        match subcommand {
            AccountSubcommand::Get { account_id } => self.cmd_get(account_id.account_id).await,
            AccountSubcommand::Update {
                account_id,
                account_name,
                account_email,
            } => {
                self.cmd_update(account_id.account_id, account_name, account_email)
                    .await
            }
            AccountSubcommand::New {
                account_name,
                account_email,
            } => self.cmd_new(account_name, account_email).await,
            AccountSubcommand::Delete { account_id } => {
                self.cmd_delete(account_id.account_id).await
            }
            AccountSubcommand::Grant { subcommand } => {
                self.ctx
                    .cloud_account_grant_handler()
                    .handle_command(subcommand)
                    .await
            }
        }
    }

    async fn cmd_get(&self, account_id: Option<AccountId>) -> anyhow::Result<()> {
        let account = self.get(account_id).await?;
        self.ctx.log_handler().log_view(&AccountGetView(account));

        Ok(())
    }

    async fn cmd_update(
        &self,
        account_id: Option<AccountId>,
        account_name: Option<String>,
        account_email: Option<String>,
    ) -> anyhow::Result<()> {
        if account_name.is_none() && account_email.is_none() {
            log_error("account name or email must be provided");
            bail!(NonSuccessfulExit)
        }

        // TODO: this should have a proper update endpoint instead of getting then updating...
        let account = self.get(account_id).await?;
        let account = self
            .ctx
            .golem_clients()
            .await?
            .account
            .update_account(
                &account.id,
                &AccountData {
                    name: account_name.unwrap_or(account.name),
                    email: account_email.unwrap_or(account.email),
                },
            )
            .await
            .map_service_error()?;

        self.ctx.log_handler().log_view(&AccountGetView(account));

        Ok(())
    }

    async fn cmd_new(&self, account_name: String, account_email: String) -> anyhow::Result<()> {
        let account = self
            .ctx
            .golem_clients()
            .await?
            .account
            .create_account(&AccountData {
                name: account_name,
                email: account_email,
            })
            .await
            .map_service_error()?;

        self.ctx.log_handler().log_view(&AccountNewView(account));

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
            .delete_account(&account.id)
            .await
            .map_service_error()?;

        log_warn_action("Deleted", "account");

        Ok(())
    }

    async fn get(&self, account_id: Option<AccountId>) -> anyhow::Result<Account> {
        self.ctx
            .golem_clients()
            .await?
            .account
            .get_account(&self.select_account_id_or_err(account_id).await?.0)
            .await
            .map_service_error()
    }

    pub async fn account_id_or_err(&self) -> anyhow::Result<AccountId> {
        Ok(self.ctx.golem_clients().await?.account_id())
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
