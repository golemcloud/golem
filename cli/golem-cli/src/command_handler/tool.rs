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

use crate::command::tool::{
    ToolGrantCreateArgs, ToolGrantSubcommand, ToolReleaseSubcommand, ToolSubcommand,
};
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::MapServiceError;
use crate::model::environment::{
    EnvironmentResolveMode, EnvironmentToolGrantCreateView, EnvironmentToolGrantDeleteView,
    EnvironmentToolGrantGetView, EnvironmentToolGrantListView, EnvironmentToolGrantRestoreView,
    EnvironmentToolGrantView,
};
use crate::model::tool_deployment::{DeployedToolListView, DeployedToolView};
use crate::model::tool_release::{ToolReleaseListView, ToolReleaseView};
use golem_client::api::{EnvironmentClient, EnvironmentToolGrantsClient, ToolReleasesClient};
use golem_common::base_model::account::AccountEmail;
use golem_common::base_model::environment_tool_grant::EnvironmentToolGrantCreation;
use golem_common::base_model::tool_release::{
    ToolReleaseByCoordinates, ToolReleaseById, ToolReleaseReference,
};
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
            ToolSubcommand::List => self.cmd_list().await,
            ToolSubcommand::Get { tool_name } => self.cmd_get(tool_name).await,
            ToolSubcommand::Release { subcommand } => self.handle_release(subcommand).await,
            ToolSubcommand::Grant { subcommand } => self.handle_grant(subcommand).await,
        }
    }

    async fn cmd_list(&self) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;
        let tools = environment
            .with_current_deployment_revision_or_default_warn(|revision| async move {
                Ok(self
                    .ctx
                    .golem_clients()
                    .await?
                    .environment
                    .list_deployment_registered_tools(
                        &environment.environment_id.0,
                        revision.into(),
                    )
                    .await
                    .map_service_error()?
                    .values)
            })
            .await?;
        self.ctx
            .log_handler()
            .log_output(DeployedToolListView { tools })?;
        Ok(())
    }

    async fn cmd_get(
        &self,
        tool_name: golem_common::base_model::tool::ToolName,
    ) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;
        let revision = environment.current_deployment_or_err()?.deployment_revision;
        let tool = self
            .ctx
            .golem_clients()
            .await?
            .environment
            .get_deployment_registered_tool(
                &environment.environment_id.0,
                revision.into(),
                tool_name.as_str(),
            )
            .await
            .map_service_error()?;
        self.ctx
            .log_handler()
            .log_output(DeployedToolView { tool })?;
        Ok(())
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

    async fn handle_grant(&self, subcommand: ToolGrantSubcommand) -> anyhow::Result<()> {
        match subcommand {
            ToolGrantSubcommand::Create(args) => self.cmd_grant_create(args).await,
            ToolGrantSubcommand::List => {
                let environment = self
                    .ctx
                    .environment_handler()
                    .resolve_environment(EnvironmentResolveMode::Any)
                    .await?;
                let grants = self
                    .ctx
                    .golem_clients()
                    .await?
                    .environment_tool_grants
                    .list_environment_tool_grants(&environment.environment_id.0)
                    .await
                    .map_service_error()?
                    .values
                    .into_iter()
                    .map(Into::into)
                    .collect();
                self.ctx
                    .log_handler()
                    .log_output(EnvironmentToolGrantListView { grants })?;
                Ok(())
            }
            ToolGrantSubcommand::Get { grant_id } => {
                let grant = self
                    .ctx
                    .golem_clients()
                    .await?
                    .environment_tool_grants
                    .get_environment_tool_grant(&grant_id.0)
                    .await
                    .map_service_error()?;
                self.ctx
                    .log_handler()
                    .log_output(EnvironmentToolGrantGetView {
                        grant: grant.into(),
                    })?;
                Ok(())
            }
            ToolGrantSubcommand::Delete { grant_id } => {
                self.ctx
                    .golem_clients()
                    .await?
                    .environment_tool_grants
                    .delete_environment_tool_grant(&grant_id.0)
                    .await
                    .map_service_error()?;
                self.ctx
                    .log_handler()
                    .log_output(EnvironmentToolGrantDeleteView { grant_id })?;
                Ok(())
            }
            ToolGrantSubcommand::Restore { grant_id } => {
                let grant = self
                    .ctx
                    .golem_clients()
                    .await?
                    .environment_tool_grants
                    .restore_environment_tool_grant(&grant_id.0)
                    .await
                    .map_service_error()?;
                self.ctx
                    .log_handler()
                    .log_output(EnvironmentToolGrantRestoreView {
                        grant: grant.into(),
                    })?;
                Ok(())
            }
        }
    }

    async fn cmd_grant_create(&self, args: ToolGrantCreateArgs) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;
        let release = match (args.release_id, args.account, args.name, args.version) {
            (Some(release_id), None, None, None) => {
                ToolReleaseReference::ById(ToolReleaseById { release_id })
            }
            (None, Some(account), Some(name), Some(version)) => {
                ToolReleaseReference::ByCoordinates(ToolReleaseByCoordinates {
                    account: AccountEmail::new(account),
                    name,
                    version,
                })
            }
            _ => unreachable!("clap validates the tool release reference"),
        };
        let grant = self
            .ctx
            .golem_clients()
            .await?
            .environment_tool_grants
            .create_environment_tool_grant(
                &environment.environment_id.0,
                &EnvironmentToolGrantCreation { release },
            )
            .await
            .map_service_error()?;
        self.ctx
            .log_handler()
            .log_output(EnvironmentToolGrantCreateView {
                grant: EnvironmentToolGrantView::from(grant),
            })?;
        Ok(())
    }
}
