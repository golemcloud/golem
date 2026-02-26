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

use super::ComponentError;
use crate::model::component::{FinalizedComponentRevision, NewComponentRevision};
use crate::repo::component::ComponentRepo;
use crate::repo::model::component::{ComponentRepoError, ComponentRevisionRecord};
use crate::services::account_usage::AccountUsageService;
use crate::services::component_compilation::ComponentCompilationService;
use crate::services::component_object_store::ComponentObjectStore;
use crate::services::environment::EnvironmentError;
use crate::services::environment::EnvironmentService;
use crate::services::environment_plugin_grant::{
    EnvironmentPluginGrantError, EnvironmentPluginGrantService,
};
use anyhow::Context;
use golem_common::base_model::component::LocalAgentConfigEntry as CommonLocalAgentConfigEntry;
use golem_common::model::agent::{AgentType, ConfigValueType};
use golem_common::model::component::ComponentRevision;
use golem_common::model::component::{
    ComponentCreation, ComponentFileContentHash, ComponentFileOptions, ComponentFilePath,
    ComponentFilePermissions, ComponentUpdate, InitialComponentFile, InstalledPlugin,
    PluginInstallationAction,
};
use golem_common::model::component::{ComponentId, PluginInstallation};
use golem_common::model::diff::Hash;
use golem_common::model::environment::{Environment, EnvironmentId};
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::EnvironmentAction;
use golem_service_base::model::{Component, LocalAgentConfigEntry};
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_wasm::ValueAndType;
use golem_wasm::analysis::AnalysedType;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use itertools::Itertools;
use std::collections::HashSet;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tempfile::NamedTempFile;
use tracing::{debug, info};

pub struct ComponentWriteService {
    component_repo: Arc<dyn ComponentRepo>,
    object_store: Arc<ComponentObjectStore>,
    component_compilation: Arc<dyn ComponentCompilationService>,
    initial_component_files_service: Arc<InitialComponentFilesService>,
    account_usage_service: Arc<AccountUsageService>,
    environment_service: Arc<EnvironmentService>,
    environment_plugin_grant_service: Arc<EnvironmentPluginGrantService>,
}

