// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source Available License v1.1 (the "License");
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

use super::{ComponentError, environment_from_component_record};
use crate::metrics::storage::record_component_uploaded;
use crate::repo::component::ComponentRepo;
use crate::repo::model::component::{ComponentRepoError, ComponentRevisionRecord};
use crate::services::account_usage::AccountUsageService;
use crate::services::card::CardService;
use crate::services::component::utils::prepare_component_files_for_upload;
use crate::services::component_compilation::ComponentCompilationService;
use crate::services::component_object_store::ComponentObjectStore;
use crate::services::environment::EnvironmentError;
use crate::services::environment::EnvironmentService;
use crate::services::environment_plugin_grant::{
    EnvironmentPluginGrantError, EnvironmentPluginGrantService,
};
use crate::services::run_cpu_bound_work;
use anyhow::Context;
use golem_common::base_model::component_metadata::{
    AgentInitialPermissionTemplate, AgentTypeProvisionConfig,
};
use golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantWithDetails;
use golem_common::model::agent::AgentConfigSource;
use golem_common::model::agent::{AgentFileContentHash, AgentTypeName};
use golem_common::model::card::owner::ComponentOwnerPattern;
use golem_common::model::card::{
    ClassPermissionTarget, ComponentResourcePattern, ComponentVerb, PermissionTarget,
};
use golem_common::model::component::{
    AgentFilePath, ArchiveFilePath, ComponentCreation, ComponentId, ComponentName,
    ComponentRevision, ComponentUpdate, InitialAgentFile, InstalledPlugin, PluginInstallation,
    PluginInstallationAction,
};
use golem_common::model::component::{
    AgentTypeProvisionConfigCreation, AgentTypeProvisionConfigUpdate,
};
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::diff::Hash;
use golem_common::model::environment::{Environment, EnvironmentId, EnvironmentName};
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::model::worker::AgentConfigEntryDto;
use golem_common::model::worker::TypedAgentConfigEntry;
use golem_common::schema::SchemaValue;
use golem_common::schema::agent::{AgentTypeSchema, typed_schema_value_with_projected_defs};
use golem_common::schema::render::from_json_value;
use golem_common::schema::validation::{is_equivalent_cross_graph, validate_value};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::model::component::Component;
use golem_service_base::replayable_stream::ReplayableStream;
use golem_service_base::service::initial_agent_files::InitialAgentFilesService;
use itertools::Itertools;
use std::collections::HashSet;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tempfile::NamedTempFile;
use tracing::info;

pub struct ComponentWriteService {
    component_repo: Arc<dyn ComponentRepo>,
    card_service: Arc<CardService>,
    object_store: Arc<ComponentObjectStore>,
    component_compilation: Arc<dyn ComponentCompilationService>,
    initial_agent_files_service: Arc<InitialAgentFilesService>,
    account_usage_service: Arc<AccountUsageService>,
    environment_service: Arc<EnvironmentService>,
    environment_plugin_grant_service: Arc<EnvironmentPluginGrantService>,
}

impl ComponentWriteService {
    pub fn new(
        component_repo: Arc<dyn ComponentRepo>,
        card_service: Arc<CardService>,
        object_store: Arc<ComponentObjectStore>,
        component_compilation: Arc<dyn ComponentCompilationService>,
        initial_agent_files_service: Arc<InitialAgentFilesService>,
        account_usage_service: Arc<AccountUsageService>,
        environment_service: Arc<EnvironmentService>,
        environment_plugin_grant_service: Arc<EnvironmentPluginGrantService>,
    ) -> Self {
        Self {
            component_repo,
            card_service,
            object_store,
            component_compilation,
            initial_agent_files_service,
            account_usage_service,
            environment_service,
            environment_plugin_grant_service,
        }
    }

    async fn create_default_initial_permissions(
        &self,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        agent_types: &[AgentType],
        environment_name: &EnvironmentName,
        component_name: &ComponentName,
    ) -> Result<BTreeMap<AgentTypeName, AgentInitialPermissionTemplate>, ComponentError> {
        let mut result = BTreeMap::new();

        for agent_type in agent_types {
            let card_id = self
                .card_service
                .create_agent_initial_card(
                    component_id,
                    component_revision,
                    agent_type.type_name.clone(),
                )
                .await?;
            let template = AgentInitialPermissionTemplate {
                card_id,
                ..AgentInitialPermissionTemplate::default_for(environment_name, component_name)
            };

            result.insert(agent_type.type_name.clone(), template);
        }

        Ok(result)
    }

