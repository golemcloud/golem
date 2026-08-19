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

use crate::command_handler::component::ifs::{
    ComponentFilesArchive, IfsFileManager, expand_component_files,
    resolve_archive_paths_for_sources,
};
use crate::context::Context;
use crate::log::LogColorize;
use crate::model::app::{
    CanonicalFilePathWithPermissions, InitialComponentFile, InitialComponentFileSource,
};
use crate::model::app_raw;
use crate::model::component::initial_permission_recipient_context;
use crate::model::component::{AgentTypeManifestProvisionConfig, ComponentDeployProperties};
use crate::model::environment::ResolvedEnvironmentIdentity;
use crate::model::plugin::PluginNameAndVersion;
use anyhow::{Context as AnyhowContext, anyhow};
use golem_client::model::EnvironmentPluginGrantWithDetails;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::{
    AgentFileOptions, AgentFilePath, AgentFilePermissions, AgentTypeInitialPermissions,
    AgentTypeProvisionConfigCreation, AgentTypeProvisionConfigUpdate, ArchiveFilePath,
    ComponentName, PluginInstallation, PluginInstallationAction, PluginInstallationUpdate,
    PluginPriority, PluginUninstallation, ToolDeploymentConfigCreation, ToolDeploymentConfigUpdate,
    ToolProvisionConfigCreation, ToolProvisionConfigUpdate,
};
use golem_common::model::diff::{self, AgentFileDiff, AgentTypeProvisionConfigDiff};
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::model::optional_field_update::OptionalFieldUpdate;
use golem_common::model::tool::ToolName;
use golem_common::schema::agent::AgentTypeSchema;
use golem_common::schema::tool::Tool;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use tokio::fs::File;
use tokio::sync::OnceCell;

fn resolve_ifs_entry(
    file: &app_raw::InitialComponentFile,
    source: &std::path::Path,
) -> anyhow::Result<InitialComponentFile> {
    let source = InitialComponentFileSource::new(&file.source_path, source)
        .map_err(|e| anyhow::anyhow!("Invalid IFS source path '{}': {e}", file.source_path))?;
    Ok(InitialComponentFile {
        source,
        target: CanonicalFilePathWithPermissions {
            path: file.target_path.clone(),
            permissions: file.permissions.unwrap_or(AgentFilePermissions::ReadOnly),
        },
    })
}

fn resolve_tool_ifs_entry(
    file: &crate::model::app::ToolProvisionFile,
) -> anyhow::Result<InitialComponentFile> {
    resolve_ifs_entry(&file.file, &file.source)
}

#[derive(Debug)]
enum ComponentDiff {
    All,
    Diff { diff: diff::ComponentDiff },
}

impl ComponentDiff {
    pub fn new(diff: Option<&diff::DiffForHashOf<diff::Component>>) -> anyhow::Result<Self> {
        Ok(match diff {
            None => ComponentDiff::All,
            Some(diff::DiffForHashOf::HashDiff { .. }) => {
                return Err(anyhow!(
                    "Cannot stage component update from a hash-only diff; component details were not loaded"
                ));
            }
            Some(diff::DiffForHashOf::ValueDiff { diff }) => {
                ComponentDiff::Diff { diff: diff.clone() }
            }
        })
    }

    pub fn wasm_changed(&self) -> bool {
        match self {
            ComponentDiff::All => true,
            ComponentDiff::Diff { diff } => diff.wasm_changed,
        }
    }

    pub fn agent_types_changed(&self) -> bool {
        self.wasm_changed()
    }

    pub fn provision_config_changed(&self) -> bool {
        match self {
            ComponentDiff::All => true,
            ComponentDiff::Diff { diff } => {
                !diff.agent_type_provision_config_changes.is_empty()
                    || !diff.tool_deployment_config_changes.is_empty()
            }
        }
    }

    pub fn tools_changed(&self) -> bool {
        match self {
            ComponentDiff::All => true,
            ComponentDiff::Diff { diff } => {
                diff.tool_deployment_config_changes
                    .values()
                    .any(|change| match change {
                        diff::BTreeMapDiffValue::Create | diff::BTreeMapDiffValue::Delete => true,
                        diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::HashDiff {
                            ..
                        }) => true,
                        diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff {
                            diff,
                        }) => diff.definition_changed,
                    })
            }
        }
    }

    pub fn changed_agent_types(&self) -> Option<BTreeSet<String>> {
        match self {
            ComponentDiff::All => None,
            ComponentDiff::Diff { diff } => {
                if diff.agent_type_provision_config_changes.is_empty() {
                    Some(BTreeSet::new())
                } else {
                    Some(
                        diff.agent_type_provision_config_changes
                            .keys()
                            .cloned()
                            .collect(),
                    )
                }
            }
        }
    }

    pub fn file_changes_per_agent(&self) -> Vec<(&str, &AgentTypeProvisionConfigDiff)> {
        match self {
            ComponentDiff::All => vec![],
            ComponentDiff::Diff { diff } => {
                diff.agent_type_provision_config_changes
                    .iter()
                    .filter_map(|(name, change)| match change {
                        diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff {
                            diff,
                        }) if !diff.file_changes.is_empty() => Some((name.as_str(), diff)),
                        _ => None,
                    })
                    .collect()
            }
        }
    }

    pub fn changed_tool_names(&self) -> Option<BTreeSet<String>> {
        match self {
            ComponentDiff::All => None,
            ComponentDiff::Diff { diff } => Some(
                diff.tool_deployment_config_changes
                    .keys()
                    .cloned()
                    .collect(),
            ),
        }
    }

    pub fn file_changes_per_tool(&self) -> Vec<(&str, &diff::ToolDeploymentConfigDiff)> {
        match self {
            ComponentDiff::All => Vec::new(),
            ComponentDiff::Diff { diff } => {
                diff.tool_deployment_config_changes
                    .iter()
                    .filter_map(|(name, change)| match change {
                        diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff {
                            diff,
                        }) if !diff.file_changes.is_empty() => Some((name.as_str(), diff)),
                        _ => None,
                    })
                    .collect()
            }
        }
    }
}