impl ComponentWriteService {
    pub fn new(
        component_repo: Arc<dyn ComponentRepo>,
        object_store: Arc<ComponentObjectStore>,
        component_compilation: Arc<dyn ComponentCompilationService>,
        initial_component_files_service: Arc<InitialComponentFilesService>,
        account_usage_service: Arc<AccountUsageService>,
        environment_service: Arc<EnvironmentService>,
        environment_plugin_grant_service: Arc<EnvironmentPluginGrantService>,
    ) -> Self {
        Self {
            component_repo,
            object_store,
            component_compilation,
            initial_component_files_service,
            account_usage_service,
            environment_service,
            environment_plugin_grant_service,
        }
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

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::CreateComponent,
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

        let initial_component_files: Vec<InitialComponentFile> = self
            .initial_component_files_for_new_component(
                environment_id,
                files_archive,
                component_creation.file_options,
            )
            .await?;

        let component_id = ComponentId::new();
        let (wasm_hash, wasm_object_store_key) = self
            .upload_and_hash_component_wasm(environment_id, wasm.clone())
            .await?;

        let local_agent_config = validate_and_transform_local_agent_config_entries(
            &component_creation.agent_types,
            component_creation.local_agent_config,
        )?;

        let new_revision = NewComponentRevision::new(
            component_id,
            ComponentRevision::INITIAL,
            environment_id,
            component_creation.component_name.clone(),
            initial_component_files,
            component_creation.env,
            component_creation.config_vars,
            local_agent_config,
            wasm_hash,
            wasm_object_store_key,
            self.plugin_installations_for_new_component(
                &environment,
                component_creation.plugins,
                auth,
            )
            .await?,
            component_creation.agent_types,
        );

        let finalized_revision = self
            .finalize_new_component_revision(environment_id, new_revision, wasm)
            .await?;

        let record =
            ComponentRevisionRecord::from_model(finalized_revision.clone(), auth.account_id());

        let stored_component: Component = self
            .component_repo
            .create(
                environment_id.0,
                &component_creation.component_name.0,
                record,
                environment.version_check,
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
            .try_into_model(environment.application_id, environment.owner_account_id)?;

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

        let environment = self
            .environment_service
            .get(EnvironmentId(component_record.environment_id), false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    ComponentError::ComponentNotFound(component_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewComponent,
        )
        .map_err(|_| ComponentError::ComponentNotFound(component_id))?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::UpdateComponent,
        )?;

        let component = component_record
            .try_into_model(environment.application_id, environment.owner_account_id)?;

        if component_update.current_revision != component.revision {
            Err(ComponentError::ConcurrentUpdate)?
        };

        let environment_id = component.environment_id;
        let component_id = component.id;

        info!(environment_id = %environment_id, "Update component");

        let (wasm, wasm_object_store_key, wasm_hash) = if let Some(new_data) = new_wasm {
            self.account_usage_service
                .ensure_updated_component_within_limits(
                    environment.owner_account_id,
                    u64::try_from(new_data.len()).unwrap(),
                )
                .await?;

            let (wasm_hash, wasm_object_store_key) = self
                .upload_and_hash_component_wasm(environment_id, new_data.clone())
                .await?;

            (new_data, wasm_object_store_key, wasm_hash)
        } else {
            let old_data = self
                .object_store
                .get(environment_id, &component.object_store_key)
                .await?;
            (
                Arc::from(old_data),
                component.object_store_key,
                component.wasm_hash,
            )
        };

        let agent_types = component_update
            .agent_types
            .unwrap_or(component.metadata.agent_types().to_vec());

        let local_agent_config = if let Some(updated) = component_update.local_agent_config {
            validate_and_transform_local_agent_config_entries(&agent_types, updated)?
        } else {
            check_transformed_local_agent_config_entries_match(
                &agent_types,
                &component.local_agent_config,
            )?;
            component.local_agent_config
        };

        let new_revision = NewComponentRevision::new(
            component_id,
            component.revision.next()?,
            environment_id,
            component.component_name,
            self.update_initial_component_files(
                environment_id,
                component.files,
                component_update.removed_files,
                new_files_archive,
                component_update.new_file_options,
            )
            .await?,
            component_update.env.unwrap_or(component.env),
            component_update
                .config_vars
                .unwrap_or(component.config_vars),
            local_agent_config,
            wasm_hash,
            wasm_object_store_key,
            self.update_plugin_installations(
                &environment,
                component.installed_plugins,
                component_update.plugin_updates,
                auth,
            )
            .await?,
            agent_types,
        );

        let finalized_revision = self
            .finalize_new_component_revision(environment_id, new_revision, wasm)
            .await?;

        let record = ComponentRevisionRecord::from_model(finalized_revision, auth.account_id());

        let stored_component: Component = self
            .component_repo
            .update(record, environment.version_check)
            .await
            .map_err(|err| match err {
                ComponentRepoError::ConcurrentModification => ComponentError::ConcurrentUpdate,
                ComponentRepoError::VersionAlreadyExists { version } => {
                    ComponentError::ComponentVersionAlreadyExists(version)
                }
                other => other.into(),
            })?
            .try_into_model(environment.application_id, environment.owner_account_id)?;

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

        let environment = self
            .environment_service
            .get(EnvironmentId(component_record.environment_id), false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    ComponentError::ComponentNotFound(component_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewComponent,
        )
        .map_err(|_| ComponentError::ComponentNotFound(component_id))?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::UpdateComponent,
        )?;

        let component = component_record
            .try_into_model(environment.application_id, environment.owner_account_id)?;

        if current_revision != component.revision {
            Err(ComponentError::ConcurrentUpdate)?
        };

        self.component_repo
            .delete(
                auth.account_id().0,
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

    async fn initial_component_files_for_new_component(
        &self,
        environment_id: EnvironmentId,
        files_archive: Option<NamedTempFile>,
        file_options: BTreeMap<ComponentFilePath, ComponentFileOptions>,
    ) -> Result<Vec<InitialComponentFile>, ComponentError> {
        let uploaded_files = match files_archive {
            Some(files) => self.upload_component_files(environment_id, files).await?,
            None => HashMap::new(),
        };

        let mut result = Vec::new();

        for (path, key) in uploaded_files {
            let options = file_options.get(&path).cloned().unwrap_or_default();
            result.push(InitialComponentFile {
                path,
                content_hash: key,
                permissions: options.permissions,
            });
        }

        Ok(result)
    }

    async fn update_initial_component_files(
        &self,
        environment_id: EnvironmentId,
        previous: Vec<InitialComponentFile>,
        removed_files: Vec<ComponentFilePath>,
        new_files_archive: Option<NamedTempFile>,
        new_file_options: BTreeMap<ComponentFilePath, ComponentFileOptions>,
    ) -> Result<Vec<InitialComponentFile>, ComponentError> {
        let uploaded_files = match new_files_archive {
            Some(files) => self.upload_component_files(environment_id, files).await?,
            None => HashMap::new(),
        };

        let removed_files: HashSet<ComponentFilePath> = HashSet::from_iter(removed_files);

        let mut result = HashMap::new();
        for file in previous {
            if !removed_files.contains(&file.path) {
                result.insert(file.path.clone(), file);
            }
        }

        for (path, key) in uploaded_files {
            result.insert(
                path.clone(),
                InitialComponentFile {
                    content_hash: key,
                    path,
                    permissions: ComponentFilePermissions::default(),
                },
            );
        }

        for (path, options) in new_file_options {
            let entry = result.get_mut(&path);
            if let Some(entry) = entry {
                entry.permissions = options.permissions;
            }
        }

        Ok(result.into_values().collect())
    }

    async fn upload_component_files(
        &self,
        environment_id: EnvironmentId,
        archive: NamedTempFile,
    ) -> Result<HashMap<ComponentFilePath, ComponentFileContentHash>, ComponentError> {
        let to_upload = super::utils::prepare_component_files_for_upload(archive).await?;

        let tasks = to_upload.into_iter().map(|(path, stream)| async move {
            info!("Uploading file: {}", path.to_string());

            let key = self
                .initial_component_files_service
                .put_if_not_exists(environment_id, &stream)
                .await
                .context("Failed to upload component files")?;

            Ok::<_, ComponentError>((path, key))
        });

        let uploaded = futures::future::try_join_all(tasks).await?;

        Ok(HashMap::from_iter(uploaded))
    }

    async fn plugin_installations_for_new_component(
        &self,
        environment: &Environment,
        plugin_installations: Vec<PluginInstallation>,
        auth: &AuthCtx,
    ) -> Result<Vec<InstalledPlugin>, ComponentError> {
        let mut result: Vec<InstalledPlugin> = Vec::new();

        for plugin_installation in plugin_installations {
            // ensure the plugin priority is not already used
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

            // get the plugin details and ensure the plugin is installed to the environment
            let environment_plugin_grant = self
                .environment_plugin_grant_service
                .get_active_by_id_for_environment(
                    plugin_installation.environment_plugin_grant_id,
                    environment,
                    auth,
                )
                .await
                .map_err(|err| match err {
                    EnvironmentPluginGrantError::EnvironmentPluginGrantNotFound(grant_id) => {
                        ComponentError::EnvironmentPluginNotFound(grant_id)
                    }
                    other => other.into(),
                })?;

            result.push(InstalledPlugin {
                environment_plugin_grant_id: environment_plugin_grant.id,
                parameters: plugin_installation.parameters,
                priority: plugin_installation.priority,
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

        Ok(result)
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

    async fn finalize_new_component_revision(
        &self,
        environment_id: EnvironmentId,
        new_revision: NewComponentRevision,
        wasm: Arc<[u8]>,
    ) -> Result<FinalizedComponentRevision, ComponentError> {
        let (_, transformed_object_store_key) = self
            .upload_and_hash_component_wasm(environment_id, wasm.clone())
            .await?;

        let finalized_revision =
            new_revision.with_transformed_component(transformed_object_store_key, &wasm)?;

        if let Some(known_root_package_name) = &finalized_revision.metadata.root_package_name()
            && finalized_revision.component_name.0 != *known_root_package_name
        {
            Err(ComponentError::InvalidComponentName {
                actual: finalized_revision.component_name.0.clone(),
                expected: known_root_package_name.clone(),
            })?;
        }

        debug!(
            environment_id = %environment_id,
            "Finalized component",
        );

        Ok(finalized_revision)
    }
}

fn validate_and_transform_local_agent_config_entries(
    agent_types: &[AgentType],
    mut agent_local_config: Vec<CommonLocalAgentConfigEntry>,
) -> Result<Vec<LocalAgentConfigEntry>, ComponentError> {
    let mut results = Vec::new();

    for agent_type in agent_types {
        let mut seen_agent_config_keys = HashSet::new();

        let applicable_config_values =
            agent_local_config.extract_if(.., |alc| alc.agent == agent_type.type_name);
        for config_value in applicable_config_values {
            let matching_declaration = agent_type
                .config
                .iter()
                .find(|c| c.key == config_value.key)
                .ok_or_else(|| ComponentError::AgentConfigNotDeclared {
                    agent: agent_type.type_name.clone(),
                    key: config_value.key.clone(),
                })?;

            let analysed_type: AnalysedType = match &matching_declaration.value {
                ConfigValueType::Local(inner) => inner.value.clone(),
                ConfigValueType::Shared(_) => {
                    return Err(
                        ComponentError::AgentConfigProvidedSharedWhereOnlyLocalAllowed {
                            agent: agent_type.type_name.clone(),
                            key: config_value.key,
                        },
                    );
                }
            };

            let value = ValueAndType::parse_with_type(&config_value.value, &analysed_type)
                .map_err(|errors| ComponentError::AgentConfigTypeMismatch {
                    agent: agent_type.type_name.clone(),
                    key: config_value.key.clone(),
                    errors,
                })?;

            let result = LocalAgentConfigEntry {
                agent: config_value.agent,
                key: config_value.key,
                value,
            };

            if !seen_agent_config_keys.insert(result.key.clone()) {
                return Err(ComponentError::AgentConfigDuplicateValue {
                    agent: agent_type.type_name.clone(),
                    key: result.key.clone(),
                });
            }

            results.push(result);
        }
    }

    Ok(results)
}

fn check_transformed_local_agent_config_entries_match(
    agent_types: &[AgentType],
    agent_local_config: &[LocalAgentConfigEntry],
) -> Result<(), ComponentError> {
    for agent_type in agent_types {
        let applicable_config_values = agent_local_config
            .iter()
            .filter(|alc| alc.agent == agent_type.type_name);
        for config_value in applicable_config_values {
            let matching_declaration = agent_type
                .config
                .iter()
                .find(|c| c.key == config_value.key)
                .ok_or_else(|| ComponentError::AgentConfigNotDeclared {
                    agent: agent_type.type_name.clone(),
                    key: config_value.key.clone(),
                })?;

            let analysed_type: &AnalysedType = match &matching_declaration.value {
                ConfigValueType::Local(inner) => &inner.value,
                ConfigValueType::Shared(_) => {
                    return Err(
                        ComponentError::AgentConfigProvidedSharedWhereOnlyLocalAllowed {
                            agent: agent_type.type_name.clone(),
                            key: config_value.key.clone(),
                        },
                    );
                }
            };

            if config_value.value.typ != *analysed_type {
                return Err(ComponentError::AgentConfigOldConfigNotValid {
                    agent: agent_type.type_name.clone(),
                    key: config_value.key.clone(),
                });
            }
        }
    }
    Ok(())
}