    pub async fn create(
        &self,
        environment_id: EnvironmentId,
        component_creation: ComponentCreation,
        wasm: Vec<u8>,
        files_archive: Option<NamedTempFile>,
        auth: &AuthCtx,
    ) -> Result<Component, ComponentError> {
        info!(environment_id = %environment_id, "Create component");

        let wasm: Arc<[u8]> = Arc::from(wasm);

        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    ComponentError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        authorize_component_permission(
            auth,
            &environment,
            &component_creation.component_name,
            ComponentVerb::Create,
        )?;

        // Fast path check to avoid processing the component if we are going to reject it anyway
        self.component_repo
            .get_staged_by_name(environment_id.0, &component_creation.component_name.0)
            .await?
            .map_or(Ok(()), |_| {
                Err(ComponentError::ComponentWithNameAlreadyExists(
                    component_creation.component_name.clone(),
                ))
            })?;

        self.account_usage_service
            .ensure_new_component_within_limits(
                environment.owner_account_id,
                u64::try_from(wasm.len()).unwrap(),
            )
            .await?;

        let referenced_paths: HashSet<ArchiveFilePath> = component_creation
            .agent_type_provision_configs
            .values()
            .flat_map(|c| c.files.keys().cloned())
            .collect();
        let uploaded_files = match files_archive {
            Some(archive) => {
                self.upload_agent_files(environment_id, archive, &referenced_paths)
                    .await?
            }
            None => HashMap::new(),
        };

        let component_id = ComponentId::new();
        let wasm_bytes = wasm.len() as u64;
        let (wasm_hash, wasm_object_store_key) = self
            .upload_and_hash_component_wasm(environment_id, wasm.clone())
            .await?;
        record_component_uploaded(
            &auth.actor_account_id().to_string(),
            &environment_id.to_string(),
            wasm_bytes,
        );

        // Batch-resolve all plugin grants referenced across all agent types in one pass,
        // so the same grant is only fetched once even if shared by multiple agent types.
        let all_grant_ids: HashSet<EnvironmentPluginGrantId> = component_creation
            .agent_type_provision_configs
            .values()
            .flat_map(|c| {
                c.plugin_installations
                    .iter()
                    .map(|p| p.environment_plugin_grant_id)
            })
            .collect();
        let resolved_grants = self
            .resolve_all_plugin_grants(&environment, all_grant_ids, auth)
            .await?;

        let mut provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig> =
            BTreeMap::new();

        for (agent_type_name, creation) in &component_creation.agent_type_provision_configs {
            let agent_type = component_creation
                .agent_types
                .iter()
                .find(|t| &t.type_name == agent_type_name)
                .ok_or_else(|| {
                    ComponentError::UndeclaredAgentTypeInProvisionConfig(agent_type_name.clone())
                })?;

            let files = resolve_files_for_creation(agent_type_name, creation, &uploaded_files)?;
            let plugins =
                resolve_plugins_for_creation(&creation.plugin_installations, &resolved_grants)?;
            let config =
                validate_and_transform_config_entries(agent_type, creation.config.clone())?;

            provision_configs.insert(
                agent_type_name.clone(),
                AgentTypeProvisionConfig {
                    env: creation.env.clone(),
                    config,
                    plugins,
                    files,
                },
            );
        }

        let component_metadata = analyze_and_validate_component_wasm(
            component_creation.agent_types,
            wasm.clone(),
            provision_configs,
        )
        .await?;
        let component_metadata = component_metadata.with_agent_initial_permissions(
            self.create_default_initial_permissions(
                component_id,
                ComponentRevision::INITIAL,
                component_metadata.agent_types(),
                &environment.name,
                &component_creation.component_name,
            )
            .await?,
        );
        validate_component_metadata_invariants(&component_metadata)?;

        let component_size = wasm.len() as u64;

        let record = ComponentRevisionRecord::creation(
            component_id,
            component_size,
            component_metadata,
            wasm_hash,
            wasm_object_store_key,
            auth.actor_account_id(),
        );

        let stored_component: Component = self
            .component_repo
            .create(
                environment_id.0,
                &component_creation.component_name.0,
                record,
            )
            .await
            .map_err(|err| match err {
                ComponentRepoError::ConcurrentModification
                | ComponentRepoError::VersionAlreadyExists { .. } => {
                    ComponentError::ConcurrentUpdate
                }
                ComponentRepoError::ComponentViolatesUniqueness => {
                    ComponentError::ComponentWithNameAlreadyExists(
                        component_creation.component_name,
                    )
                }
                other => other.into(),
            })?
            .try_into_model(
                environment.application_id,
                environment.owner_account_id,
                environment.owner_account_email.clone(),
                environment.application_name.clone(),
                environment.name.clone(),
            )?;

        self.component_compilation
            .enqueue_compilation(environment_id, component_id, stored_component.revision)
            .await;

        Ok(stored_component)
    }

