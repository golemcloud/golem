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

use crate::command::account::AccountLimitsSubcommand;
use crate::command::shared_args::AccountScopeOptionalArgs;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::MapServiceError;
use crate::model::account::AccountLimitsView;
use anyhow::bail;
use golem_client::api::AccountClient;
use golem_common::model::account_usage::{SetMemoryLimit, SetStorageLimit};
use std::sync::Arc;

pub struct AccountLimitsCommandHandler {
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
    async fn cmd_show(&self, account: AccountScopeOptionalArgs) -> anyhow::Result<()>;

    async fn cmd_set(
        &self,
        account: AccountScopeOptionalArgs,
        storage: Option<u64>,
        memory: Option<u64>,
    ) -> anyhow::Result<()>;

    async fn cmd_unset(
        &self,
        account: AccountScopeOptionalArgs,
        storage: bool,
        memory: bool,
    ) -> anyhow::Result<()>;
}

impl AccountLimitsCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: AccountLimitsSubcommand) -> anyhow::Result<()> {
        Self::dispatch_command(self, subcommand).await
    }

    async fn dispatch_command(
        actions: &impl LimitsCommandActions,
        subcommand: AccountLimitsSubcommand,
    ) -> anyhow::Result<()> {
        match subcommand {
            AccountLimitsSubcommand::Show { account } => actions.cmd_show(account).await,
            AccountLimitsSubcommand::Set {
                account,
                max_storage_per_agent,
                max_memory_per_agent,
            } => {
                actions
                    .cmd_set(account, max_storage_per_agent, max_memory_per_agent)
                    .await
            }
            AccountLimitsSubcommand::Unset {
                account,
                max_storage_per_agent,
                max_memory_per_agent,
            } => {
                actions
                    .cmd_unset(account, max_storage_per_agent, max_memory_per_agent)
                    .await
            }
        }
    }
}

impl LimitsCommandActions for AccountLimitsCommandHandler {
    async fn cmd_show(&self, account: AccountScopeOptionalArgs) -> anyhow::Result<()> {
        let account_id = self
            .ctx
            .account_handler()
            .select_account_id_or_err(account)
            .await?;
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

    async fn cmd_set(
        &self,
        account: AccountScopeOptionalArgs,
        storage: Option<u64>,
        max_memory: Option<u64>,
    ) -> anyhow::Result<()> {
        let account_id = self
            .ctx
            .account_handler()
            .select_account_id_or_err(account)
            .await?;
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
        self.cmd_show(AccountScopeOptionalArgs {
            account: None,
            account_id: Some(account_id),
        })
        .await
    }

    async fn cmd_unset(
        &self,
        account: AccountScopeOptionalArgs,
        storage: bool,
        max_memory: bool,
    ) -> anyhow::Result<()> {
        let account_id = self
            .ctx
            .account_handler()
            .select_account_id_or_err(account)
            .await?;
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
        self.cmd_show(AccountScopeOptionalArgs {
            account: None,
            account_id: Some(account_id),
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::account::AccountId;
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
        async fn cmd_show(&self, account: AccountScopeOptionalArgs) -> anyhow::Result<()> {
            self.actions
                .lock()
                .unwrap()
                .push(RecordedLimitsAction::Show(
                    account.account,
                    account.account_id,
                ));
            Ok(())
        }

        async fn cmd_set(
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

        async fn cmd_unset(
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

        AccountLimitsCommandHandler::dispatch_command(
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
        AccountLimitsCommandHandler::dispatch_command(
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
        AccountLimitsCommandHandler::dispatch_command(
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
