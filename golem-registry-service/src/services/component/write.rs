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
use super::component_transformer_plugin_caller::ComponentTransformerPluginCaller;
use crate::model::component::Component;
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
use crate::services::plugin_registration::PluginRegistrationService;
use crate::services::run_cpu_bound_work;
use anyhow::{Context, anyhow};
use golem_common::model::account::AccountId;
use golem_common::model::component::{
    ComponentCreation, ComponentFileContentHash, ComponentFileOptions, ComponentFilePath,
    ComponentFilePermissions, ComponentUpdate, InitialComponentFile, InstalledPlugin,
    PluginInstallationAction,
};
use golem_common::model::component::{ComponentId, PluginInstallation};
use golem_common::model::component::{ComponentRevision, PluginPriority};
use golem_common::model::diff::Hash;
use golem_common::model::environment::{Environment, EnvironmentId};
use golem_common::widen_infallible;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::EnvironmentAction;
use golem_service_base::model::plugin_registration::{
    AppPluginSpec, LibraryPluginSpec, PluginSpec,
};
use golem_service_base::replayable_stream::ReplayableStream;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use std::collections::HashSet;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tempfile::NamedTempFile;
use tracing::{Instrument, debug, info, info_span};

pub struct ComponentWriteService {
    component_repo: Arc<dyn ComponentRepo>,
    object_store: Arc<ComponentObjectStore>,
    component_compilation: Arc<dyn ComponentCompilationService>,
    initial_component_files_service: Arc<InitialComponentFilesService>,
    plugin_wasm_files_service: Arc<PluginWasmFilesService>,
    account_usage_service: Arc<AccountUsageService>,
    environment_service: Arc<EnvironmentService>,
    environment_plugin_grant_service: Arc<EnvironmentPluginGrantService>,
    plugin_registration_service: Arc<PluginRegistrationService>,
    component_transformer_plugin_caller: Arc<dyn ComponentTransformerPluginCaller>,
}

impl ComponentWriteService {
    pub fn new(
        component_repo: Arc<dyn ComponentRepo>,
        object_store: Arc<ComponentObjectStore>,
        component_compilation: Arc<dyn ComponentCompilationService>,
        initial_component_files_service: Arc<InitialComponentFilesService>,
        plugin_wasm_files_service: Arc<PluginWasmFilesService>,
        account_usage_service: Arc<AccountUsageService>,
        environment_service: Arc<EnvironmentService>,
        environment_plugin_grant_service: Arc<EnvironmentPluginGrantService>,
        plugin_registration_service: Arc<PluginRegistrationService>,
        component_transformer_plugin_caller: Arc<dyn ComponentTransformerPluginCaller>,
    ) -> Self {
        Self {
            component_repo,
            object_store,
            component_compilation,
            initial_component_files_service,
            plugin_wasm_files_service,
            account_usage_service,
            environment_service,
            environment_plugin_grant_service,
            plugin_registration_service,
            component_transformer_plugin_caller,
        }
    }

    pub async fn create(
        &self,
        environment_id: &EnvironmentId,
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
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::CreateComponent,
        )?;

        // Fast path check to avoid processing the component if we are going to reject it anyway
        self.component_repo
            .get_staged_by_name(&environment_id.0, &component_creation.component_name.0)
            .await?
            .map_or(Ok(()), |_| {
                Err(ComponentError::ComponentWithNameAlreadyExists(
                    component_creation.component_name.clone(),
                ))
            })?;

        self.account_usage_service
            .ensure_new_component_within_limits(&environment.owner_account_id, wasm.len() as i64)
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

        let new_revision = NewComponentRevision::new(
            component_id,
            ComponentRevision::INITIAL,
            *environment_id,
            component_creation.component_name.clone(),
            initial_component_files,
            component_creation.env,
            wasm_hash,
            wasm_object_store_key,
            component_creation.dynamic_linking,
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
                &environment_id.0,
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
            .enqueue_compilation(environment_id, &component_id, stored_component.revision)
            .await;

        Ok(stored_component)
    }

    pub async fn update(
        &self,
        component_id: &ComponentId,
        component_update: ComponentUpdate,
        new_wasm: Option<Vec<u8>>,
        new_files_archive: Option<NamedTempFile>,
        auth: &AuthCtx,
    ) -> Result<Component, ComponentError> {
        let new_wasm: Option<Arc<[u8]>> = new_wasm.map(Arc::from);

        let component_record = self
            .component_repo
            .get_staged_by_id(&component_id.0)
            .await?
            .ok_or(ComponentError::ComponentNotFound(*component_id))?;

        let environment = self
            .environment_service
            .get(&EnvironmentId(component_record.environment_id), false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    ComponentError::ComponentNotFound(*component_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewComponent,
        )
        .map_err(|_| ComponentError::ComponentNotFound(*component_id))?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
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
                    &environment.owner_account_id,
                    new_data.len() as i64,
                )
                .await?;

            let (wasm_hash, wasm_object_store_key) = self
                .upload_and_hash_component_wasm(&environment_id, new_data.clone())
                .await?;

            (new_data, wasm_object_store_key, wasm_hash)
        } else {
            let old_data = self
                .object_store
                .get(&environment_id, &component.object_store_key)
                .await?;
            (
                Arc::from(old_data),
                component.object_store_key,
                component.wasm_hash,
            )
        };

