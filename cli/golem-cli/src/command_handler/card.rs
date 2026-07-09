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

use crate::command::card::CardSubcommand;
use crate::command_handler::Handlers;
use crate::command_handler::worker::WorkerCommandHandler;
use crate::context::Context;
use crate::error::NonSuccessfulExit;
use crate::error::service::MapServiceError;
use crate::log::log_warn_action;
use crate::model::text::card::{CardGetView, CardListView, CardRevokeResult};
use crate::model::worker::RawAgentId;
use anyhow::bail;
use golem_client::api::{CardClient, WorkerClient};
use golem_common::model::account::AccountId;
use golem_common::model::card::CardId;
use std::sync::Arc;

pub struct CardCommandHandler {
    ctx: Arc<Context>,
}

#[derive(Clone, Copy, Debug)]
struct CardListFilter {
    include_root: bool,
    include_permission_shares: bool,
    include_environment_defaults: bool,
    include_agent_initials: bool,
    explicit_includes: bool,
}

impl CardListFilter {
    fn from_flags(
        include_root: bool,
        include_permission_shares: bool,
        include_environment_defaults: bool,
        include_agent_initials: bool,
    ) -> Self {
        let explicit_includes = include_root
            || include_permission_shares
            || include_environment_defaults
            || include_agent_initials;
        if explicit_includes {
            Self {
                include_root,
                include_permission_shares,
                include_environment_defaults,
                include_agent_initials,
                explicit_includes,
            }
        } else {
            Self {
                include_root: true,
                include_permission_shares: true,
                include_environment_defaults: true,
                include_agent_initials: true,
                explicit_includes,
            }
        }
    }
}

impl CardCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: CardSubcommand) -> anyhow::Result<()> {
        match subcommand {
            CardSubcommand::List {
                account_id,
                agent,
                include_root,
                include_permission_shares,
                include_environment_defaults,
                include_agent_initials,
            } => {
                self.cmd_list(
                    account_id.account_id,
                    agent,
                    CardListFilter::from_flags(
                        include_root,
                        include_permission_shares,
                        include_environment_defaults,
                        include_agent_initials,
                    ),
                )
                .await
            }
            CardSubcommand::Get { card_id } => self.cmd_get(card_id).await,
            CardSubcommand::Revoke { card_id } => self.cmd_revoke(card_id).await,
        }
    }

    async fn cmd_list(
        &self,
        account_id: Option<AccountId>,
        agent: Option<RawAgentId>,
        filter: CardListFilter,
    ) -> anyhow::Result<()> {
        if let Some(agent) = agent {
            if filter.explicit_includes {
                bail!("card list include flags cannot be used together with --agent");
            }
            return self.cmd_list_agent_wallet(agent).await;
        }

        let account_id = account_id.unwrap_or(*self.ctx.golem_clients().await?.account_id());
        let cards = self
            .ctx
            .golem_clients()
            .await?
            .card
            .list_account_cards(
                &account_id.0,
                Some(filter.include_root),
                Some(filter.include_permission_shares),
                Some(filter.include_environment_defaults),
                Some(filter.include_agent_initials),
            )
            .await
            .map_service_error()?;

        self.ctx.log_handler().log_output(CardListView { cards })?;

        Ok(())
    }

    async fn cmd_list_agent_wallet(&self, agent: RawAgentId) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;

        let worker_handler = WorkerCommandHandler::new(self.ctx.clone());
        let agent_name_match = worker_handler.match_agent_name(agent).await?;
        let (component, agent_name) = worker_handler
            .component_by_agent_name_match(&agent_name_match)
            .await?;

        let cards = self
            .ctx
            .golem_clients()
            .await?
            .worker
            .get_agent_wallet(&component.id.0, &agent_name.0)
            .await
            .map_service_error()?;

        self.ctx.log_handler().log_output(CardListView { cards })?;

        Ok(())
    }

    async fn cmd_get(&self, card_id: CardId) -> anyhow::Result<()> {
        let card = self
            .ctx
            .golem_clients()
            .await?
            .card
            .get_card(&card_id.0)
            .await
            .map_service_error()?;

        self.ctx.log_handler().log_output(CardGetView(card))?;

        Ok(())
    }

    async fn cmd_revoke(&self, card_id: CardId) -> anyhow::Result<()> {
        if !self
            .ctx
            .interactive_handler()
            .confirm_revoke_card(card_id)?
        {
            bail!(NonSuccessfulExit)
        }

        let response = self
            .ctx
            .golem_clients()
            .await?
            .card
            .revoke_card(&card_id.0)
            .await
            .map_service_error()?;

        log_warn_action("Revoked", "card");

        self.ctx.log_handler().log_output(CardRevokeResult {
            revoked_card_ids: response.revoked_card_ids,
        })?;

        Ok(())
    }
}