pub struct ChangedComponentFiles {
    pub new_or_updated_content: Option<ComponentFilesArchive>,
    pub removed_per_agent: BTreeMap<AgentTypeName, Vec<AgentFilePath>>,
    pub archive_paths_by_source: BTreeMap<String, ArchiveFilePath>,
    /// Files whose only change is permissions — no content re-upload needed.
    pub file_permission_updates_per_agent:
        BTreeMap<AgentTypeName, BTreeMap<AgentFilePath, AgentFilePermissions>>,
    pub removed_per_tool: BTreeMap<ToolName, Vec<AgentFilePath>>,
    pub file_permission_updates_per_tool:
        BTreeMap<ToolName, BTreeMap<AgentFilePath, AgentFilePermissions>>,
}

impl ChangedComponentFiles {
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
    manifest_files_by_agent: OnceCell<BTreeMap<AgentTypeName, Vec<InitialComponentFile>>>,
    manifest_files_by_tool: OnceCell<BTreeMap<ToolName, Vec<InitialComponentFile>>>,
}

impl<'a> ComponentStager<'a> {
    pub fn new(
        ctx: Arc<Context>,
        component_deploy_properties: &'a ComponentDeployProperties,
        plugin_grants: HashMap<PluginNameAndVersion, EnvironmentPluginGrantWithDetails>,
        // NOTE: none means ALL changed (e.g. new component)
        diff: Option<&diff::DiffForHashOf<diff::Component>>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            ctx,
            component_deploy_properties,
            diff: ComponentDiff::new(diff)?,
            plugin_grants,
            manifest_files_by_agent: OnceCell::new(),
            manifest_files_by_tool: OnceCell::new(),
        })
    }

    pub async fn open_wasm(&self) -> anyhow::Result<File> {
        File::open(&self.component_deploy_properties.wasm_path)
            .await
            .with_context(|| {
                anyhow!(
                    "Failed to open component output WASM at {}",
                    self.component_deploy_properties
                        .wasm_path
                        .display()
                        .to_string()
                        .log_color_error_highlight()
                )
            })
    }

    pub async fn open_wasm_if_changed(&self) -> anyhow::Result<Option<File>> {
        if self.diff.wasm_changed() {
            Ok(Some(self.open_wasm().await?))
        } else {
            Ok(None)
        }
    }

    async fn manifest_files_by_agent(
        &self,
    ) -> anyhow::Result<&BTreeMap<AgentTypeName, Vec<InitialComponentFile>>> {
        self.manifest_files_by_agent
            .get_or_try_init(|| async {
                let mut result = BTreeMap::new();

                for (agent_type_name, manifest_config) in
                    &self.component_deploy_properties.agent_type_configs
                {
                    let files = manifest_config
                        .files
                        .iter()
                        .map(|file| resolve_ifs_entry(file, &manifest_config.files_source))
                        .collect::<anyhow::Result<Vec<_>>>()?;

                    result.insert(
                        agent_type_name.clone(),
                        expand_component_files(&files).await?,
                    );
                }

                Ok(result)
            })
            .await
    }

    async fn manifest_files_for_agent(
        &self,
        agent_type_name: &AgentTypeName,
    ) -> anyhow::Result<Vec<InitialComponentFile>> {
        Ok(self
            .manifest_files_by_agent()
            .await?
            .get(agent_type_name)
            .cloned()
            .unwrap_or_default())
    }

    async fn manifest_files_by_tool(
        &self,
    ) -> anyhow::Result<&BTreeMap<ToolName, Vec<InitialComponentFile>>> {
        self.manifest_files_by_tool
            .get_or_try_init(|| async {
                let mut result = BTreeMap::new();
                for (tool_name, manifest_config) in
                    &self.component_deploy_properties.tool_deployment_configs
                {
                    let files = manifest_config
                        .provision
                        .files
                        .iter()
                        .map(resolve_tool_ifs_entry)
                        .collect::<anyhow::Result<Vec<_>>>()?;
                    result.insert(tool_name.clone(), expand_component_files(&files).await?);
                }
                Ok(result)
            })
            .await
    }

    async fn manifest_files_for_tool(
        &self,
        tool_name: &ToolName,
    ) -> anyhow::Result<Vec<InitialComponentFile>> {
        Ok(self
            .manifest_files_by_tool()
            .await?
            .get(tool_name)
            .cloned()
            .unwrap_or_default())
    }

    async fn all_manifest_files(&self) -> anyhow::Result<Vec<InitialComponentFile>> {
        let mut files = self
            .manifest_files_by_agent()
            .await?
            .values()
            .flatten()
            .cloned()
            .collect::<Vec<_>>();
        files.extend(
            self.manifest_files_by_tool()
                .await?
                .values()
                .flatten()
                .cloned(),
        );
        Ok(files)
    }

    async fn changed_manifest_files(&self) -> anyhow::Result<Vec<InitialComponentFile>> {
        if matches!(self.diff, ComponentDiff::All) {
            return self.all_manifest_files().await;
        }

        let mut result = Vec::new();
        if let Some(changed) = self.diff.changed_agent_types()
            && !changed.is_empty()
        {
            for (agent_type_name, _) in self
                .component_deploy_properties
                .agent_type_configs
                .iter()
                .filter(|(name, _)| changed.contains(name.0.as_str()))
            {
                let Some(agent_change) = self.agent_change(agent_type_name) else {
                    continue;
                };

                match agent_change {
                    diff::BTreeMapDiffValue::Create => {
                        result.extend(self.manifest_files_for_agent(agent_type_name).await?);
                    }
                    diff::BTreeMapDiffValue::Delete => {}
                    diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::HashDiff { .. }) => {
                        return Err(anyhow!(
                            "Cannot determine changed file contents for agent type {} from a hash-only provision config diff; component details were not loaded",
                            agent_type_name.0.log_color_highlight()
                        ));
                    }
                    diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff { diff }) => {
                        let content_changed_paths = content_changed_file_paths(diff);
                        if content_changed_paths.is_empty() {
                            continue;
                        }

                        result.extend(
                            self.manifest_files_for_agent(agent_type_name)
                                .await?
                                .into_iter()
                                .filter(|file| {
                                    content_changed_paths.contains(file.target.path.as_abs_str())
                                }),
                        );
                    }
                }
            }
        }

        if let Some(changed_tools) = self.diff.changed_tool_names() {
            for raw_name in changed_tools {
                let tool_name =
                    ToolName::try_from(raw_name.as_str()).map_err(anyhow::Error::msg)?;
                let Some(tool_change) = self.tool_change(&tool_name) else {
                    continue;
                };
                match tool_change {
                    diff::BTreeMapDiffValue::Create => {
                        result.extend(self.manifest_files_for_tool(&tool_name).await?);
                    }
                    diff::BTreeMapDiffValue::Delete => {}
                    diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::HashDiff { .. }) => {
                        return Err(anyhow!(
                            "Cannot determine changed file contents for tool {} from a hash-only deployment config diff; component details were not loaded",
                            tool_name.as_str().log_color_highlight()
                        ));
                    }
                    diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff { diff }) => {
                        let content_changed_paths = content_changed_tool_file_paths(diff);
                        result.extend(
                            self.manifest_files_for_tool(&tool_name)
                                .await?
                                .into_iter()
                                .filter(|file| {
                                    content_changed_paths.contains(file.target.path.as_abs_str())
                                }),
                        );
                    }
                }
            }
        }

        Ok(result)
    }

    pub async fn all_files(&self) -> anyhow::Result<Option<ComponentFilesArchive>> {
        let files = self.all_manifest_files().await?;
        if files.is_empty() {
            return Ok(None);
        }
        Ok(Some(
            IfsFileManager::new(self.ctx.file_download_client().clone())
                .build_files_archive(&files)
                .await?,
        ))
    }

    pub async fn changed_files(&self) -> anyhow::Result<ChangedComponentFiles> {
        if !self.diff.provision_config_changed() {
            return Ok(ChangedComponentFiles {
                new_or_updated_content: None,
                removed_per_agent: BTreeMap::new(),
                archive_paths_by_source: BTreeMap::new(),
                file_permission_updates_per_agent: BTreeMap::new(),
                removed_per_tool: BTreeMap::new(),
                file_permission_updates_per_tool: BTreeMap::new(),
            });
        }

        let files_to_archive = self.changed_manifest_files().await?;
        let archive_paths_by_source = resolve_archive_paths_for_sources(
            files_to_archive
                .iter()
                .map(|file| file.source.as_url().clone()),
        )?;
        let new_or_updated_content = if files_to_archive.is_empty() {
            None
        } else {
            Some(
                IfsFileManager::new(self.ctx.file_download_client().clone())
                    .build_files_archive(&files_to_archive)
                    .await?,
            )
        };

        // Compute removed files per agent type from the fine-grained diff
        let mut removed_per_agent = BTreeMap::new();
        for (agent_type_str, agent_diff) in self.diff.file_changes_per_agent() {
            let removed: Vec<AgentFilePath> = agent_diff
                .file_changes
                .iter()
                .filter_map(|(path, change)| {
                    if matches!(change, diff::BTreeMapDiffValue::Delete) {
                        AgentFilePath::from_abs_str(path).ok()
                    } else {
                        None
                    }
                })
                .collect();
            if !removed.is_empty() {
                removed_per_agent.insert(
                    golem_common::model::agent::AgentTypeName(agent_type_str.to_string()),
                    removed,
                );
            }
        }

        let mut removed_per_tool = BTreeMap::new();
        for (tool_name, tool_diff) in self.diff.file_changes_per_tool() {
            let removed = tool_diff
                .file_changes
                .iter()
                .filter_map(|(path, change)| {
                    matches!(change, diff::BTreeMapDiffValue::Delete)
                        .then(|| AgentFilePath::from_abs_str(path).ok())
                        .flatten()
                })
                .collect::<Vec<_>>();
            if !removed.is_empty() {
                removed_per_tool.insert(
                    ToolName::try_from(tool_name).map_err(anyhow::Error::msg)?,
                    removed,
                );
            }
        }

        // Compute permissions-only updates per agent type
        let mut file_permission_updates_per_agent = BTreeMap::new();
        for (agent_type_str, agent_diff) in self.diff.file_changes_per_agent() {
            let agent_id = golem_common::model::agent::AgentTypeName(agent_type_str.to_string());
            let manifest_files = match self
                .component_deploy_properties
                .agent_type_configs
                .get(&agent_id)
            {
                Some(_) => self.manifest_files_for_agent(&agent_id).await?,
                None => Vec::new(),
            };
            let manifest_files: std::collections::HashMap<_, _> = manifest_files
                .iter()
                .map(|f| (f.target.path.as_abs_str(), f))
                .collect();

            let mut perm_updates: BTreeMap<AgentFilePath, AgentFilePermissions> = BTreeMap::new();
            for (path, change) in &agent_diff.file_changes {
                if let diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff {
                    diff:
                        AgentFileDiff {
                            content_changed: false,
                            permissions_changed: true,
                        },
                }) = change
                    && let Ok(file_path) = AgentFilePath::from_abs_str(path)
                {
                    // Look up the new permissions from the manifest
                    let new_perms = manifest_files
                        .get(path.as_str())
                        .map(|f| f.target.permissions)
                        .unwrap_or(AgentFilePermissions::ReadOnly);
                    perm_updates.insert(file_path, new_perms);
                }
            }
            if !perm_updates.is_empty() {
                file_permission_updates_per_agent.insert(agent_id, perm_updates);
            }
        }

        let mut file_permission_updates_per_tool = BTreeMap::new();
        for (raw_tool_name, tool_diff) in self.diff.file_changes_per_tool() {
            let tool_name = ToolName::try_from(raw_tool_name).map_err(anyhow::Error::msg)?;
            let manifest_files = self.manifest_files_for_tool(&tool_name).await?;
            let manifest_files = manifest_files
                .iter()
                .map(|file| (file.target.path.as_abs_str(), file))
                .collect::<HashMap<_, _>>();
            let mut permission_updates = BTreeMap::new();
            for (path, change) in &tool_diff.file_changes {
                if let diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff {
                    diff:
                        AgentFileDiff {
                            content_changed: false,
                            permissions_changed: true,
                        },
                }) = change
                    && let Ok(file_path) = AgentFilePath::from_abs_str(path)
                {
                    let permissions = manifest_files
                        .get(path.as_str())
                        .map(|file| file.target.permissions)
                        .unwrap_or(AgentFilePermissions::ReadOnly);
                    permission_updates.insert(file_path, permissions);
                }
            }
            if !permission_updates.is_empty() {
                file_permission_updates_per_tool.insert(tool_name, permission_updates);
            }
        }

        Ok(ChangedComponentFiles {
            new_or_updated_content,
            removed_per_agent,
            archive_paths_by_source,
            file_permission_updates_per_agent,
            removed_per_tool,
            file_permission_updates_per_tool,
        })
    }

    pub fn agent_types(&self) -> &Vec<AgentTypeSchema> {
        &self.component_deploy_properties.agent_types
    }

    pub fn agent_types_if_changed(&self) -> Option<&Vec<AgentTypeSchema>> {
        if self.diff.agent_types_changed() {
            Some(self.agent_types())
        } else {
            None
        }
    }

    pub fn tools(&self) -> &Vec<Tool> {
        &self.component_deploy_properties.tools
    }

    pub fn tools_if_changed(&self) -> Option<&Vec<Tool>> {
        if self.diff.tools_changed() {
            Some(self.tools())
        } else {
            None
        }
    }

    pub async fn tool_deployment_configs(
        &self,
    ) -> anyhow::Result<BTreeMap<ToolName, ToolDeploymentConfigCreation>> {
        let all_files = self.all_manifest_files().await?;
        let archive_paths_by_source = resolve_archive_paths_for_sources(
            all_files.iter().map(|file| file.source.as_url().clone()),
        )?;
        let mut result = BTreeMap::new();
        for (tool_name, manifest_config) in
            &self.component_deploy_properties.tool_deployment_configs
        {
            result.insert(
                tool_name.clone(),
                ToolDeploymentConfigCreation {
                    provision: ToolProvisionConfigCreation {
                        config: manifest_config.provision.config.clone(),
                        env: manifest_config.provision.env.clone(),
                        plugin_installations: self
                            .resolve_plugins(&manifest_config.provision.plugins)?,
                        files: self
                            .resolve_archive_files_for_tool(tool_name, &archive_paths_by_source)
                            .await?,
                    },
                    environment_binding: manifest_config.environment_binding.clone(),
                    agent_bindings: manifest_config.agent_bindings.clone(),
                },
            );
        }
        Ok(result)
    }

    pub async fn tool_deployment_config_updates_if_changed(
        &self,
        changed_files: &ChangedComponentFiles,
    ) -> anyhow::Result<Option<BTreeMap<ToolName, ToolDeploymentConfigUpdate>>> {
        let Some(changed_tool_names) = self.diff.changed_tool_names() else {
            return Err(anyhow!("Tool update diff is unavailable"));
        };
        if changed_tool_names.is_empty() {
            return Ok(None);
        }

        let mut result = BTreeMap::new();
        for raw_tool_name in changed_tool_names {
            let tool_name =
                ToolName::try_from(raw_tool_name.as_str()).map_err(anyhow::Error::msg)?;
            let Some(manifest_config) = self
                .component_deploy_properties
                .tool_deployment_configs
                .get(&tool_name)
            else {
                continue;
            };
            let Some(change) = self.tool_change(&tool_name) else {
                continue;
            };
            let resolved_plugins = self.resolve_plugins(&manifest_config.provision.plugins)?;
            let archive_files = self
                .resolve_archive_files_for_tool(&tool_name, &changed_files.archive_paths_by_source)
                .await?;

            let (plugin_updates, environment_binding, agent_bindings) = match change {
                diff::BTreeMapDiffValue::Create => (
                    resolved_plugins
                        .iter()
                        .cloned()
                        .map(PluginInstallationAction::Install)
                        .collect(),
                    OptionalFieldUpdate::update_from_option(
                        manifest_config.environment_binding.clone(),
                    ),
                    Some(manifest_config.agent_bindings.clone()),
                ),
                diff::BTreeMapDiffValue::Delete => continue,
                diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::HashDiff { .. }) => {
                    return Err(anyhow!(
                        "Cannot stage tool {} from a hash-only deployment config diff; component details were not loaded",
                        tool_name.as_str().log_color_highlight()
                    ));
                }
                diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff { diff }) => (
                    self.plugin_updates_for_tool_change(
                        &tool_name,
                        &diff.plugin_changes,
                        &resolved_plugins,
                    )?,
                    if diff.environment_binding_changed {
                        OptionalFieldUpdate::update_from_option(
                            manifest_config.environment_binding.clone(),
                        )
                    } else {
                        OptionalFieldUpdate::NoChange
                    },
                    (!diff.agent_binding_changes.is_empty())
                        .then(|| manifest_config.agent_bindings.clone()),
                ),
            };

            result.insert(
                tool_name.clone(),
                ToolDeploymentConfigUpdate {
                    provision: Some(ToolProvisionConfigUpdate {
                        config: Some(manifest_config.provision.config.clone()),
                        env: Some(manifest_config.provision.env.clone()),
                        plugin_updates,
                        files_to_add_or_update: self
                            .files_to_add_or_update_for_tool(&tool_name, archive_files)?,
                        files_to_remove: changed_files
                            .removed_per_tool
                            .get(&tool_name)
                            .cloned()
                            .unwrap_or_default(),
                        file_permission_updates: changed_files
                            .file_permission_updates_per_tool
                            .get(&tool_name)
                            .cloned()
                            .unwrap_or_default(),
                    }),
                    environment_binding,
                    agent_bindings,
                },
            );
        }

        Ok((!result.is_empty()).then_some(result))
    }

    fn resolve_plugins_for(
        &self,
        manifest_config: &AgentTypeManifestProvisionConfig,
    ) -> anyhow::Result<Vec<PluginInstallation>> {
        self.resolve_plugins(&manifest_config.plugins)
    }

    fn resolve_plugins(
        &self,
        plugins: &[app_raw::PluginInstallation],
    ) -> anyhow::Result<Vec<PluginInstallation>> {
        plugins
            .iter()
            .enumerate()
            .map(|(idx, p)| {
                let grant = self
                    .plugin_grants
                    .get(&PluginNameAndVersion {
                        name: p.name.clone(),
                        version: p.version.clone(),
                    })
                    .ok_or_else(|| {
                        anyhow!(
                            "Plugin {}/{} is not available in this environment. \
                             Use 'golem plugin list' to see available plugins, \
                             or grant the plugin to this environment first.",
                            p.name,
                            p.version
                        )
                    })?;
                Ok(PluginInstallation {
                    environment_plugin_grant_id: grant.id,
                    priority: PluginPriority(idx as i32),
                    parameters: p
                        .parameters
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                })
            })
            .collect()
    }

    async fn resolve_archive_files_for_agent(
        &self,
        agent_type_name: &AgentTypeName,
        archive_paths_by_source: &BTreeMap<String, ArchiveFilePath>,
    ) -> anyhow::Result<BTreeMap<ArchiveFilePath, AgentFileOptions>> {
        let mut archive_files = BTreeMap::new();

        for resolved in self.manifest_files_for_agent(agent_type_name).await? {
            let source = resolved.source.as_url().as_str().to_string();
            let Some(archive_path) = archive_paths_by_source.get(&source) else {
                continue;
            };

            let options = AgentFileOptions {
                target_path: AgentFilePath(resolved.target.path.clone()),
                permissions: resolved.target.permissions,
            };

            if let Some(existing) = archive_files.insert(archive_path.clone(), options.clone())
                && existing != options
            {
                return Err(anyhow!(
                    "Found conflicting archive mapping for source {} in agent manifest",
                    archive_path
                ));
            }
        }

        Ok(archive_files)
    }

    async fn resolve_archive_files_for_tool(
        &self,
        tool_name: &ToolName,
        archive_paths_by_source: &BTreeMap<String, ArchiveFilePath>,
    ) -> anyhow::Result<BTreeMap<ArchiveFilePath, AgentFileOptions>> {
        let mut archive_files = BTreeMap::new();
        for resolved in self.manifest_files_for_tool(tool_name).await? {
            let source = resolved.source.as_url().as_str().to_string();
            let Some(archive_path) = archive_paths_by_source.get(&source) else {
                continue;
            };
            let options = AgentFileOptions {
                target_path: AgentFilePath(resolved.target.path.clone()),
                permissions: resolved.target.permissions,
            };
            if let Some(existing) = archive_files.insert(archive_path.clone(), options.clone())
                && existing != options
            {
                return Err(anyhow!(
                    "Found conflicting archive mapping for source {} in tool {} manifest",
                    archive_path,
                    tool_name.as_str().log_color_highlight()
                ));
            }
        }
        Ok(archive_files)
    }

    fn files_to_add_or_update_for_agent(
        &self,
        agent_type_name: &AgentTypeName,
        files: BTreeMap<ArchiveFilePath, AgentFileOptions>,
    ) -> anyhow::Result<BTreeMap<ArchiveFilePath, AgentFileOptions>> {
        match &self.diff {
            ComponentDiff::All => Ok(files),
            ComponentDiff::Diff { diff } => {
                let Some(agent_change) = diff
                    .agent_type_provision_config_changes
                    .get(agent_type_name.0.as_str())
                else {
                    return Ok(BTreeMap::new());
                };

                match agent_change {
                    diff::BTreeMapDiffValue::Create => Ok(files),
                    diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::HashDiff { .. }) => {
                        Err(anyhow!(
                            "Cannot determine files to add or update for agent type {} from a hash-only provision config diff; component details were not loaded",
                            agent_type_name.0.log_color_highlight()
                        ))
                    }
                    diff::BTreeMapDiffValue::Delete => Ok(BTreeMap::new()),
                    diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff { diff }) => {
                        let changed_content_paths = content_changed_file_paths(diff);

                        if changed_content_paths.is_empty() {
                            Ok(BTreeMap::new())
                        } else {
                            Ok(files
                                .into_iter()
                                .filter(|(_, options)| {
                                    changed_content_paths.contains(options.target_path.as_abs_str())
                                })
                                .collect())
                        }
                    }
                }
            }
        }
    }

    fn files_to_add_or_update_for_tool(
        &self,
        tool_name: &ToolName,
        files: BTreeMap<ArchiveFilePath, AgentFileOptions>,
    ) -> anyhow::Result<BTreeMap<ArchiveFilePath, AgentFileOptions>> {
        match &self.diff {
            ComponentDiff::All => Ok(files),
            ComponentDiff::Diff { diff } => {
                let Some(tool_change) = diff.tool_deployment_config_changes.get(tool_name.as_str())
                else {
                    return Ok(BTreeMap::new());
                };
                match tool_change {
                    diff::BTreeMapDiffValue::Create => Ok(files),
                    diff::BTreeMapDiffValue::Delete => Ok(BTreeMap::new()),
                    diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::HashDiff { .. }) => {
                        Err(anyhow!(
                            "Cannot determine files to add or update for tool {} from a hash-only deployment config diff; component details were not loaded",
                            tool_name.as_str().log_color_highlight()
                        ))
                    }
                    diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff { diff }) => {
                        let changed_content_paths = content_changed_tool_file_paths(diff);
                        Ok(files
                            .into_iter()
                            .filter(|(_, options)| {
                                changed_content_paths.contains(options.target_path.as_abs_str())
                            })
                            .collect())
                    }
                }
            }
        }
    }

    pub async fn agent_type_provision_configs(
        &self,
        environment: &ResolvedEnvironmentIdentity,
        component_name: &ComponentName,
    ) -> anyhow::Result<BTreeMap<AgentTypeName, AgentTypeProvisionConfigCreation>> {
        let all_files = self.all_manifest_files().await?;
        let archive_paths_by_source =
            resolve_archive_paths_for_sources(all_files.iter().map(|f| f.source.as_url().clone()))?;
        let mut result = BTreeMap::new();
        for (agent_type_name, manifest_config) in
            &self.component_deploy_properties.agent_type_configs
        {
            let resolved_plugins = self.resolve_plugins_for(manifest_config)?;
            let initial_permission = self.normalize_initial_permission(
                environment,
                component_name,
                agent_type_name,
                manifest_config,
            )?;
            let mut creation = manifest_config
                .to_provision_config_creation(resolved_plugins, initial_permission)?;
            creation.files = self
                .resolve_archive_files_for_agent(agent_type_name, &archive_paths_by_source)
                .await?;
            result.insert(agent_type_name.clone(), creation);
        }

        Ok(result)
    }

    pub async fn agent_type_provision_config_updates(
        &self,
        environment: &ResolvedEnvironmentIdentity,
        component_name: &ComponentName,
        changed_files: &ChangedComponentFiles,
    ) -> anyhow::Result<Option<BTreeMap<AgentTypeName, AgentTypeProvisionConfigUpdate>>> {
        let changed = match self.diff.changed_agent_types() {
            None => {
                // All changed — return updates for all agent types
                let mut result = BTreeMap::new();
                for (name, manifest_config) in &self.component_deploy_properties.agent_type_configs
                {
                    let resolved_plugins = self.resolve_plugins_for(manifest_config)?;
                    let initial_permission = self.normalize_initial_permission(
                        environment,
                        component_name,
                        name,
                        manifest_config,
                    )?;
                    let mut creation = manifest_config
                        .to_provision_config_creation(resolved_plugins, initial_permission)?;
                    creation.files = self
                        .resolve_archive_files_for_agent(
                            name,
                            &changed_files.archive_paths_by_source,
                        )
                        .await?;
                    let files_to_remove = changed_files
                        .removed_per_agent
                        .get(name)
                        .cloned()
                        .unwrap_or_default();
                    let file_permission_updates = changed_files
                        .file_permission_updates_per_agent
                        .get(name)
                        .cloned()
                        .unwrap_or_default();
                    result.insert(
                        name.clone(),
                        AgentTypeProvisionConfigUpdate {
                            initial_permissions: Some(creation.initial_permissions),
                            env: Some(creation.env),
                            config: Some(creation.config),
                            files_to_add_or_update: self
                                .files_to_add_or_update_for_agent(name, creation.files)?,
                            files_to_remove,
                            file_permission_updates,
                            plugin_updates: creation
                                .plugin_installations
                                .into_iter()
                                .map(PluginInstallationAction::Install)
                                .collect(),
                        },
                    );
                }
                return Ok(Some(result));
            }
            Some(changed) if changed.is_empty() => return Ok(None),
            Some(changed) => changed,
        };

        // Only update agent types that changed
        let mut result = BTreeMap::new();
        for (name, manifest_config) in self
            .component_deploy_properties
            .agent_type_configs
            .iter()
            .filter(|(name, _)| changed.contains(name.0.as_str()))
        {
            let resolved_plugins = self.resolve_plugins_for(manifest_config)?;
            let initial_permission = self.normalize_initial_permission(
                environment,
                component_name,
                name,
                manifest_config,
            )?;
            let mut creation = manifest_config
                .to_provision_config_creation(resolved_plugins, initial_permission)?;
            creation.files = self
                .resolve_archive_files_for_agent(name, &changed_files.archive_paths_by_source)
                .await?;

            let plugin_updates =
                self.plugin_updates_for_agent(name, &creation.plugin_installations)?;

            let files_to_remove = changed_files
                .removed_per_agent
                .get(name)
                .cloned()
                .unwrap_or_default();
            let file_permission_updates = changed_files
                .file_permission_updates_per_agent
                .get(name)
                .cloned()
                .unwrap_or_default();
            result.insert(
                name.clone(),
                AgentTypeProvisionConfigUpdate {
                    initial_permissions: self
                        .initial_permission_update_for(name, creation.initial_permissions)?,
                    env: Some(creation.env),
                    config: Some(creation.config),
                    files_to_add_or_update: self
                        .files_to_add_or_update_for_agent(name, creation.files)?,
                    files_to_remove,
                    file_permission_updates,
                    plugin_updates,
                },
            );
        }

        Ok(Some(result))
    }

    fn normalize_initial_permission(
        &self,
        environment: &ResolvedEnvironmentIdentity,
        component_name: &ComponentName,
        agent_type_name: &AgentTypeName,
        manifest_config: &AgentTypeManifestProvisionConfig,
    ) -> anyhow::Result<AgentTypeInitialPermissions> {
        let context =
            initial_permission_recipient_context(environment, component_name, agent_type_name);
        Ok(manifest_config.to_initial_permission(&context))
    }

    fn initial_permission_update_for(
        &self,
        name: &AgentTypeName,
        initial_permissions: AgentTypeInitialPermissions,
    ) -> anyhow::Result<Option<AgentTypeInitialPermissions>> {
        match &self.diff {
            ComponentDiff::All => Ok(Some(initial_permissions)),
            ComponentDiff::Diff { diff } => {
                match diff
                    .agent_type_provision_config_changes
                    .get(name.0.as_str())
                {
                    Some(diff::BTreeMapDiffValue::Create) => Ok(Some(initial_permissions)),
                    Some(diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::HashDiff {
                        ..
                    })) => Err(anyhow!(
                        "Cannot determine initial permission update for agent type {} from a hash-only provision config diff; component details were not loaded",
                        name.0.log_color_highlight()
                    )),
                    Some(diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff {
                        diff,
                    })) if diff.initial_permission_changed => Ok(Some(initial_permissions)),
                    _ => Ok(None),
                }
            }
        }
    }

    fn agent_change(
        &self,
        agent_type_name: &AgentTypeName,
    ) -> Option<&diff::BTreeMapDiffValue<diff::DiffForHashOf<diff::AgentTypeProvisionConfig>>> {
        match &self.diff {
            ComponentDiff::All => None,
            ComponentDiff::Diff { diff } => diff
                .agent_type_provision_config_changes
                .get(agent_type_name.0.as_str()),
        }
    }

    fn tool_change(
        &self,
        tool_name: &ToolName,
    ) -> Option<&diff::BTreeMapDiffValue<diff::DiffForHashOf<diff::ToolDeploymentConfig>>> {
        match &self.diff {
            ComponentDiff::All => None,
            ComponentDiff::Diff { diff } => {
                diff.tool_deployment_config_changes.get(tool_name.as_str())
            }
        }
    }

    fn plugin_updates_for_tool_change(
        &self,
        tool_name: &ToolName,
        plugin_changes: &diff::BTreeMapDiff<uuid::Uuid, diff::PluginInstallation>,
        plugin_installations: &[PluginInstallation],
    ) -> anyhow::Result<Vec<PluginInstallationAction>> {
        let resolved_by_grant = plugin_installations
            .iter()
            .map(|plugin| (plugin.environment_plugin_grant_id.0, plugin))
            .collect::<HashMap<_, _>>();
        plugin_changes
            .iter()
            .map(|(grant_id, change)| match change {
                diff::BTreeMapDiffValue::Create => resolved_by_grant
                    .get(grant_id)
                    .map(|plugin| PluginInstallationAction::Install((*plugin).clone()))
                    .ok_or_else(|| {
                        anyhow!(
                            "Missing resolved plugin grant {} for tool {}",
                            grant_id,
                            tool_name.as_str().log_color_highlight()
                        )
                    }),
                diff::BTreeMapDiffValue::Delete => {
                    Ok(PluginInstallationAction::Uninstall(PluginUninstallation {
                        environment_plugin_grant_id: EnvironmentPluginGrantId(*grant_id),
                    }))
                }
                diff::BTreeMapDiffValue::Update(plugin_diff) => {
                    let plugin = resolved_by_grant.get(grant_id).ok_or_else(|| {
                        anyhow!(
                            "Missing resolved plugin grant {} for tool {}",
                            grant_id,
                            tool_name.as_str().log_color_highlight()
                        )
                    })?;
                    Ok(PluginInstallationAction::Update(PluginInstallationUpdate {
                        environment_plugin_grant_id: plugin.environment_plugin_grant_id,
                        new_priority: plugin_diff.priority_changed.then_some(plugin.priority),
                        new_parameters: plugin_diff
                            .parameters_changed
                            .then(|| plugin.parameters.clone()),
                    }))
                }
            })
            .collect()
    }

    fn plugin_updates_for_agent(
        &self,
        name: &AgentTypeName,
        plugin_installations: &[PluginInstallation],
    ) -> anyhow::Result<Vec<PluginInstallationAction>> {
        Self::plugin_updates_for_agent_change(name, self.agent_change(name), plugin_installations)
    }

    fn plugin_updates_for_agent_change(
        name: &AgentTypeName,
        agent_change: Option<
            &diff::BTreeMapDiffValue<diff::DiffForHashOf<diff::AgentTypeProvisionConfig>>,
        >,
        plugin_installations: &[PluginInstallation],
    ) -> anyhow::Result<Vec<PluginInstallationAction>> {
        let Some(agent_change) = agent_change else {
            return Ok(Vec::new());
        };

        match agent_change {
            diff::BTreeMapDiffValue::Create => Ok(plugin_installations
                .iter()
                .cloned()
                .map(PluginInstallationAction::Install)
                .collect()),
            diff::BTreeMapDiffValue::Delete => Ok(Vec::new()),
            diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::HashDiff { .. }) => Err(anyhow!(
                "Cannot determine plugin installation actions for agent type {} from a hash-only provision config diff; component details were not loaded",
                name.0.log_color_highlight()
            )),
            diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff { diff }) => {
                if diff.plugin_changes.is_empty() {
                    return Ok(Vec::new());
                }

                let resolved_by_grant: HashMap<uuid::Uuid, &PluginInstallation> =
                    plugin_installations
                        .iter()
                        .map(|p| (p.environment_plugin_grant_id.0, p))
                        .collect();
                Ok(diff
                    .plugin_changes
                    .iter()
                    .filter_map(|(grant_id, change)| match change {
                        diff::BTreeMapDiffValue::Create => resolved_by_grant
                            .get(grant_id)
                            .map(|&p| PluginInstallationAction::Install(p.clone())),
                        diff::BTreeMapDiffValue::Delete => {
                            Some(PluginInstallationAction::Uninstall(PluginUninstallation {
                                environment_plugin_grant_id: EnvironmentPluginGrantId(*grant_id),
                            }))
                        }
                        diff::BTreeMapDiffValue::Update(plugin_diff) => {
                            resolved_by_grant.get(grant_id).map(|&p| {
                                PluginInstallationAction::Update(PluginInstallationUpdate {
                                    environment_plugin_grant_id: p.environment_plugin_grant_id,
                                    new_priority: plugin_diff
                                        .priority_changed
                                        .then_some(p.priority),
                                    new_parameters: plugin_diff
                                        .parameters_changed
                                        .then_some(p.parameters.clone()),
                                })
                            })
                        }
                    })
                    .collect())
            }
        }
    }
}

