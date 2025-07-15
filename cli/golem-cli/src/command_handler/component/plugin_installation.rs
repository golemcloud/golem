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

use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::error::NonSuccessfulExit;
use crate::log::LogColorize;
use crate::model::app::{AppComponentName, BuildProfileName, PluginInstallation};
use crate::model::component::Component;
use anyhow::bail;
use async_trait::async_trait;
use golem_client::api::ComponentClient;
use golem_common::model::plugin::{
    PluginInstallationAction, PluginInstallationCreation, PluginInstallationUpdateWithId,
    PluginUninstallation,
};
use golem_common::model::PluginInstallationId;
use itertools::Itertools;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub struct PluginInstallationHandler {
    ctx: Arc<Context>,
}

impl PluginInstallationHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    /// Applies changes to plugin installations of a given component
    ///
    /// The changes are rendered and asked for confirmation in interactive mode.
    /// The returned `Component` is updated as plugin installation changes increase the component revision.
    pub async fn apply_plugin_installation_changes(
        &self,
        component_name: &AppComponentName,
        build_profile_name: Option<&BuildProfileName>,
        component: Component,
    ) -> anyhow::Result<Component> {
        let mut target =
            ComponentPluginInstallationTarget::new(self.ctx.clone(), component.clone());

        let all_defined_installations = self
            .get_all_defined_installations(component_name, build_profile_name)
            .await?;

        let all_existing_installations = target.get_all_existing_plugin_installations().await?;

        let mut used_existing_indices = HashSet::new();
        let full_match_indices = Self::find_matches(
            &all_defined_installations,
            &all_existing_installations,
            &mut used_existing_indices,
            Self::full_match,
        );
        let partial_match_indices = Self::find_matches(
            &all_defined_installations,
            &all_existing_installations,
            &mut used_existing_indices,
            Self::partial_match,
        );

        let mapping = Self::create_mapping(
            &all_defined_installations,
            &full_match_indices,
            &partial_match_indices,
        );

        let mut commands = Vec::new();
        commands.extend(Self::collect_uninstall_commands(
            &mapping,
            &all_existing_installations,
        ));
        commands.extend(Self::collect_create_and_update_commands(
            &mapping,
            &all_defined_installations,
            &all_existing_installations,
        ));

        if commands.is_empty() {
            Ok(component)
        } else {
            let known_plugins =
                HashMap::from_iter(all_existing_installations.iter().map(|installation| {
                    (PluginInstallationId(installation.id), installation.clone())
                }));
            let rendered_commands = commands
                .iter()
                .map(|cmd| cmd.render(&known_plugins))
                .collect::<Vec<_>>();
            if self
                .ctx
                .interactive_handler()
                .confirm_plugin_installation_changes(component_name, &rendered_commands)?
            {
                for command in commands {
                    target.execute(command).await?;
                }
                target.finish().await?;

                let Some(latest_component) = self
                    .ctx
                    .component_handler()
                    .latest_component_by_id(component.versioned_component_id.component_id)
                    .await?
                else {
                    bail!(
                        "Component {} not found, after plugin deployment",
                        component.component_name.0.log_color_highlight()
                    );
                };

                Ok(latest_component)
            } else {
                bail!(NonSuccessfulExit);
            }
        }
    }

    /// Find matching between app-manifest defined plugin installations and existing plugin installations
    ///
    /// The `used_existing_indices` set is updated with each found match to ensure that every existing plugin
    /// installation is only matched once, even when `find_matches` is called multiple times with different comparison
    /// functions.
    ///
    /// To be used with the `full_match` and `partial_match` functions.
    ///
    /// The resulting map connects indices of `all_defined_installations` to indices of `all_existing_installations`.
    fn find_matches(
        all_defined_installations: &[PluginInstallation],
        all_existing_installations: &[golem_client::model::PluginInstallation],
        used_existing_indices: &mut HashSet<usize>,
        comparison: impl Fn(&PluginInstallation, &golem_client::model::PluginInstallation) -> bool,
    ) -> HashMap<usize, usize> {
        let mut full_match_indices = HashMap::new();
        for (defined_idx, defined_installation) in all_defined_installations.iter().enumerate() {
            if let Some((existing_idx, _)) = all_existing_installations
                .iter()
                .enumerate()
                .find_position(|(existing_idx, existing_installation)| {
                    !used_existing_indices.contains(existing_idx)
                        && comparison(defined_installation, existing_installation)
                })
            {
                used_existing_indices.insert(existing_idx);
                full_match_indices.insert(defined_idx, existing_idx);
            }
        }
        full_match_indices
    }

    /// Full match between a plugin installation definition and the server state means that the
    /// plugin name and version are matched, as well as the plugin parameters.
    fn full_match(
        defined: &PluginInstallation,
        existing: &golem_client::model::PluginInstallation,
    ) -> bool {
        defined.name == existing.plugin_name
            && defined.version == existing.plugin_version
            && defined.parameters == existing.parameters
    }

    /// Partial match means the plugin name and version are matched, but the parameters are not
    fn partial_match(
        defined: &PluginInstallation,
        existing: &golem_client::model::PluginInstallation,
    ) -> bool {
        defined.name == existing.plugin_name && defined.version == existing.plugin_version
    }

    /// Get all the plugin installations for a given component from the app manifest
    async fn get_all_defined_installations(
        &self,
        component_name: &AppComponentName,
        build_profile_name: Option<&BuildProfileName>,
    ) -> anyhow::Result<Vec<PluginInstallation>> {
        let app_ctx = self.ctx.app_context_lock().await;
        let props = app_ctx
            .some_or_err()?
            .application
            .component_properties(component_name, build_profile_name);

        Ok(props.plugins.clone())
    }

    /// Create a mapping value for each defined installation. See `Mapping` for the possible values.
    fn create_mapping(
        all_defined_installations: &[PluginInstallation],
        full_match_indices: &HashMap<usize, usize>,
        partial_match_indices: &HashMap<usize, usize>,
    ) -> Vec<Mapping> {
        let mut mapping = Vec::new();
        for (defined_idx, _) in all_defined_installations.iter().enumerate() {
            if let Some(existing_idx) = full_match_indices.get(&defined_idx) {
                mapping.push(Mapping::UseExisting(*existing_idx))
            } else if let Some(existing_idx) = partial_match_indices.get(&defined_idx) {
                mapping.push(Mapping::UpdateExisting(*existing_idx))
            } else {
                mapping.push(Mapping::CreateNew);
            }
        }
        mapping
    }

    /// Generates uninstall `Command`s for every plugin installation existing on the server which
    /// are not going to be used by the new state. This is decided by checking the generated `Mapping`s
    /// and only NOT uninstalling those that are used in any of the mappings.
    fn collect_uninstall_commands(
        mappings: &[Mapping],
        all_existing_installations: &[golem_client::model::PluginInstallation],
    ) -> Vec<Command> {
        let mut commands = Vec::new();
        let mut indices: HashSet<usize> = HashSet::from_iter(0..all_existing_installations.len());
        for mapping in mappings {
            match mapping {
                Mapping::CreateNew => {}
                Mapping::UseExisting(existing_idx) | Mapping::UpdateExisting(existing_idx) => {
                    indices.remove(existing_idx);
                }
            }
        }
        for idx in indices {
            let installation = &all_existing_installations[idx];
            commands.push(Command::Uninstall {
                id: PluginInstallationId(installation.id),
            });
        }
        commands
    }

    /// Generates `Command`s for creating new installations and updating existing ones.
    fn collect_create_and_update_commands(
        mappings: &[Mapping],
        all_defined_installations: &[PluginInstallation],
        all_existing_installations: &[golem_client::model::PluginInstallation],
    ) -> Vec<Command> {
        let mut last_priority = None;
        let mut commands = Vec::new();

        for (defined_idx, mapping) in mappings.iter().enumerate() {
            match mapping {
                Mapping::CreateNew => {
                    let priority = if let Some(last_priority_value) = last_priority {
                        last_priority = Some(last_priority_value + 1);
                        last_priority_value + 1
                    } else {
                        last_priority = Some(0);
                        0
                    };
                    commands.push(Command::Create {
                        definition: all_defined_installations[defined_idx].clone(),
                        priority,
                    })
                }
                Mapping::UseExisting(existing_idx) => {
                    let existing_installation = &all_existing_installations[*existing_idx];
                    let existing_priority = existing_installation.priority;
                    if let Some(last_priority_value) = last_priority {
                        if existing_priority > last_priority_value {
                            last_priority = Some(existing_priority);
                        } else {
                            let updated_priority = last_priority_value + 1;
                            last_priority = Some(updated_priority);
                            commands.push(Command::Update {
                                id: PluginInstallationId(existing_installation.id),
                                priority: updated_priority,
                                parameters: existing_installation.parameters.clone(),
                            })
                        }
                    } else {
                        last_priority = Some(existing_priority);
                    }
                }
                Mapping::UpdateExisting(existing_idx) => {
                    let defined_installation = &all_defined_installations[defined_idx];
                    let existing_installation = &all_existing_installations[*existing_idx];
                    let existing_priority = existing_installation.priority;
                    let updated_priority = if let Some(last_priority_value) = last_priority {
                        if existing_priority > last_priority_value {
                            last_priority = Some(existing_priority);
                            existing_priority
                        } else {
                            let updated_priority = last_priority_value + 1;
                            last_priority = Some(updated_priority);
                            updated_priority
                        }
                    } else {
                        last_priority = Some(existing_priority);
                        existing_priority
                    };
                    commands.push(Command::Update {
                        id: PluginInstallationId(existing_installation.id),
                        priority: updated_priority,
                        parameters: defined_installation.parameters.clone(),
                    })
                }
            }
        }

        commands
    }
}

