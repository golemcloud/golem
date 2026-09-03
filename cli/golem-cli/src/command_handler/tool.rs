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

use crate::command::tool::{ToolReleaseSubcommand, ToolSubcommand};
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::MapServiceError;
use crate::model::tool_release::{ToolReleaseListView, ToolReleaseView};
use golem_client::api::ToolReleasesClient;
use std::sync::Arc;

pub struct ToolCommandHandler {
    ctx: Arc<Context>,
}

impl ToolCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: ToolSubcommand) -> anyhow::Result<()> {
        match subcommand {
            ToolSubcommand::Release { subcommand } => self.handle_release(subcommand).await,
        }
    }

    async fn handle_release(&self, subcommand: ToolReleaseSubcommand) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;
        match subcommand {
            ToolReleaseSubcommand::List { account_id } => {
                let account_id = account_id.unwrap_or(*clients.account_id());
                let releases = clients
                    .tool_releases
                    .list_account_tool_releases(&account_id.0)
                    .await
                    .map_service_error()?
                    .values;
                self.ctx
                    .log_handler()
                    .log_output(ToolReleaseListView { releases })?;
            }
            ToolReleaseSubcommand::Get { release_id } => {
                let release = clients
                    .tool_releases
                    .get_tool_release(&release_id.0)
                    .await
                    .map_service_error()?;
                self.ctx
                    .log_handler()
                    .log_output(ToolReleaseView { release })?;
            }
            ToolReleaseSubcommand::DePublish { release_id } => {
                let release = clients
                    .tool_releases
                    .de_publish_tool_release(&release_id.0)
                    .await
                    .map_service_error()?;
                self.ctx
                    .log_handler()
                    .log_output(ToolReleaseView { release })?;
            }
            ToolReleaseSubcommand::Restore { release_id } => {
                let release = clients
                    .tool_releases
                    .restore_tool_release(&release_id.0)
                    .await
                    .map_service_error()?;
                self.ctx
                    .log_handler()
                    .log_output(ToolReleaseView { release })?;
            }
        }
        Ok(())
    }
}
