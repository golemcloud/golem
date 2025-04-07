// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::command::cloud::project::plugin::ProjectPluginSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::log::{log_action, log_warn_action, logln};
use crate::model::ProjectName;
use golem_client::model::{PluginInstallationCreation, PluginInstallationUpdate};
use golem_cloud_client::api::ProjectClient;
use golem_common::base_model::PluginInstallationId;
use std::sync::Arc;

pub struct CloudProjectPluginCommandHandler {
    ctx: Arc<Context>,
}

impl CloudProjectPluginCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: ProjectPluginSubcommand) -> anyhow::Result<()> {
        match subcommand {
            ProjectPluginSubcommand::Install {
                project_name,
                plugin_name,
                plugin_version,
                priority,
                param,
            } => {
                self.cmd_install(project_name, plugin_name, plugin_version, priority, param)
                    .await
            }
            ProjectPluginSubcommand::Get { project_name } => self.cmd_get(project_name).await,
            ProjectPluginSubcommand::Update {
                project_name,
                plugin_installation_id,
                priority,
                param,
            } => {
                self.cmd_update(project_name, plugin_installation_id, priority, param)
                    .await
            }
            ProjectPluginSubcommand::Uninstall {
                project_name,
                plugin_installation_id,
            } => {
                self.cmd_uninstall(project_name, plugin_installation_id)
                    .await
            }
        }
    }

    async fn cmd_install(
        &self,
        project_name: ProjectName,
        plugin_name: String,
        plugin_version: String,
        priority: i32,
        parameters: Vec<(String, String)>,
    ) -> anyhow::Result<()> {
        // TODO: account id
        let project = self
            .ctx
            .cloud_project_handler()
            .select_project(None, &project_name)
            .await?;

        log_action(
            "Installing",
            format!("plugin {} for project {}", plugin_name, project_name.0),
        );
        logln("");

        let result = self
            .ctx
            .golem_clients_cloud()
            .await?
            .project
            .install_plugin_to_project(
                &project.project_id.0,
                &PluginInstallationCreation {
                    name: plugin_name,
                    version: plugin_version,
                    priority,
                    parameters: parameters.into_iter().collect(),
                },
            )
            .await
            .map_service_error()?;

        self.ctx.log_handler().log_view(&result);

        Ok(())
    }

    async fn cmd_get(&self, project_name: ProjectName) -> anyhow::Result<()> {
        // TODO: account id
        let project = self
            .ctx
            .cloud_project_handler()
            .select_project(None, &project_name)
            .await?;

        let results = self
            .ctx
            .golem_clients_cloud()
            .await?
            .project
            .get_installed_plugins_of_project(&project.project_id.0)
            .await
            .map_service_error()?;

        self.ctx.log_handler().log_view(&results);

        Ok(())
    }

    async fn cmd_update(
        &self,
        project_name: ProjectName,
        plugin_installation_id: PluginInstallationId,
        priority: i32,
        parameters: Vec<(String, String)>,
    ) -> anyhow::Result<()> {
        // TODO: account id
        let project = self
            .ctx
            .cloud_project_handler()
            .select_project(None, &project_name)
            .await?;

        log_action(
            "Updating",
            format!(
                "plugin {} for project {}",
                plugin_installation_id, project_name.0
            ),
        );

        self.ctx
            .golem_clients_cloud()
            .await?
            .project
            .update_installed_plugin_in_project(
                &project.project_id.0,
                &plugin_installation_id.0,
                &PluginInstallationUpdate {
                    priority,
                    parameters: parameters.into_iter().collect(),
                },
            )
            .await
            .map_service_error()?;

        log_action("Updated", "plugin");

        Ok(())
    }

    async fn cmd_uninstall(
        &self,
        project_name: ProjectName,
        plugin_installation_id: PluginInstallationId,
    ) -> anyhow::Result<()> {
        // TODO: account id
        let project = self
            .ctx
            .cloud_project_handler()
            .select_project(None, &project_name)
            .await?;

        log_warn_action(
            "Uninstalling",
            format!(
                "plugin {} from project {}",
                plugin_installation_id, project_name.0
            ),
        );

        self.ctx
            .golem_clients_cloud()
            .await?
            .project
            .uninstall_plugin_from_project(&project.project_id.0, &plugin_installation_id.0)
            .await
            .map_service_error()?;

        log_action("Uninstalled", "plugin");

        Ok(())
    }
}