        let new_revision = NewComponentRevision::new(
            component_id,
            component.revision.next()?,
            environment_id,
            component.component_name,
            self.update_initial_component_files(
                &environment_id,
                component.files,
                component_update.removed_files,
                new_files_archive,
                component_update.new_file_options,
            )
            .await?,
            component_update.env.unwrap_or(component.env),
            wasm_hash,
            wasm_object_store_key,
            component_update
                .dynamic_linking
                .unwrap_or(component.metadata.dynamic_linking().clone()),
            self.update_plugin_installations(
                &environment,
                component.installed_plugins,
                component_update.plugin_updates,
                auth,
            )
            .await?,
            component_update
                .agent_types
                .unwrap_or(component.metadata.agent_types().to_vec()),
        );

        let finalized_revision = self
            .finalize_new_component_revision(&environment_id, new_revision, wasm)
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
            .enqueue_compilation(&environment_id, &component_id, stored_component.revision)
            .await;

        Ok(stored_component)
    }

    pub async fn delete(
        &self,
        component_id: &ComponentId,
        current_revision: ComponentRevision,
        auth: &AuthCtx,
    ) -> Result<(), ComponentError> {
        let component_record = self
            .component_repo
            .get_staged_by_id(&component_id.0)
            .await?
            .ok_or(ComponentError::ComponentNotFound(*component_id))?;

        let environment = self
            .environment_service
            .get(&EnvironmentId(component_record.environment_id), false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    ComponentError::ComponentNotFound(*component_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewComponent,
        )
        .map_err(|_| ComponentError::ComponentNotFound(*component_id))?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
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
                &auth.account_id().0,
                &component_id.0,
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
        environment_id: &EnvironmentId,
        data: Arc<[u8]>,
    ) -> Result<(Hash, String), ComponentError> {
        // TODO: use something like PluginWasmFilesService instead of raw object store
        let hash = self.object_store.put(environment_id, data).await?;
        Ok((hash, hash.to_string()))
    }

    async fn initial_component_files_for_new_component(
        &self,
        environment_id: &EnvironmentId,
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
        environment_id: &EnvironmentId,
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
        environment_id: &EnvironmentId,
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

            // get the plugin details and ensure the plugin is installed to the environment
            let environment_plugin_grant = self
                .environment_plugin_grant_service
                .get_active_by_id_for_environment(
                    &plugin_installation.environment_plugin_grant_id,
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
                        .position(|p| p.priority == inner.plugin_priority)
                        .ok_or(ComponentError::PluginInstallationNotFound(
                            inner.plugin_priority,
                        ))?;

                    updated.swap_remove(plugin_index);
                }
                PluginInstallationAction::Update(inner) => {
                    let plugin_index = updated
                        .iter()
                        .position(|p| p.priority == inner.plugin_priority)
                        .ok_or(ComponentError::PluginInstallationNotFound(
                            inner.plugin_priority,
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
                    // ensure the plugin priority is not already used
                    if updated.iter().any(|p| p.priority == inner.priority) {
                        return Err(ComponentError::ConflictingPluginPriority(inner.priority));
                    };

                    // get the plugin details and ensure the plugin is installed to the environment
                    let environment_plugin_grant = self
                        .environment_plugin_grant_service
                        .get_active_by_id_for_environment(
                            &inner.environment_plugin_grant_id,
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

        Ok(updated)
    }

    async fn finalize_new_component_revision(
        &self,
        environment_id: &EnvironmentId,
        new_revision: NewComponentRevision,
        wasm: Arc<[u8]>,
    ) -> Result<FinalizedComponentRevision, ComponentError> {
        let (new_revision, transformed_data) = self
            .transform_with_installed_plugins(new_revision, wasm)
            .await?;

        let (_, transformed_object_store_key) = self
            .upload_and_hash_component_wasm(environment_id, transformed_data.clone())
            .await?;

        let finalized_revision = new_revision
            .with_transformed_component(transformed_object_store_key, &transformed_data)?;

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
            exports = ?finalized_revision.metadata.exports(),
            dynamic_linking = ?finalized_revision.metadata.dynamic_linking(),
            "Finalized component",
        );

        Ok(finalized_revision)
    }

    async fn transform_with_installed_plugins(
        &self,
        mut component: NewComponentRevision,
        mut data: Arc<[u8]>,
    ) -> Result<(NewComponentRevision, Arc<[u8]>), ComponentError> {
        if component.installed_plugins.is_empty() {
            return Ok((component, data));
        };

        let mut installed_plugins = component.installed_plugins.clone();
        installed_plugins.sort_by_key(|p| p.priority);

        for installation in installed_plugins {
            // Auth was checked when initially installing the plugins. No need to check here (and users wouldn't be able to directly access the plugin anyway)
            // include deleted here as the plugin might have been deleted in the meantime, but components should keep working.
            let plugin = self
                .plugin_registration_service
                .get_plugin(&installation.plugin_registration_id, true, &AuthCtx::System)
                .await?;

            match plugin.spec {
                PluginSpec::ComponentTransformer(spec) => {
                    let span = info_span!("component transformation",
                        component_id = %component.component_id,
                        plugin_registration_id = %installation.plugin_registration_id,
                        plugin_priority = %installation.priority,
                    );

                    (component, data) = self
                        .apply_component_transformer_plugin(
                            component,
                            installation.priority,
                            data,
                            spec.transform_url,
                            &installation.parameters,
                        )
                        .instrument(span)
                        .await?;
                }
                PluginSpec::Library(spec) => {
                    let span = info_span!("library plugin",
                        component_id = %component.component_id,
                        plugin_registration_id = %installation.plugin_registration_id,
                        plugin_priority = %installation.priority,
                    );
                    data = self
                        .apply_library_plugin(
                            data,
                            &plugin.account_id,
                            installation.priority,
                            &spec,
                        )
                        .instrument(span)
                        .await?;
                }
                PluginSpec::App(spec) => {
                    let span = info_span!("app plugin",
                        component_id = %component.component_id,
                        plugin_registration_id = %installation.plugin_registration_id,
                        plugin_priority = %installation.priority,
                    );
                    data = self
                        .apply_app_plugin(data, &plugin.account_id, installation.priority, &spec)
                        .instrument(span)
                        .await?;
                }
                PluginSpec::OplogProcessor(_) => (),
            }
        }

        Ok((component, data))
    }

    async fn apply_component_transformer_plugin(
        &self,
        mut component: NewComponentRevision,
        plugin_priority: PluginPriority,
        data: Arc<[u8]>,
        url: String,
        parameters: &BTreeMap<String, String>,
    ) -> Result<(NewComponentRevision, Arc<[u8]>), ComponentError> {
        info!(%url, "Applying component transformation plugin");

        let response = self
            .component_transformer_plugin_caller
            .call_remote_transformer_plugin(&component, &data, url, parameters)
            .await
            .map_err(|err| ComponentError::ComponentTransformerPluginFailed {
                plugin_priority,
                reason: err,
            })?;

        let data = response.data.map(|b64| Arc::from(b64.0)).unwrap_or(data);

        for (k, v) in response.env.unwrap_or_default() {
            component.env.insert(k, v);
        }

        let mut files = component.files;
        for file in response.additional_files.unwrap_or_default() {
            let content_stream = file
                .content
                .0
                .map_item(|i| i.map_err(widen_infallible::<anyhow::Error>))
                .map_error(widen_infallible::<anyhow::Error>);

            let key = self
                .initial_component_files_service
                .put_if_not_exists(&component.environment_id, content_stream)
                .await?;

            let item = InitialComponentFile {
                content_hash: key,
                path: file.path,
                permissions: file.permissions,
            };

            files.retain_mut(|f| f.path != item.path);
            files.push(item)
        }
        component.files = files;

        Ok((component, data))
    }

    async fn apply_library_plugin(
        &self,
        data: Arc<[u8]>,
        plugin_owner: &AccountId,
        plugin_priority: PluginPriority,
        plugin_spec: &LibraryPluginSpec,
    ) -> Result<Arc<[u8]>, ComponentError> {
        tracing::debug!("applying library plugin");

        let plug_bytes = self
            .plugin_wasm_files_service
            .get(plugin_owner, &plugin_spec.wasm_content_hash)
            .await?
            .ok_or(anyhow!(
                "Did not find plugin data for key {}",
                plugin_spec.wasm_content_hash.0
            ))?;

        let composed =
            run_cpu_bound_work(move || super::utils::compose_components(&data, &plug_bytes))
                .await
                .map_err(|e| ComponentError::PluginCompositionFailed {
                    plugin_priority,
                    cause: e,
                })?;

        Ok(Arc::from(composed))
    }

    async fn apply_app_plugin(
        &self,
        data: Arc<[u8]>,
        plugin_owner: &AccountId,
        plugin_priority: PluginPriority,
        plugin_spec: &AppPluginSpec,
    ) -> Result<Arc<[u8]>, ComponentError> {
        tracing::debug!("applying app plugin");

        let socket_bytes = self
            .plugin_wasm_files_service
            .get(plugin_owner, &plugin_spec.wasm_content_hash)
            .await?
            .ok_or(anyhow!(
                "Did not find plugin data for key {}",
                plugin_spec.wasm_content_hash.0
            ))?;

        let composed =
            run_cpu_bound_work(move || super::utils::compose_components(&socket_bytes, &data))
                .await
                .map_err(|e| ComponentError::PluginCompositionFailed {
                    plugin_priority,
                    cause: e,
                })?;

        Ok(Arc::from(composed))
    }
}
