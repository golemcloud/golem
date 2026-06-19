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
use crate::model::component::{AgentTypeManifestProvisionConfig, ComponentDeployProperties};
use crate::model::text::plugin::PluginNameAndVersion;
use anyhow::{Context as AnyhowContext, anyhow};
use golem_client::model::EnvironmentPluginGrantWithDetails;
use golem_common::model::agent::AgentTypeName;
use golem_common::schema::agent::AgentTypeSchema;
use golem_common::model::component::{
    AgentFileOptions, AgentFilePath, AgentFilePermissions, AgentTypeProvisionConfigCreation,
    AgentTypeProvisionConfigUpdate, ArchiveFilePath, PluginInstallation, PluginInstallationAction,
    PluginInstallationUpdate, PluginPriority, PluginUninstallation,
};
use golem_common::model::diff::{self, AgentFileDiff, AgentTypeProvisionConfigDiff};
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
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

    pub fn provision_config_changed(&self) -> bool {
        match self {
            ComponentDiff::All => true,
            ComponentDiff::Diff { diff } => !diff.agent_type_provision_config_changes.is_empty(),
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
}

pub struct ChangedComponentFiles {
    pub new_or_updated_content: Option<ComponentFilesArchive>,
    pub removed_per_agent: BTreeMap<AgentTypeName, Vec<AgentFilePath>>,
    pub archive_paths_by_source: BTreeMap<String, ArchiveFilePath>,
    /// Files whose only change is permissions — no content re-upload needed.
    pub file_permission_updates_per_agent:
        BTreeMap<AgentTypeName, BTreeMap<AgentFilePath, AgentFilePermissions>>,
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
}

impl<'a> ComponentStager<'a> {
    pub fn new(
        ctx: Arc<Context>,
        component_deploy_properties: &'a ComponentDeployProperties,
        plugin_grants: HashMap<PluginNameAndVersion, EnvironmentPluginGrantWithDetails>,
        // NOTE: none means ALL changed (e.g. new component)
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
            manifest_files_by_agent: OnceCell::new(),
        }
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

    async fn all_manifest_files(&self) -> anyhow::Result<Vec<InitialComponentFile>> {
        Ok(self
            .manifest_files_by_agent()
            .await?
            .values()
            .flatten()
            .cloned()
            .collect())
    }

    async fn changed_manifest_files(&self) -> anyhow::Result<Vec<InitialComponentFile>> {
        match self.diff.changed_agent_types() {
            None => self.all_manifest_files().await, // all changed (new component or hash-diff)
            Some(changed) if changed.is_empty() => Ok(Vec::new()),
            Some(changed) => {
                // Only include files that have content changes — skip permissions-only
                let content_changed_paths = self.content_changed_file_paths();
                let mut result = Vec::new();
                for (agent_type_name, _) in self
                    .component_deploy_properties
                    .agent_type_configs
                    .iter()
                    .filter(|(name, _)| changed.contains(name.0.as_str()))
                {
                    result.extend(
                        self.manifest_files_for_agent(agent_type_name)
                            .await?
                            .into_iter()
                            .filter(|file| {
                                // If we have fine-grained diff, skip permissions-only files.
                                content_changed_paths.is_empty()
                                    || content_changed_paths.contains(file.target.path.as_abs_str())
                            }),
                    );
                }

                Ok(result)
            }
        }
    }

    fn content_changed_file_paths(&self) -> BTreeSet<String> {
        self.diff
            .file_changes_per_agent()
            .into_iter()
            .flat_map(|(_, agent_diff)| {
                agent_diff
                    .file_changes
                    .iter()
                    .filter_map(|(path, change)| match change {
                        diff::BTreeMapDiffValue::Create => Some(path.clone()),
                        diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff {
                            diff,
                        }) if diff.content_changed => Some(path.clone()),
                        _ => None,
                    })
            })
            .collect()
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

        // Compute permissions-only updates per agent type
        let mut file_permission_updates_per_agent = BTreeMap::new();
        for (agent_type_str, agent_diff) in self.diff.file_changes_per_agent() {
            let agent_name = golem_common::model::agent::AgentTypeName(agent_type_str.to_string());
            let manifest_files = match self
                .component_deploy_properties
                .agent_type_configs
                .get(&agent_name)
            {
                Some(_) => self.manifest_files_for_agent(&agent_name).await?,
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
                file_permission_updates_per_agent.insert(agent_name, perm_updates);
            }
        }

        Ok(ChangedComponentFiles {
            new_or_updated_content,
            removed_per_agent,
            archive_paths_by_source,
            file_permission_updates_per_agent,
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

    fn resolve_plugins_for(
        &self,
        manifest_config: &AgentTypeManifestProvisionConfig,
    ) -> anyhow::Result<Vec<PluginInstallation>> {
        manifest_config
            .plugins
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

    fn files_to_add_or_update_for_agent(
        &self,
        agent_type_name: &AgentTypeName,
        files: BTreeMap<ArchiveFilePath, AgentFileOptions>,
    ) -> BTreeMap<ArchiveFilePath, AgentFileOptions> {
        match &self.diff {
            ComponentDiff::All => files,
            ComponentDiff::Diff { diff } => {
                let Some(agent_change) = diff
                    .agent_type_provision_config_changes
                    .get(agent_type_name.0.as_str())
                else {
                    return BTreeMap::new();
                };

                match agent_change {
                    diff::BTreeMapDiffValue::Create
                    | diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::HashDiff { .. }) => {
                        files
                    }
                    diff::BTreeMapDiffValue::Delete => BTreeMap::new(),
                    diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff { diff }) => {
                        let changed_content_paths: BTreeSet<&str> = diff
                            .file_changes
                            .iter()
                            .filter_map(|(path, change)| match change {
                                diff::BTreeMapDiffValue::Create => Some(path.as_str()),
                                diff::BTreeMapDiffValue::Update(
                                    diff::DiffForHashOf::ValueDiff { diff },
                                ) if diff.content_changed => Some(path.as_str()),
                                _ => None,
                            })
                            .collect();

                        if changed_content_paths.is_empty() {
                            BTreeMap::new()
                        } else {
                            files
                                .into_iter()
                                .filter(|(_, options)| {
                                    changed_content_paths.contains(options.target_path.as_abs_str())
                                })
                                .collect()
                        }
                    }
                }
            }
        }
    }

    pub async fn agent_type_provision_configs(
        &self,
    ) -> anyhow::Result<BTreeMap<AgentTypeName, AgentTypeProvisionConfigCreation>> {
        let all_files = self.all_manifest_files().await?;
        let archive_paths_by_source =
            resolve_archive_paths_for_sources(all_files.iter().map(|f| f.source.as_url().clone()))?;
        let mut result = BTreeMap::new();
        for (agent_type_name, manifest_config) in
            &self.component_deploy_properties.agent_type_configs
        {
            let resolved_plugins = self.resolve_plugins_for(manifest_config)?;
            let mut creation = manifest_config.to_provision_config_creation(resolved_plugins);
            creation.files = self
                .resolve_archive_files_for_agent(agent_type_name, &archive_paths_by_source)
                .await?;
            result.insert(agent_type_name.clone(), creation);
        }

        Ok(result)
    }

    pub async fn agent_type_provision_config_updates(
        &self,
        changed_files: &ChangedComponentFiles,
    ) -> anyhow::Result<Option<BTreeMap<AgentTypeName, AgentTypeProvisionConfigUpdate>>> {
        let changed = match self.diff.changed_agent_types() {
            None => {
                // All changed — return updates for all agent types
                let mut result = BTreeMap::new();
                for (name, manifest_config) in &self.component_deploy_properties.agent_type_configs
                {
                    let resolved_plugins = self.resolve_plugins_for(manifest_config)?;
                    let mut creation =
                        manifest_config.to_provision_config_creation(resolved_plugins);
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
                            env: Some(creation.env),
                            config: Some(creation.config),
                            files_to_add_or_update: self
                                .files_to_add_or_update_for_agent(name, creation.files),
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
            let mut creation = manifest_config.to_provision_config_creation(resolved_plugins);
            creation.files = self
                .resolve_archive_files_for_agent(name, &changed_files.archive_paths_by_source)
                .await?;

            let plugin_updates: Vec<PluginInstallationAction> = match &self.diff {
                ComponentDiff::All => creation
                    .plugin_installations
                    .into_iter()
                    .map(PluginInstallationAction::Install)
                    .collect(),
                ComponentDiff::Diff { diff } => match diff
                    .agent_type_provision_config_changes
                    .get(name.0.as_str())
                {
                    Some(diff::BTreeMapDiffValue::Update(diff::DiffForHashOf::ValueDiff {
                        diff,
                    })) if !diff.plugin_changes.is_empty() => {
                        let resolved_by_grant: HashMap<uuid::Uuid, &PluginInstallation> = creation
                            .plugin_installations
                            .iter()
                            .map(|p| (p.environment_plugin_grant_id.0, p))
                            .collect();
                        diff.plugin_changes
                            .iter()
                            .filter_map(|(grant_id, change)| match change {
                                diff::BTreeMapDiffValue::Create => resolved_by_grant
                                    .get(grant_id)
                                    .map(|&p| PluginInstallationAction::Install(p.clone())),
                                diff::BTreeMapDiffValue::Delete => Some(
                                    PluginInstallationAction::Uninstall(PluginUninstallation {
                                        environment_plugin_grant_id: EnvironmentPluginGrantId(
                                            *grant_id,
                                        ),
                                    }),
                                ),
                                diff::BTreeMapDiffValue::Update(plugin_diff) => {
                                    resolved_by_grant.get(grant_id).map(|&p| {
                                        PluginInstallationAction::Update(PluginInstallationUpdate {
                                            environment_plugin_grant_id: p
                                                .environment_plugin_grant_id,
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
                            .collect()
                    }
                    _ => creation
                        .plugin_installations
                        .into_iter()
                        .map(PluginInstallationAction::Install)
                        .collect(),
                },
            };

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
                    env: Some(creation.env),
                    config: Some(creation.config),
                    files_to_add_or_update: self
                        .files_to_add_or_update_for_agent(name, creation.files),
                    files_to_remove,
                    file_permission_updates,
                    plugin_updates,
                },
            );
        }

        Ok(Some(result))
    }
}