    pub async fn update(
        &self,
        component_id: ComponentId,
        component_update: ComponentUpdate,
        new_wasm: Option<Vec<u8>>,
        new_files_archive: Option<NamedTempFile>,
        auth: &AuthCtx,
    ) -> Result<Component, ComponentError> {
        let new_wasm: Option<Arc<[u8]>> = new_wasm.map(Arc::from);

        let component_record = self
            .component_repo
            .get_staged_by_id(component_id.0)
            .await?
            .ok_or(ComponentError::ComponentNotFound(component_id))?;

        let environment = environment_from_component_record(&component_record)?;

        let component_name = ComponentName(component_record.component.name.clone());

        authorize_component_permission(auth, &environment, &component_name, ComponentVerb::View)
            .map_err(|_| ComponentError::ComponentNotFound(component_id))?;
        authorize_component_permission(auth, &environment, &component_name, ComponentVerb::Update)?;

        if component_update.allow_incompatible_config && environment.compatibility_check {
            return Err(ComponentError::ResetOverrideRequiresCompatibilityCheckDisabled);
        }

        let mut component = component_record.try_into_model()?;

        if component_update.current_revision != component.revision {
            Err(ComponentError::ConcurrentUpdate)?
        };

        component.revision = component.revision.next()?;

        let environment_id = component.environment_id;
        let component_id = component.id;

        info!(environment_id = %environment_id, "Update component");

        let agent_types_changed = component_update.agent_types.is_some();
        let allow_incompatible_config = component_update.allow_incompatible_config;

        // When no agent type update is supplied, fall back to the schema-native
        // agent types already stored on the existing component metadata.
        let agent_types = match component_update.agent_types {
            Some(agent_types) => agent_types,
            None => component.metadata.agent_types().to_vec(),
        };

        let mut final_provision_configs = component.metadata.agent_type_provision_configs().clone();
        if agent_types_changed {
            final_provision_configs =
                provision_configs_for_agent_types(&agent_types, final_provision_configs);
        }

        let mut provision_configs_changed = false;

        if let Some(updates) = component_update.agent_type_provision_config_updates {
            provision_configs_changed = true;

            let referenced_paths: HashSet<ArchiveFilePath> = updates
                .values()
                .flat_map(|u| u.files_to_add_or_update.keys().cloned())
                .collect();
            let uploaded_files = match new_files_archive {
                Some(archive) => {
                    self.upload_agent_files(environment_id, archive, &referenced_paths)
                        .await?
                }
                None => HashMap::new(),
            };

            for (agent_type_name, update) in updates {
                let agent_type = agent_types
                    .iter()
                    .find(|agent_type| agent_type.type_name == agent_type_name)
                    .ok_or_else(|| {
                        ComponentError::UndeclaredAgentTypeInProvisionConfig(
                            agent_type_name.clone(),
                        )
                    })?;

                let existing = final_provision_configs
                    .get(&agent_type_name)
                    .cloned()
                    .unwrap_or_default();

                let updated = self
                    .apply_provision_config_update(
                        &agent_type_name,
                        existing,
                        update,
                        &uploaded_files,
                        agent_type,
                        &environment,
                        auth,
                    )
                    .await?;

                final_provision_configs.insert(agent_type_name, updated);
            }
        }

        if agent_types_changed && !allow_incompatible_config {
            for (agent_type_name, config) in &final_provision_configs {
                let agent_type = agent_types
                    .iter()
                    .find(|agent_type| &agent_type.type_name == agent_type_name)
                    .ok_or_else(|| {
                        ComponentError::UndeclaredAgentTypeInProvisionConfig(
                            agent_type_name.clone(),
                        )
                    })?;
                check_config_entries_match(agent_type, &config.config)?;
            }
        }

        if let Some(new_wasm) = new_wasm {
            self.account_usage_service
                .ensure_updated_component_within_limits(
                    environment.owner_account_id,
                    u64::try_from(new_wasm.len()).unwrap(),
                    component.component_size,
                )
                .await?;

            let new_wasm_bytes = new_wasm.len() as u64;
            let (wasm_hash, wasm_object_store_key) = self
                .upload_and_hash_component_wasm(environment_id, new_wasm.clone())
                .await?;
            record_component_uploaded(
                &auth.actor_account_id().to_string(),
                &environment_id.to_string(),
                new_wasm_bytes,
            );

            component.wasm_hash = wasm_hash;
            component.object_store_key = wasm_object_store_key;
            let metadata = analyze_and_validate_component_wasm(
                agent_types,
                new_wasm.clone(),
                final_provision_configs,
            )
            .await?;
            component.metadata = metadata.with_agent_initial_permissions(
                self.create_default_initial_permissions(
                    component.id,
                    component.revision,
                    metadata.agent_types(),
                    &environment.name,
                    &component.component_name,
                )
                .await?,
            );
        } else if agent_types_changed {
            // TODO: skip the download here
            let old_data = self
                .object_store
                .get(environment_id, &component.object_store_key)
                .await?;

            let metadata = analyze_and_validate_component_wasm(
                agent_types,
                Arc::from(old_data),
                final_provision_configs,
            )
            .await?;
            component.metadata = metadata.with_agent_initial_permissions(
                self.create_default_initial_permissions(
                    component.id,
                    component.revision,
                    metadata.agent_types(),
                    &environment.name,
                    &component_name,
                )
                .await?,
            );
        } else if provision_configs_changed {
            component.metadata = component
                .metadata
                .with_provision_configs(final_provision_configs);
        }

        validate_component_metadata_invariants(&component.metadata)?;

        let record = ComponentRevisionRecord::from_model(component, auth.actor_account_id());

        let stored_component: Component = self
            .component_repo
            .update(record)
            .await
            .map_err(|err| match err {
                ComponentRepoError::ConcurrentModification => ComponentError::ConcurrentUpdate,
                ComponentRepoError::VersionAlreadyExists { version } => {
                    ComponentError::ComponentVersionAlreadyExists(version)
                }
                other => other.into(),
            })?
            .try_into_model(
                environment.application_id,
                environment.owner_account_id,
                environment.owner_account_email.clone(),
                environment.application_name.clone(),
                environment.name.clone(),
            )?;

        self.component_compilation
            .enqueue_compilation(environment_id, component_id, stored_component.revision)
            .await;

        Ok(stored_component)
    }

