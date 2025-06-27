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

use crate::command::component::plugin::ComponentPluginSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::error::NonSuccessfulExit;
use crate::log::{log_action, log_error_action, log_warn_action, LogIndent};
use crate::model::text::fmt::log_warn;
use crate::model::ComponentName;
use anyhow::bail;
use golem_client::api::ComponentClient;
use golem_client::model::{
    PluginInstallation, PluginInstallationCreation, PluginInstallationUpdate,
};
use golem_common::base_model::PluginInstallationId;
use std::sync::Arc;

pub struct ComponentPluginCommandHandler {
    ctx: Arc<Context>,
}

impl ComponentPluginCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(
        &self,
        subcommand: ComponentPluginSubcommand,
    ) -> anyhow::Result<()> {
        match subcommand {
            ComponentPluginSubcommand::Install {
                component_name,
                plugin_name,
                plugin_version,
                priority,
                param,
            } => {
                self.cmd_install(
                    component_name.component_name,
                    plugin_name,
                    plugin_version,
                    priority,
                    param,
                )
                .await
            }
            ComponentPluginSubcommand::Get {
                component_name,
                version,
            } => self.cmd_get(component_name.component_name, version).await,
            ComponentPluginSubcommand::Update {
                component_name,
                installation_id,
                priority,
                param,
            } => {
                self.cmd_update(
                    component_name.component_name,
                    installation_id,
                    priority,
                    param,
                )
                .await
            }
            ComponentPluginSubcommand::Uninstall {
                component_name,
                installation_id,
            } => {
                self.cmd_uninstall(component_name.component_name, installation_id)
                    .await
            }
        }
    }

    async fn cmd_install(
        &self,
        component_name: Option<ComponentName>,
        plugin_name: String,
        plugin_version: String,
        priority: i32,
        parameters: Vec<(String, String)>,
    ) -> anyhow::Result<()> {
        let selected_components = self
            .ctx
            .component_handler()
            .must_select_components_by_app_dir_or_name(component_name.as_ref())
            .await?;

        let mut installations = Vec::<PluginInstallation>::new();
        let mut any_error = false;
        for component_name in &selected_components.component_names {
            log_action(
                "Installing",
                format!("plugin {plugin_name} from component {component_name}"),
            );
            let _indent = LogIndent::new();

            let component = self
                .ctx
                .component_handler()
                .component(
                    selected_components.project.as_ref(),
                    component_name.into(),
                    None,
                )
                .await?;

            let result = match component {
                Some(component) => {
                    let clients = self.ctx.golem_clients().await?;

                    let result = clients
                        .component
                        .install_plugin(
                            &component.versioned_component_id.component_id,
                            &PluginInstallationCreation {
                                name: plugin_name.clone(),
                                version: plugin_version.clone(),
                                priority,
                                parameters: parameters.clone().into_iter().collect(),
                            },
                        )
                        .await
                        .map_service_error()?;

                    Some(result)
                }
                None => {
                    log_warn(format!("Component {component_name} not found"));
                    any_error = true;
                    None
                }
            };
            if let Some(result) = result {
                log_action("Installed", "plugin");
                installations.push(result);
            }
        }

        self.ctx.log_handler().log_view(&installations);

        if any_error {
            bail!(NonSuccessfulExit)
        }

        Ok(())
    }

    async fn cmd_get(
        &self,
        component_name: Option<ComponentName>,
        version: Option<u64>,
    ) -> anyhow::Result<()> {
        let selected_components = self
            .ctx
            .component_handler()
            .must_select_components_by_app_dir_or_name(component_name.as_ref())
            .await?;

        let mut installations = Vec::<PluginInstallation>::new();
        for component_name in &selected_components.component_names {
            let component = self
                .ctx
                .component_handler()
                .component(
                    selected_components.project.as_ref(),
                    component_name.into(),
                    None,
                )
                .await?;

            let result = match component {
                Some(component) => self
                    .ctx
                    .golem_clients()
                    .await?
                    .component
                    .get_installed_plugins(
                        &component.versioned_component_id.component_id,
                        &version
                            .unwrap_or(component.versioned_component_id.version)
                            .to_string(),
                    )
                    .await
                    .map_service_error()?,
                None => {
                    log_warn(format!("Component {component_name} not found"));
                    vec![]
                }
            };
            installations.extend(result);
        }

        self.ctx.log_handler().log_view(&installations);

        Ok(())
    }

    async fn cmd_update(
        &self,
        component_name: Option<ComponentName>,
        plugin_installation_id: PluginInstallationId,
        priority: i32,
        parameters: Vec<(String, String)>,
    ) -> anyhow::Result<()> {
        let selected_components = self
            .ctx
            .component_handler()
            .must_select_components_by_app_dir_or_name(component_name.as_ref())
            .await?;

        let mut any_error = false;
        for component_name in &selected_components.component_names {
            log_action(
                "Updating",
                format!("plugin {plugin_installation_id} for component {component_name}"),
            );
            let _indent = LogIndent::new();

            let component = self
                .ctx
                .component_handler()
                .component(
                    selected_components.project.as_ref(),
                    component_name.into(),
                    None,
                )
                .await?;

            match component {
                Some(component) => {
                    self.ctx
                        .golem_clients()
                        .await?
                        .component
                        .update_installed_plugin(
                            &component.versioned_component_id.component_id,
                            &plugin_installation_id.0,
                            &PluginInstallationUpdate {
                                priority,
                                parameters: parameters.clone().into_iter().collect(),
                            },
                        )
                        .await
                        .map(|_| ())
                        .map_service_error()?;

                    log_action("Updated", "plugin");
                }
                None => {
                    log_warn(format!("Component {component_name} not found"));
                    any_error = true;
                }
            };
        }

        if any_error {
            bail!(NonSuccessfulExit)
        }

        Ok(())
    }

    async fn cmd_uninstall(
        &self,
        component_name: Option<ComponentName>,
        plugin_installation_id: PluginInstallationId,
    ) -> anyhow::Result<()> {
        let selected_components = self
            .ctx
            .component_handler()
            .must_select_components_by_app_dir_or_name(component_name.as_ref())
            .await?;

        let mut any_error = false;
        for component_name in &selected_components.component_names {
            log_warn_action(
                "Uninstalling",
                format!("plugin {plugin_installation_id} from component {component_name}"),
            );
            let _ident = LogIndent::new();

            let component = self
                .ctx
                .component_handler()
                .component(
                    selected_components.project.as_ref(),
                    component_name.into(),
                    None,
                )
                .await?;

            let result = match component {
                Some(component) => self
                    .ctx
                    .golem_clients()
                    .await?
                    .component
                    .uninstall_plugin(
                        &component.versioned_component_id.component_id,
                        &plugin_installation_id.0,
                    )
                    .await
                    .map(|_| ())
                    .map_service_error(),
                None => {
                    log_warn(format!("Component {component_name} not found"));
                    any_error = true;
                    Ok(())
                }
            };

            match result {
                Ok(()) => {
                    log_action("Uninstalled", "plugin");
                }
                Err(error) => {
                    log_error_action("Uninstall", format!("failed: {error}"));
                    any_error = true;
                }
            }
        }

        if any_error {
            bail!(NonSuccessfulExit);
        }

        Ok(())
    }
}
