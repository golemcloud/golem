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

use crate::command::cloud::project::plugin::ProjectPluginSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::log::{log_action, log_warn_action, logln};
use crate::model::ProjectReference;
use golem_client::api::ProjectClient;
use golem_client::model::{PluginInstallationCreation, PluginInstallationUpdate};
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
                project,
                plugin_name,
                plugin_version,
                priority,
                param,
            } => {
                self.cmd_install(
                    project.project,
                    plugin_name,
                    plugin_version,
                    priority,
                    param,
                )
                .await
            }
            ProjectPluginSubcommand::Get { project } => self.cmd_get(project.project).await,
            ProjectPluginSubcommand::Update {
                project,
                plugin_installation_id,
                priority,
                param,
            } => {
                self.cmd_update(project.project, plugin_installation_id, priority, param)
                    .await
            }
            ProjectPluginSubcommand::Uninstall {
                project,
                plugin_installation_id,
            } => {
                self.cmd_uninstall(project.project, plugin_installation_id)
                    .await
            }
        }
    }

    async fn cmd_install(
        &self,
        project_reference: ProjectReference,
        plugin_name: String,
        plugin_version: String,
        priority: i32,
        parameters: Vec<(String, String)>,
    ) -> anyhow::Result<()> {
        let project = self
            .ctx
            .cloud_project_handler()
            .select_project(&project_reference)
            .await?;

        log_action(
            "Installing",
            format!("plugin {plugin_name} for project {project_reference}"),
        );
        logln("");

        let result = self
            .ctx
            .golem_clients()
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

    async fn cmd_get(&self, project: ProjectReference) -> anyhow::Result<()> {
        let project = self
            .ctx
            .cloud_project_handler()
            .select_project(&project)
            .await?;

        let results = self
            .ctx
            .golem_clients()
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
        project_reference: ProjectReference,
        plugin_installation_id: PluginInstallationId,
        priority: i32,
        parameters: Vec<(String, String)>,
    ) -> anyhow::Result<()> {
        let project = self
            .ctx
            .cloud_project_handler()
            .select_project(&project_reference)
            .await?;

        log_action(
            "Updating",
            format!("plugin {plugin_installation_id} for project {project_reference}"),
        );

        self.ctx
            .golem_clients()
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
        project_reference: ProjectReference,
        plugin_installation_id: PluginInstallationId,
    ) -> anyhow::Result<()> {
        let project = self
            .ctx
            .cloud_project_handler()
            .select_project(&project_reference)
            .await?;

        log_warn_action(
            "Uninstalling",
            format!("plugin {plugin_installation_id} from project {project_reference}"),
        );

        self.ctx
            .golem_clients()
            .await?
            .project
            .uninstall_plugin_from_project(&project.project_id.0, &plugin_installation_id.0)
            .await
            .map_service_error()?;

        log_action("Uninstalled", "plugin");

        Ok(())
    }
}