    pub async fn delete(
        &self,
        component_id: ComponentId,
        current_revision: ComponentRevision,
        auth: &AuthCtx,
    ) -> Result<(), ComponentError> {
        let component_record = self
            .component_repo
            .get_staged_by_id(component_id.0)
            .await?
            .ok_or(ComponentError::ComponentNotFound(component_id))?;

        let environment = environment_from_component_record(&component_record)?;

        let component_name = ComponentName(component_record.component.name.clone());
        authorize_component_permission(auth, &environment, &component_name, ComponentVerb::View)
            .map_err(|_| ComponentError::ComponentNotFound(component_id))?;
        authorize_component_permission(auth, &environment, &component_name, ComponentVerb::Delete)?;

        let component = component_record.try_into_model()?;

        if current_revision != component.revision {
            Err(ComponentError::ConcurrentUpdate)?
        };

        self.component_repo
            .delete(
                auth.actor_account_id().0,
                component_id.0,
                current_revision.next()?.into(),
            )
            .await
            .map_err(|err| match err {
                ComponentRepoError::ConcurrentModification => ComponentError::ConcurrentUpdate,
                other => other.into(),
            })?;

        Ok(())
    }

    async fn upload_and_hash_component_wasm(
        &self,
        environment_id: EnvironmentId,
        data: Arc<[u8]>,
    ) -> Result<(Hash, String), ComponentError> {
        // TODO: use something like PluginWasmFilesService instead of raw object store
        let hash = self.object_store.put(environment_id, data).await?;
        Ok((hash, hash.to_string()))
    }

