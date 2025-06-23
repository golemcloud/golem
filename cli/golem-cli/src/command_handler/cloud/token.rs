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

use crate::command::cloud::token::TokenSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::log::{log_warn_action, LogColorize};
use crate::model::text::token::{TokenListView, TokenNewView};
use crate::model::TokenId;
use chrono::{DateTime, Utc};
use golem_client::api::TokenClient;
use golem_client::model::CreateTokenDto;
use std::sync::Arc;

pub struct CloudTokenCommandHandler {
    ctx: Arc<Context>,
}

impl CloudTokenCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: TokenSubcommand) -> anyhow::Result<()> {
        match subcommand {
            TokenSubcommand::List => self.cmd_list().await,
            TokenSubcommand::New { expires_at } => self.cmd_new(expires_at).await,
            TokenSubcommand::Delete { token_id } => self.cmd_delete(token_id).await,
        }
    }

    async fn cmd_list(&self) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        let tokens = clients
            .token
            .get_tokens(&clients.account_id().0)
            .await
            .map_service_error()?;

        self.ctx.log_handler().log_view(&TokenListView(tokens));

        Ok(())
    }

    async fn cmd_new(&self, expires_at: DateTime<Utc>) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        let token = clients
            .token
            .create_token(&clients.account_id().0, &CreateTokenDto { expires_at })
            .await
            .map_service_error()?;

        self.ctx.log_handler().log_view(&TokenNewView(token));

        Ok(())
    }

    async fn cmd_delete(&self, token_id: TokenId) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .token
            .delete_token(&clients.account_id().0, &token_id.0)
            .await
            .map_service_error()?;

        log_warn_action(
            "Deleted",
            format!("token {}", token_id.0.to_string().log_color_highlight()),
        );

        Ok(())
    }
}
