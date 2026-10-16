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

use crate::command::account::AccountUsageSubcommand;
use crate::command::shared_args::AccountScopeOptionalArgs;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::MapServiceError;
use crate::model::account::{AccountUsageListView, AccountUsageView};
use golem_client::api::AccountClient;
use golem_common::model::account_usage::AccountUsagePeriod;
use std::sync::Arc;

pub struct AccountUsageCommandHandler {
    ctx: Arc<Context>,
}

impl AccountUsageCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: AccountUsageSubcommand) -> anyhow::Result<()> {
        match subcommand {
            AccountUsageSubcommand::Show { account, period } => {
                self.cmd_show(account, period).await
            }
            AccountUsageSubcommand::History { account, last } => {
                self.cmd_history(account, last).await
            }
        }
    }

    async fn cmd_show(
        &self,
        account: AccountScopeOptionalArgs,
        period: Option<AccountUsagePeriod>,
    ) -> anyhow::Result<()> {
        let account_id = self
            .ctx
            .account_handler()
            .select_account_id_or_err(account)
            .await?;
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

    async fn cmd_history(
        &self,
        account: AccountScopeOptionalArgs,
        last: usize,
    ) -> anyhow::Result<()> {
        let account_id = self
            .ctx
            .account_handler()
            .select_account_id_or_err(account)
            .await?;
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
}