    async fn upload_agent_files(
        &self,
        environment_id: EnvironmentId,
        archive: NamedTempFile,
        referenced_paths: &HashSet<ArchiveFilePath>,
    ) -> Result<HashMap<ArchiveFilePath, (AgentFileContentHash, u64)>, ComponentError> {
        let to_upload = prepare_component_files_for_upload(archive)
            .await?
            .into_iter()
            .filter(|(path, _)| referenced_paths.contains(path))
            .collect::<Vec<_>>();

        let tasks = to_upload.into_iter().map(|(path, stream)| async move {
            info!("Uploading file: {}", path.to_string());

            let size = stream
                .length()
                .await
                .context("Failed to get component file size")?;

            let key = self
                .initial_agent_files_service
                .put_if_not_exists(environment_id, &stream)
                .await
                .context("Failed to upload component files")?;

            Ok::<_, ComponentError>((path, (key, size)))
        });

        let uploaded = futures::future::try_join_all(tasks).await?;

        Ok(HashMap::from_iter(uploaded))
    }

    /// Resolves all plugin grants in a single DB query.
    /// Deduplicates by grant ID so the same grant is fetched at most once,
    /// even if referenced by multiple agent types.
    async fn resolve_all_plugin_grants(
        &self,
        environment: &Environment,
        grant_ids: impl IntoIterator<Item = EnvironmentPluginGrantId>,
        auth: &AuthCtx,
    ) -> Result<HashMap<EnvironmentPluginGrantId, EnvironmentPluginGrantWithDetails>, ComponentError>
    {
        self.environment_plugin_grant_service
            .get_active_by_ids_for_environment(grant_ids, environment, auth)
            .await
            .map_err(|err| match err {
                EnvironmentPluginGrantError::EnvironmentPluginGrantNotFound(id) => {
                    ComponentError::EnvironmentPluginNotFound(id)
                }
                other => other.into(),
            })
    }

    async fn update_plugin_installations(
        &self,
        environment: &Environment,
        previous: Vec<InstalledPlugin>,
        updates: Vec<PluginInstallationAction>,
        auth: &AuthCtx,
    ) -> Result<Vec<InstalledPlugin>, ComponentError> {
        let mut updated = previous;

        for update in updates {
            match update {
                PluginInstallationAction::Uninstall(inner) => {
                    let plugin_index = updated
                        .iter()
                        .position(|p| {
                            p.environment_plugin_grant_id == inner.environment_plugin_grant_id
                        })
                        .ok_or(ComponentError::PluginInstallationNotFound(
                            inner.environment_plugin_grant_id,
                        ))?;

                    updated.swap_remove(plugin_index);
                }
                PluginInstallationAction::Update(inner) => {
                    let plugin_index = updated
                        .iter()
                        .position(|p| {
                            p.environment_plugin_grant_id == inner.environment_plugin_grant_id
                        })
                        .ok_or(ComponentError::PluginInstallationNotFound(
                            inner.environment_plugin_grant_id,
                        ))?;

                    // Currently it's ok to update a plugin even if it was removed from the enviroment / deleted.
                    // Fetch the environment_grant_here if you want to restrict that.

                    if let Some(new_priority) = inner.new_priority {
                        // ensure the plugin priority is not already used
                        if updated.iter().any(|p| p.priority == new_priority) {
                            return Err(ComponentError::ConflictingPluginPriority(new_priority));
                        };
                    };

                    let plugin = updated.get_mut(plugin_index).unwrap();

                    if let Some(new_priority) = inner.new_priority {
                        plugin.priority = new_priority;
                    };

                    if let Some(new_parameters) = inner.new_parameters {
                        plugin.parameters = new_parameters;
                    };
                }
                PluginInstallationAction::Install(inner) => {
                    // ensure the plugin priority and environment_plugin_grant_id is not already used
                    if updated.iter().any(|p| p.priority == inner.priority) {
                        return Err(ComponentError::ConflictingPluginPriority(inner.priority));
                    };

                    if updated
                        .iter()
                        .any(|p| p.environment_plugin_grant_id == inner.environment_plugin_grant_id)
                    {
                        return Err(ComponentError::ConflictingEnvironmentPluginGrantId(
                            inner.environment_plugin_grant_id,
                        ));
                    };

                    // get the plugin details and ensure the plugin is installed to the environment
                    let environment_plugin_grant = self
                        .environment_plugin_grant_service
                        .get_active_by_id_for_environment(
                            inner.environment_plugin_grant_id,
                            environment,
                            auth,
                        )
                        .await
                        .map_err(|err| match err {
                            EnvironmentPluginGrantError::EnvironmentPluginGrantNotFound(
                                grant_id,
                            ) => ComponentError::EnvironmentPluginNotFound(grant_id),
                            other => other.into(),
                        })?;

                    updated.push(InstalledPlugin {
                        environment_plugin_grant_id: environment_plugin_grant.id,
                        parameters: inner.parameters,
                        priority: inner.priority,
                        plugin_registration_id: environment_plugin_grant.plugin.id,
                        oplog_processor_component_id: environment_plugin_grant
                            .plugin
                            .oplog_processor_component_id(),
                        oplog_processor_component_revision: environment_plugin_grant
                            .plugin
                            .oplog_processor_component_revision(),
                        plugin_name: environment_plugin_grant.plugin.name,
                        plugin_version: environment_plugin_grant.plugin.version,
                    });
                }
            }
        }

        let non_unique_priorities = updated
            .iter()
            .into_group_map_by(|p| p.priority)
            .into_iter()
            .filter(|(_, plugins)| plugins.len() > 1)
            .collect::<HashMap<_, _>>();
        if let Some((priority, _)) = non_unique_priorities.iter().next() {
            return Err(ComponentError::ConflictingPluginPriority(*priority));
        }

        Ok(updated)
    }

