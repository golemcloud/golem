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

use crate::command_handler::component::ifs::{ComponentFilesArchive, IfsFileManager};
use crate::context::Context;
use crate::log::LogColorize;
use crate::model::app::InitialComponentFile;
use crate::model::component::ComponentDeployProperties;
use crate::model::text::plugin::PluginNameAndVersion;
use anyhow::{anyhow, Context as AnyhowContext};
use golem_client::model::EnvironmentPluginGrantWithDetails;
use golem_common::model::agent::AgentType;
use golem_common::model::component::{
    ComponentFileOptions, ComponentFilePath, PluginInstallation, PluginInstallationAction,
    PluginInstallationUpdate, PluginPriority, PluginUninstallation,
};
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_common::model::diff;
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use tokio::fs::File;

enum ComponentDiff {
    All,
    Diff { diff: diff::ComponentDiff },
}

impl ComponentDiff {
    pub fn wasm_changed(&self) -> bool {
        match self {
            ComponentDiff::All => true,
            ComponentDiff::Diff { diff } => diff.wasm_changed,
        }
    }

    pub fn agent_types_changed(&self) -> bool {
        self.wasm_changed()
    }

    pub fn files_changed(&self) -> bool {
        match self {
            ComponentDiff::All => true,
            ComponentDiff::Diff { diff } => !diff.file_changes.is_empty(),
        }
    }

    pub fn metadata_changed(&self) -> bool {
        match self {
            ComponentDiff::All => true,
            ComponentDiff::Diff { diff } => diff.metadata_changed,
        }
    }
}

pub struct ChangedComponentFiles {
    pub updated_meta_only: BTreeMap<ComponentFilePath, ComponentFileOptions>,
    pub new_or_updated_content: Option<ComponentFilesArchive>,
    pub removed: Vec<ComponentFilePath>,
}

impl ChangedComponentFiles {
    pub fn merged_file_options(&self) -> BTreeMap<ComponentFilePath, ComponentFileOptions> {
        let mut file_options = self.updated_meta_only.clone();
        if let Some(files) = &self.new_or_updated_content {
            for (path, options) in &files.file_options {
                file_options.insert(path.clone(), options.clone());
            }
        }
        file_options
    }

    pub async fn open_archive(&self) -> anyhow::Result<Option<File>> {
        match &self.new_or_updated_content {
            Some(files) => Ok(Some(files.open_archive().await?)),
            None => Ok(None),
        }
    }
}

pub struct ComponentStager<'a> {
    ctx: Arc<Context>,
    component_deploy_properties: &'a ComponentDeployProperties,
    diff: ComponentDiff,
    plugin_grants: HashMap<PluginNameAndVersion, EnvironmentPluginGrantWithDetails>,
}

impl<'a> ComponentStager<'a> {
    pub fn new(
        ctx: Arc<Context>,
        component_deploy_properties: &'a ComponentDeployProperties,
        plugin_grants: HashMap<PluginNameAndVersion, EnvironmentPluginGrantWithDetails>,
        // NOTE: none means ALL changed (e.g. new)
        diff: Option<&diff::DiffForHashOf<diff::Component>>,
    ) -> Self {
        Self {
            ctx,
            component_deploy_properties,
            diff: match diff {
                Some(diff::DiffForHashOf::HashDiff { .. }) | None => ComponentDiff::All,
                Some(diff::DiffForHashOf::ValueDiff { diff }) => {
                    ComponentDiff::Diff { diff: diff.clone() }
                }
            },
            plugin_grants,
        }
    }

    pub async fn open_linked_wasm(&self) -> anyhow::Result<File> {
        File::open(&self.component_deploy_properties.linked_wasm_path)
            .await
            .with_context(|| {
                anyhow!(
                    "Failed to open component linked WASM at {}",
                    self.component_deploy_properties
                        .linked_wasm_path
                        .display()
                        .to_string()
                        .log_color_error_highlight()
                )
            })
    }

    pub async fn open_linked_wasm_if_changed(&self) -> anyhow::Result<Option<File>> {
        if self.diff.wasm_changed() {
            Ok(Some(self.open_linked_wasm().await?))
        } else {
            Ok(None)
        }
    }

    pub async fn all_files(&self) -> anyhow::Result<Option<ComponentFilesArchive>> {
        if self.diff.files_changed() && !self.component_deploy_properties.files.is_empty() {
            Ok(Some(
                IfsFileManager::new(self.ctx.file_download_client().clone())
                    .build_files_archive(self.component_deploy_properties.files.as_slice())
                    .await?,
            ))
        } else {
            Ok(None)
        }
    }

