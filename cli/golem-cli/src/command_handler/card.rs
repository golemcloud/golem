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
use crate::context::Context;
use crate::error::NonSuccessfulExit;
use crate::error::service::MapServiceError;
use crate::log::log_warn_action;
use crate::model::text::card::{CardGetView, CardListView, CardRevokeResult};
use crate::model::worker::RawAgentId;
use anyhow::bail;
use golem_client::api::CardClient;
use golem_common::model::account::AccountId;
use golem_common::model::card::CardId;
use std::sync::Arc;

pub struct CardCommandHandler {
    ctx: Arc<Context>,
}

impl CardCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: CardSubcommand) -> anyhow::Result<()> {
        match subcommand {
            CardSubcommand::List { account_id, agent } => {
                self.cmd_list(account_id.account_id, agent).await
            }
            CardSubcommand::Get { card_id } => self.cmd_get(card_id).await,
            CardSubcommand::Revoke { card_id } => self.cmd_revoke(card_id).await,
        }
    }

    async fn cmd_list(
        &self,
        account_id: Option<AccountId>,
        agent: Option<RawAgentId>,
    ) -> anyhow::Result<()> {
        if let Some(agent) = agent {
            bail!(
                "Listing the wallet for agent {agent} requires the get-agent-wallet worker endpoint"
            );
        }

        let account_id = account_id.unwrap_or(*self.ctx.golem_clients().await?.account_id());
        let cards = self
            .ctx
            .golem_clients()
            .await?
            .card
            .list_account_cards(&account_id.0)
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