    async fn apply_provision_config_update(
        &self,
        agent_type_name: &AgentTypeName,
        existing: AgentTypeProvisionConfig,
        update: AgentTypeProvisionConfigUpdate,
        uploaded_files: &HashMap<ArchiveFilePath, (AgentFileContentHash, u64)>,
        agent_type: &AgentTypeSchema,
        environment: &Environment,
        auth: &AuthCtx,
    ) -> Result<AgentTypeProvisionConfig, ComponentError> {
        // Env
        let env = update.env.unwrap_or(existing.env);

        // Config entries: validate and transform new ones, or keep existing
        let config = if let Some(new_config) = update.config {
            validate_and_transform_config_entries(agent_type, new_config)?
        } else {
            existing.config
        };

        // Files: start from existing, remove removed, add/update new ones
        let removed: HashSet<AgentFilePath> = HashSet::from_iter(update.files_to_remove);
        let mut files: HashMap<AgentFilePath, InitialAgentFile> = existing
            .files
            .into_iter()
            .filter(|f| !removed.contains(&f.path))
            .map(|f| (f.path.clone(), f))
            .collect();

        for (archive_path, options) in &update.files_to_add_or_update {
            let (content_hash, size) = uploaded_files.get(archive_path).ok_or_else(|| {
                ComponentError::AgentFileNotFoundInArchive {
                    agent_type: agent_type_name.clone(),
                    archive_path: archive_path.clone(),
                }
            })?;
            files.insert(
                options.target_path.clone(),
                InitialAgentFile {
                    path: options.target_path.clone(),
                    content_hash: *content_hash,
                    permissions: options.permissions,
                    size: *size,
                },
            );
        }

        for (target_path, permissions) in &update.file_permission_updates {
            if let Some(file) = files.get_mut(target_path) {
                file.permissions = *permissions;
            }
        }

        let files = files.into_values().collect();

        // Plugins
        let plugins = self
            .update_plugin_installations(environment, existing.plugins, update.plugin_updates, auth)
            .await?;

        Ok(AgentTypeProvisionConfig {
            env,
            config,
            plugins,
            files,
        })
    }
}

fn resolve_files_for_creation(
    agent_type_name: &AgentTypeName,
    creation: &AgentTypeProvisionConfigCreation,
    uploaded_files: &HashMap<ArchiveFilePath, (AgentFileContentHash, u64)>,
) -> Result<Vec<InitialAgentFile>, ComponentError> {
    creation
        .files
        .iter()
        .map(|(archive_path, options)| {
            let (content_hash, size) = uploaded_files.get(archive_path).ok_or_else(|| {
                ComponentError::AgentFileNotFoundInArchive {
                    agent_type: agent_type_name.clone(),
                    archive_path: archive_path.clone(),
                }
            })?;
            Ok(InitialAgentFile {
                path: options.target_path.clone(),
                content_hash: *content_hash,
                permissions: options.permissions,
                size: *size,
            })
        })
        .collect()
}

