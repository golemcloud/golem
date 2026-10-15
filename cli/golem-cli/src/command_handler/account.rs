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
use crate::command::shared_args::AccountScopeOptionalArgs;
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

#[derive(Debug, PartialEq)]
enum SelectedLimit<T> {
    Storage(T),
    Memory(T),
}

fn select_limit<T>(storage: Option<T>, memory: Option<T>) -> anyhow::Result<SelectedLimit<T>> {
    match (storage, memory) {
        (Some(value), None) => Ok(SelectedLimit::Storage(value)),
        (None, Some(value)) => Ok(SelectedLimit::Memory(value)),
        (None, None) => bail!("at least one limit must be provided"),
        (Some(_), Some(_)) => bail!("only one limit can be changed per command"),
    }
}

trait LimitsCommandActions {
    async fn show_limits(&self, account: AccountScopeOptionalArgs) -> anyhow::Result<()>;

    async fn set_limits(
        &self,
        account: AccountScopeOptionalArgs,
        storage: Option<u64>,
        memory: Option<u64>,
    ) -> anyhow::Result<()>;

    async fn unset_limits(
        &self,
        account: AccountScopeOptionalArgs,
        storage: bool,
        memory: bool,
    ) -> anyhow::Result<()>;
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
            AccountSubcommand::Usage { subcommand } => self.handle_usage_command(subcommand).await,
            AccountSubcommand::Limits { subcommand } => {
                Self::handle_limits_command(self, subcommand).await
            }
            AccountSubcommand::PermissionShare { subcommand } => {
                self.handle_permission_share_command(subcommand).await
            }
        }
    }

    async fn handle_usage_command(&self, subcommand: AccountUsageSubcommand) -> anyhow::Result<()> {
        match subcommand {
            AccountUsageSubcommand::Show { account, period } => {
                self.cmd_usage_show(account, period).await
            }
            AccountUsageSubcommand::History { account, last } => {
                self.cmd_usage_history(account, last).await
            }
        }
    }

    async fn handle_limits_command(
        actions: &impl LimitsCommandActions,
        subcommand: AccountLimitsSubcommand,
    ) -> anyhow::Result<()> {
        match subcommand {
            AccountLimitsSubcommand::Show { account } => actions.show_limits(account).await,
            AccountLimitsSubcommand::Set {
                account,
                max_storage_per_agent,
                max_memory_per_agent,
            } => {
                actions
                    .set_limits(account, max_storage_per_agent, max_memory_per_agent)
                    .await
            }
            AccountLimitsSubcommand::Unset {
                account,
                max_storage_per_agent,
                max_memory_per_agent,
            } => {
                actions
                    .unset_limits(account, max_storage_per_agent, max_memory_per_agent)
                    .await
            }
        }
    }

    async fn handle_permission_share_command(
        &self,
        subcommand: PermissionShareSubcommand,
    ) -> anyhow::Result<()> {
        match subcommand {
            PermissionShareSubcommand::List { account, received } => {
                self.cmd_permission_share_list(account, received).await
            }
            PermissionShareSubcommand::Get {
                permission_share_id,
            } => self.cmd_permission_share_get(permission_share_id).await,
            PermissionShareSubcommand::GetByName { account, name } => {
                self.cmd_permission_share_get_by_name(account, name).await
            }
            PermissionShareSubcommand::New {
                account,
                target_account_email,
                name,
                grants,
            } => {
                self.cmd_permission_share_new(account, target_account_email, name, grants)
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

    async fn cmd_usage_show(
        &self,
        account: AccountScopeOptionalArgs,
        period: Option<AccountUsagePeriod>,
    ) -> anyhow::Result<()> {
        let account_id = self.select_account_id_or_err(account).await?;
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
        account: AccountScopeOptionalArgs,
        last: usize,
    ) -> anyhow::Result<()> {
        let account_id = self.select_account_id_or_err(account).await?;
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

    async fn cmd_limits_show(&self, account: AccountScopeOptionalArgs) -> anyhow::Result<()> {
        let account_id = self.select_account_id_or_err(account).await?;
        let clients = self.ctx.golem_clients().await?;
        let policy = clients
            .account
            .get_account_limits(&account_id.0)
            .await
            .map_service_error()?;
        self.ctx
            .log_handler()
            .log_output(AccountLimitsView::new(policy))?;
        Ok(())
    }

    async fn cmd_limits_set(
        &self,
        account: AccountScopeOptionalArgs,
        storage: Option<u64>,
        max_memory: Option<u64>,
    ) -> anyhow::Result<()> {
        let account_id = self.select_account_id_or_err(account).await?;
        let clients = self.ctx.golem_clients().await?;
        match select_limit(storage, max_memory)? {
            SelectedLimit::Storage(value) => {
                clients
                    .account
                    .set_account_storage_override(&account_id.0, &SetStorageLimit { value })
                    .await
                    .map_service_error()?;
            }
            SelectedLimit::Memory(value) => {
                clients
                    .account
                    .set_account_max_memory_override(&account_id.0, &SetMemoryLimit { value })
                    .await
                    .map_service_error()?;
            }
        }
        self.cmd_limits_show(AccountScopeOptionalArgs {
            account: None,
            account_id: Some(account_id),
        })
        .await
    }

    async fn cmd_limits_unset(
        &self,
        account: AccountScopeOptionalArgs,
        storage: bool,
        max_memory: bool,
    ) -> anyhow::Result<()> {
        let account_id = self.select_account_id_or_err(account).await?;
        let clients = self.ctx.golem_clients().await?;
        match select_limit(storage.then_some(()), max_memory.then_some(()))? {
            SelectedLimit::Storage(()) => {
                clients
                    .account
                    .clear_account_storage_override(&account_id.0)
                    .await
                    .map_service_error()?;
            }
            SelectedLimit::Memory(()) => {
                clients
                    .account
                    .clear_account_max_memory_override(&account_id.0)
                    .await
                    .map_service_error()?;
            }
        }
        self.cmd_limits_show(AccountScopeOptionalArgs {
            account: None,
            account_id: Some(account_id),
        })
        .await
    }

    async fn cmd_permission_share_list(
        &self,
        account: AccountScopeOptionalArgs,
        received: bool,
    ) -> anyhow::Result<()> {
        let account_id = self.select_account_id_or_err(account).await?;
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
        account: AccountScopeOptionalArgs,
        name: String,
    ) -> anyhow::Result<()> {
        let account_id = self.select_account_id_or_err(account).await?;
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
        account: AccountScopeOptionalArgs,
        target_account_email: String,
        name: String,
        grants: PermissionShareGrantArgs,
    ) -> anyhow::Result<()> {
        let account_id = self.select_account_id_or_err(account).await?;
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

    async fn get(&self, account: AccountScopeOptionalArgs) -> anyhow::Result<Account> {
        self.select_account_or_err(account).await
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
/// by-id resource endpoint. See [`AccountHandler::select_account_scope_or_err`].
pub enum AccountScope {
    Email(String),
    Id(AccountId),
}

impl LimitsCommandActions for AccountCommandHandler {
    async fn show_limits(&self, account: AccountScopeOptionalArgs) -> anyhow::Result<()> {
        self.cmd_limits_show(account).await
    }

    async fn set_limits(
        &self,
        account: AccountScopeOptionalArgs,
        storage: Option<u64>,
        memory: Option<u64>,
    ) -> anyhow::Result<()> {
        self.cmd_limits_set(account, storage, memory).await
    }

    async fn unset_limits(
        &self,
        account: AccountScopeOptionalArgs,
        storage: bool,
        memory: bool,
    ) -> anyhow::Result<()> {
        self.cmd_limits_unset(account, storage, memory).await
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use test_r::test;

    #[derive(Debug, PartialEq)]
    enum RecordedLimitsAction {
        Show(Option<String>, Option<AccountId>),
        Set(Option<String>, Option<AccountId>, Option<u64>, Option<u64>),
        Unset(Option<String>, Option<AccountId>, bool, bool),
    }

    #[derive(Default)]
    struct RecordingLimitsActions {
        actions: Mutex<Vec<RecordedLimitsAction>>,
    }

    impl LimitsCommandActions for RecordingLimitsActions {
        async fn show_limits(&self, account: AccountScopeOptionalArgs) -> anyhow::Result<()> {
            self.actions
                .lock()
                .unwrap()
                .push(RecordedLimitsAction::Show(
                    account.account,
                    account.account_id,
                ));
            Ok(())
        }

        async fn set_limits(
            &self,
            account: AccountScopeOptionalArgs,
            storage: Option<u64>,
            memory: Option<u64>,
        ) -> anyhow::Result<()> {
            self.actions.lock().unwrap().push(RecordedLimitsAction::Set(
                account.account,
                account.account_id,
                storage,
                memory,
            ));
            Ok(())
        }

        async fn unset_limits(
            &self,
            account: AccountScopeOptionalArgs,
            storage: bool,
            memory: bool,
        ) -> anyhow::Result<()> {
            self.actions
                .lock()
                .unwrap()
                .push(RecordedLimitsAction::Unset(
                    account.account,
                    account.account_id,
                    storage,
                    memory,
                ));
            Ok(())
        }
    }

    #[test]
    fn limit_selection_requires_exactly_one_dimension() {
        assert_eq!(
            select_limit(Some(1), None).unwrap(),
            SelectedLimit::Storage(1)
        );
        assert_eq!(
            select_limit(None, Some(2)).unwrap(),
            SelectedLimit::Memory(2)
        );
        assert_eq!(
            select_limit::<u64>(None, None).unwrap_err().to_string(),
            "at least one limit must be provided"
        );
        assert_eq!(
            select_limit(Some(1), Some(2)).unwrap_err().to_string(),
            "only one limit can be changed per command"
        );
    }

    #[test]
    async fn limits_dispatch_invokes_the_selected_action() {
        let actions = RecordingLimitsActions::default();
        let account_id = AccountId::new();

        AccountCommandHandler::handle_limits_command(
            &actions,
            AccountLimitsSubcommand::Show {
                account: AccountScopeOptionalArgs {
                    account: None,
                    account_id: Some(account_id),
                },
            },
        )
        .await
        .unwrap();
        AccountCommandHandler::handle_limits_command(
            &actions,
            AccountLimitsSubcommand::Set {
                account: AccountScopeOptionalArgs {
                    account: Some("owner@example.com".to_string()),
                    account_id: None,
                },
                max_storage_per_agent: Some(100),
                max_memory_per_agent: None,
            },
        )
        .await
        .unwrap();
        AccountCommandHandler::handle_limits_command(
            &actions,
            AccountLimitsSubcommand::Unset {
                account: AccountScopeOptionalArgs {
                    account: None,
                    account_id: None,
                },
                max_storage_per_agent: false,
                max_memory_per_agent: true,
            },
        )
        .await
        .unwrap();

        assert_eq!(
            *actions.actions.lock().unwrap(),
            vec![
                RecordedLimitsAction::Show(None, Some(account_id)),
                RecordedLimitsAction::Set(
                    Some("owner@example.com".to_string()),
                    None,
                    Some(100),
                    None,
                ),
                RecordedLimitsAction::Unset(None, None, false, true),
            ]
        );
    }
}