fn content_changed_file_paths(diff: &AgentTypeProvisionConfigDiff) -> BTreeSet<String> {
    diff.file_changes
        .iter()
        .filter_map(|(path, change)| match change {
            diff::BTreeMapDiffValue::Create => Some(path.clone()),
            diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff { diff })
                if diff.content_changed =>
            {
                Some(path.clone())
            }
            _ => None,
        })
        .collect()
}

fn content_changed_tool_file_paths(diff: &diff::ToolDeploymentConfigDiff) -> BTreeSet<String> {
    diff.file_changes
        .iter()
        .filter_map(|(path, change)| match change {
            diff::BTreeMapDiffValue::Create => Some(path.clone()),
            diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff { diff })
                if diff.content_changed =>
            {
                Some(path.clone())
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::diff::Hash;
    use test_r::test;

    fn agent_id() -> AgentTypeName {
        AgentTypeName("Cart".to_string())
    }

    fn empty_agent_diff() -> AgentTypeProvisionConfigDiff {
        AgentTypeProvisionConfigDiff {
            env_changes: BTreeMap::new(),
            file_changes: BTreeMap::new(),
            plugin_changes: BTreeMap::new(),
            config_changes: BTreeMap::new(),
            initial_permission_changed: false,
        }
    }

    fn plugin_installation(grant_id: uuid::Uuid) -> PluginInstallation {
        PluginInstallation {
            environment_plugin_grant_id: EnvironmentPluginGrantId(grant_id),
            priority: PluginPriority(10),
            parameters: BTreeMap::from([("key".to_string(), "value".to_string())]),
        }
    }

    #[test]
    fn unchanged_wasm_omits_tool_replacement_and_config_updates() {
        let diff = ComponentDiff::new(Some(&diff::DiffForHashOf::ValueDiff {
            diff: diff::ComponentDiff {
                wasm_changed: false,
                agent_type_provision_config_changes: BTreeMap::new(),
                tool_deployment_config_changes: BTreeMap::new(),
            },
        }))
        .unwrap();

        assert!(!diff.wasm_changed());
        assert!(!diff.tools_changed());
    }

    #[test]
    fn binding_only_tool_change_does_not_replace_definitions() {
        let diff = ComponentDiff::new(Some(&diff::DiffForHashOf::ValueDiff {
            diff: diff::ComponentDiff {
                wasm_changed: false,
                agent_type_provision_config_changes: BTreeMap::new(),
                tool_deployment_config_changes: BTreeMap::from([(
                    "grep".to_string(),
                    diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff {
                        diff: diff::ToolDeploymentConfigDiff {
                            definition_changed: false,
                            config_changed: false,
                            env_changes: BTreeMap::new(),
                            file_changes: BTreeMap::new(),
                            plugin_changes: BTreeMap::new(),
                            environment_binding_changed: true,
                            agent_binding_changes: BTreeMap::new(),
                        },
                    }),
                )]),
            },
        }))
        .unwrap();

        assert!(diff.provision_config_changed());
        assert!(!diff.tools_changed());
        assert_eq!(
            diff.changed_tool_names().unwrap(),
            BTreeSet::from(["grep".to_string()])
        );
    }

    #[test]
    fn component_hash_diff_is_not_treated_as_all_changed() {
        let diff = diff::DiffForHashOf::<diff::Component>::HashDiff {
            new_hash: Hash::empty(),
            current_hash: Hash::new(blake3::hash(b"current")),
        };

        let result = ComponentDiff::new(Some(&diff));

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("hash-only diff"));
    }

    #[test]
    fn value_diff_without_plugin_changes_emits_no_plugin_actions() {
        let agent_id = agent_id();
        let agent_diff = empty_agent_diff();
        let agent_change =
            diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff { diff: agent_diff });
        let grant_id = uuid::Uuid::from_u128(1);
        let plugins = vec![plugin_installation(grant_id)];

        let updates = ComponentStager::plugin_updates_for_agent_change(
            &agent_id,
            Some(&agent_change),
            &plugins,
        )
        .unwrap();

        assert!(updates.is_empty());
    }

    #[test]
    fn create_provision_config_installs_manifest_plugins() {
        let agent_id = agent_id();
        let grant_id = uuid::Uuid::from_u128(1);
        let plugins = vec![plugin_installation(grant_id)];

        let updates = ComponentStager::plugin_updates_for_agent_change(
            &agent_id,
            Some(&diff::BTreeMapDiffValue::Create),
            &plugins,
        )
        .unwrap();

        assert_eq!(updates.len(), 1);
        assert!(matches!(
            &updates[0],
            PluginInstallationAction::Install(plugin)
                if plugin.environment_plugin_grant_id == EnvironmentPluginGrantId(grant_id)
        ));
    }

    #[test]
    fn plugin_hash_diff_error_names_plugin_action_context() {
        let agent_id = agent_id();
        let agent_change = diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::HashDiff {
            new_hash: Hash::empty(),
            current_hash: Hash::new(blake3::hash(b"current")),
        });

        let err =
            ComponentStager::plugin_updates_for_agent_change(&agent_id, Some(&agent_change), &[])
                .unwrap_err();

        assert!(
            err.to_string()
                .contains("Cannot determine plugin installation actions")
        );
        assert!(err.to_string().contains("Cart"));
    }

    #[test]
    fn plugin_diff_emits_targeted_actions() {
        let agent_id = agent_id();
        let install_grant_id = uuid::Uuid::from_u128(1);
        let uninstall_grant_id = uuid::Uuid::from_u128(2);
        let update_grant_id = uuid::Uuid::from_u128(3);

        let mut agent_diff = empty_agent_diff();
        agent_diff
            .plugin_changes
            .insert(install_grant_id, diff::BTreeMapDiffValue::Create);
        agent_diff
            .plugin_changes
            .insert(uninstall_grant_id, diff::BTreeMapDiffValue::Delete);
        agent_diff.plugin_changes.insert(
            update_grant_id,
            diff::BTreeMapDiffValue::Update(diff::PluginInstallationDiff {
                priority_changed: true,
                parameters_changed: true,
            }),
        );
        let agent_change =
            diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff { diff: agent_diff });
        let plugins = vec![
            plugin_installation(install_grant_id),
            plugin_installation(update_grant_id),
        ];

        let updates = ComponentStager::plugin_updates_for_agent_change(
            &agent_id,
            Some(&agent_change),
            &plugins,
        )
        .unwrap();

        assert_eq!(updates.len(), 3);
        assert!(matches!(
            &updates[0],
            PluginInstallationAction::Install(plugin)
                if plugin.environment_plugin_grant_id == EnvironmentPluginGrantId(install_grant_id)
        ));
        assert!(matches!(
            &updates[1],
            PluginInstallationAction::Uninstall(plugin)
                if plugin.environment_plugin_grant_id == EnvironmentPluginGrantId(uninstall_grant_id)
        ));
        assert!(matches!(
            &updates[2],
            PluginInstallationAction::Update(plugin)
                if plugin.environment_plugin_grant_id == EnvironmentPluginGrantId(update_grant_id)
                    && plugin.new_priority == Some(PluginPriority(10))
                    && plugin.new_parameters == Some(BTreeMap::from([("key".to_string(), "value".to_string())]))
        ));
    }

    #[test]
    fn content_changed_file_paths_ignores_permission_only_updates_and_deletes() {
        let mut agent_diff = empty_agent_diff();
        agent_diff
            .file_changes
            .insert("/created.txt".to_string(), diff::BTreeMapDiffValue::Create);
        agent_diff.file_changes.insert(
            "/content.txt".to_string(),
            diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff {
                diff: AgentFileDiff {
                    content_changed: true,
                    permissions_changed: false,
                },
            }),
        );
        agent_diff.file_changes.insert(
            "/permissions.txt".to_string(),
            diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff {
                diff: AgentFileDiff {
                    content_changed: false,
                    permissions_changed: true,
                },
            }),
        );
        agent_diff
            .file_changes
            .insert("/deleted.txt".to_string(), diff::BTreeMapDiffValue::Delete);

        let paths = content_changed_file_paths(&agent_diff);

        assert_eq!(
            paths,
            BTreeSet::from(["/content.txt".to_string(), "/created.txt".to_string()])
        );
    }

    #[test]
    fn content_changed_tool_file_paths_ignores_permission_only_updates_and_deletes() {
        let mut tool_diff = diff::ToolDeploymentConfigDiff {
            definition_changed: false,
            config_changed: false,
            env_changes: BTreeMap::new(),
            file_changes: BTreeMap::new(),
            plugin_changes: BTreeMap::new(),
            environment_binding_changed: false,
            agent_binding_changes: BTreeMap::new(),
        };
        tool_diff
            .file_changes
            .insert("/created.txt".to_string(), diff::BTreeMapDiffValue::Create);
        tool_diff.file_changes.insert(
            "/content.txt".to_string(),
            diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff {
                diff: AgentFileDiff {
                    content_changed: true,
                    permissions_changed: false,
                },
            }),
        );
        tool_diff.file_changes.insert(
            "/permissions.txt".to_string(),
            diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff {
                diff: AgentFileDiff {
                    content_changed: false,
                    permissions_changed: true,
                },
            }),
        );
        tool_diff
            .file_changes
            .insert("/deleted.txt".to_string(), diff::BTreeMapDiffValue::Delete);

        assert_eq!(
            content_changed_tool_file_paths(&tool_diff),
            BTreeSet::from(["/content.txt".to_string(), "/created.txt".to_string()])
        );
    }
}