fn resolve_plugins_for_creation(
    plugin_installations: &[PluginInstallation],
    resolved_grants: &HashMap<EnvironmentPluginGrantId, EnvironmentPluginGrantWithDetails>,
) -> Result<Vec<InstalledPlugin>, ComponentError> {
    let mut result: Vec<InstalledPlugin> = Vec::new();

    for plugin_installation in plugin_installations {
        // ensure the plugin priority is not already used within this agent type
        if result
            .iter()
            .any(|p| p.priority == plugin_installation.priority)
        {
            return Err(ComponentError::ConflictingPluginPriority(
                plugin_installation.priority,
            ));
        };

        if result.iter().any(|p| {
            p.environment_plugin_grant_id == plugin_installation.environment_plugin_grant_id
        }) {
            return Err(ComponentError::ConflictingEnvironmentPluginGrantId(
                plugin_installation.environment_plugin_grant_id,
            ));
        };

        // look up the pre-resolved grant details
        let grant = resolved_grants
            .get(&plugin_installation.environment_plugin_grant_id)
            .ok_or(ComponentError::EnvironmentPluginNotFound(
                plugin_installation.environment_plugin_grant_id,
            ))?;

        result.push(InstalledPlugin {
            environment_plugin_grant_id: grant.id,
            parameters: plugin_installation.parameters.clone(),
            priority: plugin_installation.priority,
            plugin_registration_id: grant.plugin.id,
            oplog_processor_component_id: grant.plugin.oplog_processor_component_id(),
            oplog_processor_component_revision: grant.plugin.oplog_processor_component_revision(),
            plugin_name: grant.plugin.name.clone(),
            plugin_version: grant.plugin.version.clone(),
        });
    }

    Ok(result)
}

fn validate_and_transform_config_entries(
    agent_type: &AgentTypeSchema,
    config_entries: Vec<AgentConfigEntryDto>,
) -> Result<Vec<TypedAgentConfigEntry>, ComponentError> {
    validate_agent_config_declarations(agent_type)?;

    let mut results = Vec::new();
    let mut seen_keys = HashSet::new();

    for config_value in config_entries {
        let matching_declaration = agent_type
            .config
            .iter()
            .find(|c| c.path == config_value.path)
            .ok_or_else(|| ComponentError::AgentConfigNotDeclared {
                agent: agent_type.type_name.clone(),
                key: config_value.path.clone(),
            })?;

        if matching_declaration.source == AgentConfigSource::Secret {
            return Err(
                ComponentError::AgentConfigProvidedSecretWhereOnlyLocalAllowed {
                    agent: agent_type.type_name.clone(),
                    path: config_value.path,
                },
            );
        }

        // The DTO carries the config value as plain user JSON. Decode it
        // (schema-guided) against the agent graph and the declaration's
        // schema-native `value_type` (refs resolve through the agent's `defs`),
        // validate it, then store a self-contained `TypedSchemaValue` whose defs
        // are projected to exactly those reachable from `value_type`.
        let declared_type = &matching_declaration.value_type;

        let schema_value: SchemaValue =
            from_json_value(&agent_type.schema, declared_type, &config_value.value.0).map_err(
                |err| ComponentError::AgentConfigTypeMismatch {
                    agent: agent_type.type_name.clone(),
                    key: config_value.path.clone(),
                    errors: vec![format!("config value is not a valid schema value: {err}")],
                },
            )?;

        validate_value(&agent_type.schema, declared_type, &schema_value).map_err(|errors| {
            ComponentError::AgentConfigTypeMismatch {
                agent: agent_type.type_name.clone(),
                key: config_value.path.clone(),
                errors: errors.iter().map(|e| e.to_string()).collect(),
            }
        })?;

        if !seen_keys.insert(config_value.path.clone()) {
            return Err(ComponentError::AgentConfigDuplicateValue {
                agent: agent_type.type_name.clone(),
                path: config_value.path,
            });
        }

        let value = typed_schema_value_with_projected_defs(
            &agent_type.schema,
            matching_declaration.value_type.clone(),
            schema_value,
        );

        results.push(TypedAgentConfigEntry {
            path: config_value.path,
            value,
        });
    }

    Ok(results)
}