    pub async fn changed_files(&self) -> anyhow::Result<ChangedComponentFiles> {
        match &self.diff {
            ComponentDiff::All => Ok(ChangedComponentFiles {
                updated_meta_only: BTreeMap::new(),
                new_or_updated_content: self.all_files().await?,
                removed: Vec::new(),
            }),
            ComponentDiff::Diff { diff } => {
                if diff.file_changes.is_empty() {
                    Ok(ChangedComponentFiles {
                        updated_meta_only: BTreeMap::new(),
                        new_or_updated_content: None,
                        removed: Vec::new(),
                    })
                } else {
                    let mut new_or_updated_paths = BTreeSet::<&str>::new();
                    let mut removed = Vec::<ComponentFilePath>::new();

                    for (path, change) in &diff.file_changes {
                        match change {
                            diff::BTreeMapDiffValue::Create => {
                                new_or_updated_paths.insert(path.as_str());
                            }
                            diff::BTreeMapDiffValue::Delete => {
                                removed.push(
                                    ComponentFilePath::from_either_str(path)
                                        .map_err(|err| anyhow!(err))?,
                                );
                            }
                            diff::BTreeMapDiffValue::Update(diff) => match diff {
                                diff::DiffForHashOf::HashDiff { .. } => {
                                    new_or_updated_paths.insert(path.as_str());
                                }
                                diff::DiffForHashOf::ValueDiff { diff } => {
                                    if diff.content_changed {
                                        new_or_updated_paths.insert(path.as_str());
                                    }
                                }
                            },
                        }
                    }

                    let mut files_to_archive = Vec::<InitialComponentFile>::new();
                    let mut updated_meta_only =
                        BTreeMap::<ComponentFilePath, ComponentFileOptions>::new();

                    for file in &self.component_deploy_properties.files {
                        if new_or_updated_paths.contains(file.target.path.as_abs_str()) {
                            files_to_archive.push(file.clone())
                        } else {
                            updated_meta_only.insert(
                                file.target.path.clone(),
                                ComponentFileOptions {
                                    permissions: file.target.permissions,
                                },
                            );
                        }
                    }

                    let new_or_updated_content = {
                        if files_to_archive.is_empty() {
                            None
                        } else {
                            Some(
                                IfsFileManager::new(self.ctx.file_download_client().clone())
                                    .build_files_archive(files_to_archive.as_slice())
                                    .await?,
                            )
                        }
                    };

                    Ok(ChangedComponentFiles {
                        updated_meta_only,
                        new_or_updated_content,
                        removed,
                    })
                }
            }
        }
    }

    pub fn agent_types(&self) -> &Vec<AgentType> {
        &self.component_deploy_properties.agent_types
    }

    pub fn agent_types_if_changed(&self) -> Option<&Vec<AgentType>> {
        if self.diff.agent_types_changed() {
            Some(self.agent_types())
        } else {
            None
        }
    }

    pub fn dynamic_linking(&self) -> HashMap<String, DynamicLinkedInstance> {
        self.component_deploy_properties.dynamic_linking.clone()
    }

    pub fn dynamic_linking_if_changed(&self) -> Option<HashMap<String, DynamicLinkedInstance>> {
        if self.diff.metadata_changed() {
            Some(self.dynamic_linking())
        } else {
            None
        }
    }

    pub fn env(&self) -> BTreeMap<String, String> {
        self.component_deploy_properties.env.clone()
    }

    pub fn env_if_changed(&self) -> Option<BTreeMap<String, String>> {
        if self.diff.metadata_changed() {
            Some(self.env())
        } else {
            None
        }
    }

    pub fn wasi_config_vars(&self) -> BTreeMap<String, String> {
        self.component_deploy_properties.wasi_config_vars.clone()
    }

    pub fn wasi_config_vars_if_changed(&self) -> Option<BTreeMap<String, String>> {
        if self.diff.metadata_changed() {
            Some(self.wasi_config_vars())
        } else {
            None
        }
    }

    pub fn plugins(&self) -> Vec<PluginInstallation> {
        self.component_deploy_properties
            .plugins
            .iter()
            .enumerate()
            .map(|(idx, p)| PluginInstallation {
                environment_plugin_grant_id: self
                    .plugin_grants
                    .get(&PluginNameAndVersion {
                        name: p.name.clone(),
                        version: p.version.clone(),
                    })
                    .expect("Plugin grant not found")
                    .id,
                priority: PluginPriority(idx as i32),
                parameters: p
                    .parameters
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            })
            .collect()
    }

    pub fn plugins_if_changed(&self) -> Vec<PluginInstallationAction> {
        match &self.diff {
            ComponentDiff::All => self
                .plugins()
                .into_iter()
                .map(PluginInstallationAction::Install)
                .collect(),
            ComponentDiff::Diff { diff } => {
                let mut plugins_by_grant_id = self
                    .plugins()
                    .into_iter()
                    .map(|p| (p.environment_plugin_grant_id.0, p))
                    .collect::<HashMap<_, _>>();

                diff.plugin_changes
                    .iter()
                    .map(|(grant_id, diff)| match diff {
                        diff::BTreeMapDiffValue::Create => PluginInstallationAction::Install(
                            plugins_by_grant_id
                                .remove(grant_id)
                                .expect("Missing manifest plugin for creation"),
                        ),
                        diff::BTreeMapDiffValue::Delete => {
                            PluginInstallationAction::Uninstall(PluginUninstallation {
                                environment_plugin_grant_id: EnvironmentPluginGrantId(*grant_id),
                            })
                        }
                        diff::BTreeMapDiffValue::Update(diff) => {
                            let p = plugins_by_grant_id
                                .remove(grant_id)
                                .expect("Missing manifest plugin for Update");

                            PluginInstallationAction::Update(PluginInstallationUpdate {
                                environment_plugin_grant_id: p.environment_plugin_grant_id,
                                new_priority: diff.priority_changed.then_some(p.priority),
                                new_parameters: diff.parameters_changed.then_some(p.parameters),
                            })
                        }
                    })
                    .collect()
            }
        }
    }
}