enum Mapping {
    /// The corresponding plugin installation is new
    CreateNew,
    /// The corresponding plugin installation fully matches an existing one
    UseExisting(usize),
    /// The corresponding plugin installation partially matches an existing one
    UpdateExisting(usize),
}

enum Command {
    /// Uninstall an existing plugin installation
    Uninstall { id: PluginInstallationId },
    /// Update an existing plugin installation with new priority and parameters
    Update {
        id: PluginInstallationId,
        priority: i32,
        parameters: HashMap<String, String>,
    },
    /// Create a new plugin installation
    Create {
        definition: PluginInstallation,
        priority: i32,
    },
}

impl Command {
    /// Renders the command for the interactive confirmation question
    pub fn render(
        &self,
        known_plugins: &HashMap<PluginInstallationId, golem_client::model::PluginInstallation>,
    ) -> String {
        match self {
            Command::Uninstall { id } => {
                if let Some(def) = known_plugins.get(id) {
                    format!(
                        "Uninstalling plugin {} version {}",
                        def.plugin_name.log_color_highlight(),
                        def.plugin_version.log_color_highlight()
                    )
                } else {
                    format!(
                        "{} plugin installation {}",
                        "Uninstalling".log_color_warn(),
                        id.to_string().log_color_highlight()
                    )
                }
            }
            Command::Update {
                id,
                priority,
                parameters,
            } => {
                let plugin_description = if let Some(def) = known_plugins.get(id) {
                    format!(
                        "plugin {} version {}",
                        def.plugin_name.log_color_highlight(),
                        def.plugin_version.log_color_highlight()
                    )
                } else {
                    format!(
                        "plugin installation {}",
                        id.to_string().log_color_highlight()
                    )
                };
                let param_list = parameters
                    .iter()
                    .map(|(k, v)| {
                        format!("{}: {}", k.log_color_highlight(), v.log_color_highlight())
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "Updating {} to priority {} with parameters: {}",
                    plugin_description,
                    priority.to_string().log_color_highlight(),
                    param_list
                )
            }
            Command::Create {
                definition,
                priority,
            } => {
                let param_list = definition
                    .parameters
                    .iter()
                    .map(|(k, v)| {
                        format!("{}: {}", k.log_color_highlight(), v.log_color_highlight())
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "Installing plugin {} version {} with priority {} and parameters: {}",
                    definition.name.log_color_highlight(),
                    definition.version.log_color_highlight(),
                    priority.to_string().log_color_highlight(),
                    param_list
                )
            }
        }
    }
}

/// Abstraction of handling plugin installations in either components and cloud projects
#[async_trait]
trait PluginInstallationTarget {
    async fn get_all_existing_plugin_installations(
        &self,
    ) -> anyhow::Result<Vec<golem_client::model::PluginInstallation>>;

    async fn execute(&mut self, command: Command) -> anyhow::Result<()> {
        match command {
            Command::Uninstall { id } => self.uninstall(&id).await,
            Command::Update {
                id,
                priority,
                parameters,
            } => self.update(&id, priority, parameters).await,
            Command::Create {
                definition,
                priority,
            } => self.create(definition, priority).await,
        }
    }

    async fn uninstall(&mut self, id: &PluginInstallationId) -> anyhow::Result<()>;
    async fn update(
        &mut self,
        id: &PluginInstallationId,
        priority: i32,
        parameters: HashMap<String, String>,
    ) -> anyhow::Result<()>;
    async fn create(&mut self, definition: PluginInstallation, priority: i32)
        -> anyhow::Result<()>;

    /// Finishes a set of update commands; this can run the actual update in case the implementation supports batching
    async fn finish(&mut self) -> anyhow::Result<()>;
}

/// PluginInstallationTarget implementation for components
struct ComponentPluginInstallationTarget {
    ctx: Arc<Context>,
    component: Component,
    actions: Vec<PluginInstallationAction>,
}

impl ComponentPluginInstallationTarget {
    fn new(ctx: Arc<Context>, component: Component) -> Self {
        Self {
            ctx,
            component,
            actions: Vec::new(),
        }
    }
}

#[async_trait]
impl PluginInstallationTarget for ComponentPluginInstallationTarget {
    async fn get_all_existing_plugin_installations(
        &self,
    ) -> anyhow::Result<Vec<golem_client::model::PluginInstallation>> {
        let clients = self.ctx.golem_clients().await?;

        let mut installations = clients
            .component
            .get_installed_plugins(
                &self.component.versioned_component_id.component_id,
                &self.component.versioned_component_id.version.to_string(),
            )
            .await
            .map_service_error()?;

        installations.sort_by_key(|i| i.priority);
        Ok(installations)
    }

    async fn uninstall(&mut self, id: &PluginInstallationId) -> anyhow::Result<()> {
        self.actions
            .push(PluginInstallationAction::Uninstall(PluginUninstallation {
                installation_id: id.clone(),
            }));
        Ok(())
    }

    async fn update(
        &mut self,
        id: &PluginInstallationId,
        priority: i32,
        parameters: HashMap<String, String>,
    ) -> anyhow::Result<()> {
        self.actions.push(PluginInstallationAction::Update(
            PluginInstallationUpdateWithId {
                installation_id: id.clone(),
                priority,
                parameters: parameters.clone(),
            },
        ));
        Ok(())
    }

    async fn create(
        &mut self,
        definition: PluginInstallation,
        priority: i32,
    ) -> anyhow::Result<()> {
        self.actions.push(PluginInstallationAction::Install(
            PluginInstallationCreation {
                name: definition.name,
                version: definition.version,
                parameters: definition.parameters,
                priority,
            },
        ));
        Ok(())
    }

    async fn finish(&mut self) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .component
            .batch_update_installed_plugins(
                &self.component.versioned_component_id.component_id,
                &golem_client::model::BatchPluginInstallationUpdates {
                    actions: self.actions.drain(..).collect(),
                },
            )
            .await
            .map_service_error()?;

        Ok(())
    }
}