fn check_config_entries_match(
    agent_type: &AgentTypeSchema,
    config: &[TypedAgentConfigEntry],
) -> Result<(), ComponentError> {
    validate_agent_config_declarations(agent_type)?;

    for entry in config {
        let matching_declaration = agent_type
            .config
            .iter()
            .find(|c| c.path == entry.path)
            .ok_or_else(|| ComponentError::AgentConfigNotDeclared {
                agent: agent_type.type_name.clone(),
                key: entry.path.clone(),
            })?;

        if matching_declaration.source == AgentConfigSource::Secret {
            return Err(
                ComponentError::AgentConfigProvidedSecretWhereOnlyLocalAllowed {
                    agent: agent_type.type_name.clone(),
                    path: entry.path.clone(),
                },
            );
        };

        // Strict compatibility gate. The stored config value is positional
        // (records/variants carry no field/case names at runtime), so it may
        // only be reinterpreted under the updated declaration when the two
        // types are *structurally identical*. Field/case renames, reorderings
        // and width changes are rejected because they would silently change
        // the meaning of the stored value even though it would still
        // "validate" against the new shape. The comparison is cross-graph so
        // the stored value's own graph is compared against the updated agent
        // graph, and coinductive so recursive types terminate.
        // Compare the stored value's own graph against the updated agent graph
        // plus the borrowed declared `value_type`; `is_equivalent_cross_graph`
        // resolves refs through `defs` and never reads `graph.root`, so there is
        // no need to clone the agent's whole `defs` into a temporary graph.
        if !is_equivalent_cross_graph(
            entry.value.graph(),
            entry.value.root_type(),
            &agent_type.schema,
            &matching_declaration.value_type,
        ) {
            return Err(ComponentError::AgentConfigOldConfigNotValid {
                agent: agent_type.type_name.clone(),
                key: entry.path.clone(),
            });
        }
    }
    Ok(())
}

async fn analyze_and_validate_component_wasm(
    agent_types: Vec<AgentTypeSchema>,
    wasm: Arc<[u8]>,
    agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig>,
) -> Result<ComponentMetadata, ComponentError> {
    for agent_type in &agent_types {
        validate_agent_config_declarations(agent_type)?;
    }

    let component_metadata = run_cpu_bound_work(move || {
        ComponentMetadata::analyse_component(&wasm, agent_types, agent_type_provision_configs)
    })
    .await?;

    Ok(component_metadata)
}

fn provision_configs_for_agent_types(
    agent_types: &[AgentTypeSchema],
    provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig>,
) -> BTreeMap<AgentTypeName, AgentTypeProvisionConfig> {
    let agent_type_names = agent_types
        .iter()
        .map(|agent_type| &agent_type.type_name)
        .collect::<HashSet<_>>();

    provision_configs
        .into_iter()
        .filter(|(agent_type_name, _)| agent_type_names.contains(agent_type_name))
        .collect()
}

fn validate_component_metadata_invariants(
    metadata: &ComponentMetadata,
) -> Result<(), ComponentError> {
    let mut agent_type_names = HashSet::new();

    for agent_type in metadata.agent_types() {
        if !agent_type_names.insert(agent_type.type_name.clone()) {
            return Err(ComponentError::DuplicateAgentTypeName(
                agent_type.type_name.clone(),
            ));
        }
    }

    for agent_type_name in metadata.agent_type_provision_configs().keys() {
        if !agent_type_names.contains(agent_type_name) {
            return Err(ComponentError::UndeclaredAgentTypeInProvisionConfig(
                agent_type_name.clone(),
            ));
        }
    }

    for agent_type in metadata.agent_types() {
        match metadata.agent_type_initial_permission_template(&agent_type.type_name) {
            Some(_) => {}
            _ => {
                return Err(ComponentError::MissingAgentInitialPermissionTemplate(
                    agent_type.type_name.clone(),
                ));
            }
        }
    }

    for agent_type_name in metadata.agent_type_initial_permission_templates().keys() {
        if !agent_type_names.contains(agent_type_name) {
            return Err(
                ComponentError::UndeclaredAgentTypeInInitialPermissionTemplate(
                    agent_type_name.clone(),
                ),
            );
        }
    }

    Ok(())
}

fn authorize_component_permission(
    auth: &AuthCtx,
    environment: &Environment,
    component_name: &ComponentName,
    verb: ComponentVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::Component(ClassPermissionTarget {
        verb: Some(verb),
        owner: ComponentOwnerPattern::Component {
            account: environment.owner_account_email.clone(),
            application: environment.application_name.clone(),
            environment: environment.name.clone(),
            component: component_name.clone(),
        },
        resource: ComponentResourcePattern::Any,
    }))
}

fn validate_agent_config_declarations(agent_type: &AgentTypeSchema) -> Result<(), ComponentError> {
    for declaration in &agent_type.config {
        validate_agent_config_path(&agent_type.type_name, &declaration.path)?;
    }

    Ok(())
}

fn validate_agent_config_path(
    agent: &AgentTypeName,
    path: &[String],
) -> Result<(), ComponentError> {
    if path.iter().any(|segment| segment.contains('.')) {
        return Err(ComponentError::AgentConfigPathSegmentContainsDot {
            agent: agent.clone(),
            key: path.to_vec(),
        });
    }

    Ok(())
}
