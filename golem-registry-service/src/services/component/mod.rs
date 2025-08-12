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

pub mod error;
mod utils;

pub use self::error::ComponentError;
use super::component_compilation::ComponentCompilationService;
use super::component_object_store::ComponentObjectStore;
use super::component_transformer_plugin_caller::ComponentTransformerPluginCaller;
use crate::model::component::{ComponentFileOptions, PluginInstallation};
use crate::model::component::{Component, InitialComponentFilesArchiveAndPermissions};
use crate::repo::component::ComponentRepo;
use crate::repo::model::component::{ComponentRevisionRecord, ComponentRevisionRepoError};
use crate::services::account_usage::{AccountUsage, AccountUsageService};
use anyhow::{Context, anyhow};
use golem_common::model::{InitialComponentFile, InitialComponentFileKey, Revision};
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentType;
use golem_common::model::component::ComponentName;
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_common::model::diff::Hash;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{ComponentFilePath, ComponentFilePermissions};
use golem_common::model::{ComponentId, ComponentType};
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use std::collections::HashSet;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Debug;
use std::sync::Arc;
use std::vec;
use tracing::info;
use tempfile::NamedTempFile;

pub struct ComponentService {
    component_repo: Arc<dyn ComponentRepo>,
    object_store: Arc<ComponentObjectStore>,
    component_compilation: Arc<dyn ComponentCompilationService>,
    initial_component_files_service: Arc<InitialComponentFilesService>,
    _plugin_wasm_files_service: Arc<PluginWasmFilesService>,
    _transformer_plugin_caller: Arc<dyn ComponentTransformerPluginCaller>,
    account_usage_service: Arc<AccountUsageService>,
}

impl ComponentService {
    pub fn new(
        component_repo: Arc<dyn ComponentRepo>,
        object_store: Arc<ComponentObjectStore>,
        component_compilation: Arc<dyn ComponentCompilationService>,
        initial_component_files_service: Arc<InitialComponentFilesService>,
        plugin_wasm_files_service: Arc<PluginWasmFilesService>,
        transformer_plugin_caller: Arc<dyn ComponentTransformerPluginCaller>,
        account_usage_service: Arc<AccountUsageService>,
    ) -> Self {
        Self {
            component_repo,
            object_store,
            component_compilation,
            initial_component_files_service,
            _plugin_wasm_files_service: plugin_wasm_files_service,
            _transformer_plugin_caller: transformer_plugin_caller,
            account_usage_service,
        }
    }

    pub async fn create(
        &self,
        environment_id: &EnvironmentId,
        component_name: &ComponentName,
        component_type: ComponentType,
        data: Vec<u8>,
        files_archive: Option<NamedTempFile>,
        file_options: BTreeMap<ComponentFilePath, ComponentFileOptions>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        env: BTreeMap<String, String>,
        agent_types: Vec<AgentType>,
        actor: &AccountId,
    ) -> Result<Component, ComponentError> {
        info!(environment_id = %environment_id, "Create component");

        self.component_repo
            .get_staged_by_name(&environment_id.0, &component_name.0)
            .await?
            .map_or(Ok(()), |rec| {
                Err(ComponentError::AlreadyExists(ComponentId(
                    rec.revision.component_id,
                )))
            })?;

        let mut account_usage = self
            .account_usage_service
            .add_component(actor, data.len() as i64)
            .await?;

        let initial_component_files: Vec<InitialComponentFile> = self.initial_component_files_for_new_component(&environment_id, files_archive, file_options).await?;

        let component_id = ComponentId::new_v4();

        let wasm_hash: Hash = blake3::hash(data.as_slice()).into();

        let mut component = Component::new(
            environment_id.clone(),
            component_id.clone(),
            component_name.clone(),
            component_type,
            &data,
            initial_component_files,
            vec![],
            dynamic_linking,
            env,
            agent_types,
            wasm_hash,
        )?;

        // TODO:
        // let (component, transformed_data) =
        //     self.apply_transformations(component, data.clone()).await?;
        let transformed_data = data.clone();

        component.metadata = ComponentMetadata::analyse_component(&transformed_data, dynamic_linking, agent_types)?;

        if let Some(known_root_package_name) = &component.metadata.root_package_name
            && &component_name.0 != known_root_package_name
        {
            Err(ComponentError::InvalidComponentName {
                actual: component_name.0.clone(),
                expected: known_root_package_name.clone(),
            })?;
        }

        tokio::try_join!(
            self.upload_user_component(&component, data),
            // TODO:
            // self.upload_protected_component(&component, transformed_data)
        )?;

        info!(
            environment_id = %environment_id,
            exports = ?component.metadata.exports,
            dynamic_linking = ?component.metadata.dynamic_linking,
            "Uploaded component",
        );

        let record = ComponentRevisionRecord::from_model(component.clone(), actor);

        let result = self
            .component_repo
            .create(&environment_id.0, &component_name.0, record)
            .await;

        let stored_component: Component = match result? {
            Ok(record) => record.into(),
            Err(ComponentRevisionRepoError::ConcurrentModification) => {
                Err(ComponentError::ConcurrentUpdate {
                    component_id: component_id.clone(),
                    version: 0,
                })?
            }
            Err(ComponentRevisionRepoError::VersionAlreadyExists { .. }) => todo!(),
        };

        account_usage.ack();

        self.component_compilation
            .enqueue_compilation(
                environment_id,
                &component_id,
                stored_component.versioned_component_id.version,
            )
            .await;

        Ok(stored_component)
    }

    pub async fn update(
        &self,
        component_id: &ComponentId,
        data: Option<Vec<u8>>,
        component_type: Option<ComponentType>,
        removed_files: Vec<ComponentFilePath>,
        new_files_archive: Option<NamedTempFile>,
        new_file_options: BTreeMap<ComponentFilePath, ComponentFileOptions>,
        dynamic_linking: Option<HashMap<String, DynamicLinkedInstance>>,
        env: Option<BTreeMap<String, String>>,
        agent_types: Option<Vec<AgentType>>,
        actor: &AccountId,
    ) -> Result<Component, ComponentError> {
        let mut component: Component = self
            .component_repo
            .get_staged_by_id(&component_id.0)
            .await?
            .ok_or(ComponentError::UnknownComponentId(component_id.clone()))?
            .into();

        let environment_id = component.environment_id.clone();
        let component_id = component.versioned_component_id.component_id.clone();
        let current_revision = component.versioned_component_id.version.clone();

        info!(environment_id = %environment_id, "Update component");

        let mut account_usage: Option<AccountUsage> = None;

        // TODO:
        // let constraints = self
        //     .component_repo
        //     .get_constraint(&owner.to_string(), component_id.0)
        //     .await?;

        // let new_type_registry = FunctionDictionary::from_exports(&metadata.exports)
        //     .map_err(|e| anyhow!(e).context("Failed to convert exports to function dictionary"))?;

        // TODO:
        // if let Some(constraints) = constraints {
        //     let conflicts = self::utils::find_component_metadata_conflicts(&constraints, &new_type_registry);
        //     if !conflicts.is_empty() {
        //         return Err(ComponentError::ComponentConstraintConflictError(conflicts));
        //     }
        // }

        component.original_files = self.initial_component_files_for_updated_component(&environment_id, component.files, removed_files, new_files_archive, new_file_options).await?;
        component.files = component.original_files.clone();
        component.original_env = env.unwrap_or(component.env);
        component.env = component.original_env.clone();
        component.component_type = component_type.unwrap_or(component.component_type);

        let data = if let Some(new_data) = data {
            let actual_account_usage = self
                .account_usage_service
                .add_component(actor, new_data.len() as i64)
                .await?;

            let _ = account_usage.insert(actual_account_usage);

            // make sure we are storing data under new keys so we don't clobber old data.
            component.regenerate_object_store_key();
            self.upload_user_component(&component, new_data.clone()).await?;
            new_data
        } else {
            self.object_store.get(&environment_id, &component.full_object_store_key()).await?
        };

        // TODO:
        // let (component, transformed_data) =
        //     self.apply_transformations(component, data.clone()).await?;
        let transformed_data = data.clone();

        {
            let dynamic_linking = dynamic_linking.unwrap_or(component.metadata.dynamic_linking);
            let agent_types = agent_types.unwrap_or(component.metadata.agent_types);
            component.metadata = ComponentMetadata::analyse_component(&transformed_data, dynamic_linking, agent_types)?;
        }

        component.regenerate_transformed_object_store_key();
        self.upload_transformed_component(&component, transformed_data).await?;

        let record = ComponentRevisionRecord::from_model(component, actor);

        let result = self
            .component_repo
            .update(current_revision as i64, record)
            .await;

        let stored_component: Component = match result? {
            Ok(record) => record.into(),
            Err(ComponentRevisionRepoError::ConcurrentModification) => {
                Err(ComponentError::ConcurrentUpdate {
                    component_id: component_id.clone(),
                    version: current_revision,
                })?
            }
            Err(ComponentRevisionRepoError::VersionAlreadyExists { .. }) => todo!(),
        };

        self.component_compilation
            .enqueue_compilation(
                &environment_id,
                &component_id,
                stored_component.versioned_component_id.version,
            )
            .await;

        if let Some(mut account_usage) = account_usage {
            account_usage.ack();
        };

        Ok(stored_component)
    }
    pub async fn get_component(
        &self,
        component_id: &ComponentId,
    ) -> Result<Component, ComponentError> {
        info!(component_id = %component_id, "Get component");

        let record = self.component_repo.get_staged_by_id(&component_id.0).await?;

        match record {
            Some(record) => Ok(record.into()),
            None => Err(ComponentError::UnknownComponentId(component_id.clone()))
        }
    }

    pub async fn get_component_revision(
        &self,
        component_id: &ComponentId,
        revision: Revision,
    ) -> Result<Component, ComponentError> {
        info!(component_id = %component_id, "Get component revision");

        let record = self.component_repo.get_by_id_and_revision(&component_id.0, revision as i64).await?;

        match record {
            Some(record) => Ok(record.into()),
            None => Err(ComponentError::UnknownComponentId(component_id.clone()))
        }
    }

    // TODO:
    // async fn download(
    //     &self,
    //     component_id: &ComponentId,
    //     version: Option<ComponentVersion>,
    //     owner: &ComponentOwner,
    // ) -> Result<Vec<u8>, ComponentError> {
    //     let component = match version {
    //         None => self.get_latest_version(component_id, owner).await?,
    //         Some(version) => {
    //             self.get_by_version(
    //                 &VersionedComponentId {
    //                     component_id: component_id.clone(),
    //                     version,
    //                 },
    //                 owner,
    //             )
    //             .await?
    //         }
    //     };

    //     if let Some(component) = component {
    //         info!(owner = %owner, component_id = %component.versioned_component_id, "Download component");

    //         self.object_store
    //             .get(
    //                 &component.environment_id,
    //                 &component.transformed_object_store_key(),
    //             )
    //             .await
    //             .tap_err(|e| error!(owner = %owner, "Error downloading component - error: {}", e))
    //             .map_err(|e| {
    //                 ComponentError::component_store_error("Error downloading component", e)
    //             })
    //     } else {
    //         Err(ComponentError::UnknownComponentId(component_id.clone()))
    //     }
    // }

    //TODO:
    // async fn download_stream(
    //     &self,
    //     component_id: &ComponentId,
    //     version: Option<ComponentVersion>,
    //     owner: &ComponentOwner,
    // ) -> Result<BoxStream<'static, Result<Vec<u8>, anyhow::Error>>, ComponentError> {
    //     let component = match version {
    //         None => self.get_latest_version(component_id, owner).await?,
    //         Some(version) => {
    //             self.get_by_version(
    //                 &VersionedComponentId {
    //                     component_id: component_id.clone(),
    //                     version,
    //                 },
    //                 owner,
    //             )
    //             .await?
    //         }
    //     };

    //     if let Some(component) = component {
    //         let protected_object_store_key = component.transformed_object_store_key();

    //         info!(
    //             owner = %owner,
    //             component_id = %component.versioned_component_id,
    //             protected_object_store_key = %protected_object_store_key,
    //             "Download component as stream",
    //         );

    //         self.object_store
    //             .get_stream(&component.environment_id, &protected_object_store_key)
    //             .await
    //             .map_err(|e| {
    //                 ComponentError::component_store_error("Error downloading component", e)
    //             })
    //     } else {
    //         Err(ComponentError::UnknownComponentId(component_id.clone()))
    //     }
    // }

    //TODO:
    // async fn get_file_contents(
    //     &self,
    //     component_id: &ComponentId,
    //     version: ComponentVersion,
    //     path: &str,
    //     owner: &ComponentOwner,
    // ) -> Result<BoxStream<'static, Result<Bytes, ComponentError>>, ComponentError> {
    //     let component = self
    //         .get_by_version(
    //             &VersionedComponentId {
    //                 component_id: component_id.clone(),
    //                 version,
    //             },
    //             owner,
    //         )
    //         .await?;

    //     if let Some(component) = component {
    //         info!(owner = %owner, component_id = %component.versioned_component_id, "Stream component file: {}", path);

    //         let file = component
    //             .files
    //             .iter()
    //             .find(|&file| file.path.to_rel_string() == path)
    //             .ok_or(ComponentError::InvalidFilePath(path.to_string()))?;

    //         let stream = self
    //             .initial_component_files_service
    //             .get(&owner.environment_id, &file.key)
    //             .await?
    //             .ok_or(anyhow!("Failed to find initial component file in store"))?
    //             .map_err(|e| e.context("Failed streaming file data").into());

    //         Ok(Box::pin(stream))
    //     } else {
    //         Err(ComponentError::UnknownComponentId(component_id.clone()))
    //     }
    // }

    // TODO:
    // async fn get_latest_staged_version(
    //     &self,
    //     component_id: &ComponentId,
    // ) -> Result<Option<Component>, ComponentError> {
    //     info!(component_id = %component_id, "Get latest staged version");
    //     let result = self
    //         .component_repo
    //         .get_staged_by_id(&component_id.0)
    //         .await?;

    //     match result {
    //         Some(c) => {
    //             let value = c
    //                 .try_into()
    //                 .map_err(|e| ComponentError::conversion_error("record", e))?;
    //             Ok(Some(value))
    //         }
    //         None => Ok(None),
    //     }
    // }

    // TODO:
    // async fn get_owner(
    //     &self,
    //     component_id: &ComponentId,
    // ) -> Result<Option<ComponentOwner>, ComponentError> {
    //     info!(component_id = %component_id, "Get component owner");
    //     let result = self.component_repo.get_namespace(component_id.0).await?;
    //     if let Some(result) = result {
    //         let value = result
    //             .parse()
    //             .map_err(|e| ComponentError::conversion_error("namespace", e))?;
    //         Ok(Some(value))
    //     } else {
    //         Ok(None)
    //     }
    // }

    // TODO:
    // async fn delete(
    //     &self,
    //     component_id: &ComponentId,
    //     owner: &ComponentOwner,
    // ) -> Result<(), ComponentError> {
    //     info!(owner = %owner, component_id = %component_id, "Delete component");

    //     let records = self
    //         .component_repo
    //         .get(&owner.to_string(), component_id.0)
    //         .await?;
    //     let components = records
    //         .iter()
    //         .map(|c| c.clone().try_into())
    //         .collect::<Result<Vec<Component>, _>>()
    //         .map_err(|e| ComponentError::conversion_error("record", e))?;

    //     let mut object_store_keys = Vec::new();

    //     for component in components {
    //         object_store_keys.push((
    //             component.owner.project_id.clone(),
    //             component.transformed_object_store_key(),
    //         ));
    //         object_store_keys.push((
    //             component.owner.project_id.clone(),
    //             component.user_object_store_key(),
    //         ));
    //     }

    //     if !object_store_keys.is_empty() {
    //         for (environment_id, key) in object_store_keys {
    //             self.object_store
    //                 .delete(&environment_id, &key)
    //                 .await
    //                 .context("Failed to delete component data")?
    //         }
    //         self.component_repo
    //             .delete(&owner.to_string(), component_id.0)
    //             .await?;
    //         Ok(())
    //     } else {
    //         Err(ComponentError::UnknownComponentId(component_id.clone()))
    //     }
    // }

    // TODO:
    // async fn create_or_update_constraint(
    //     &self,
    //     component_constraint: &ComponentConstraints,
    // ) -> Result<ComponentConstraints, ComponentError> {
    //     info!(owner = %component_constraint.owner, component_id = %component_constraint.component_id, "Create or update component constraint");
    //     let component_id = &component_constraint.component_id;
    //     let record = ComponentConstraintsRecord::try_from(component_constraint.clone())
    //         .map_err(|e| ComponentError::conversion_error("record", e))?;

    //     self.component_repo
    //         .create_or_update_constraint(&record)
    //         .await?;

    //     let result = self
    //         .component_repo
    //         .get_constraint(
    //             &component_constraint.owner.to_string(),
    //             component_constraint.component_id.0,
    //         )
    //         .await?
    //         .ok_or(ComponentError::ComponentConstraintCreateError(format!(
    //             "Failed to create constraints for {component_id}"
    //         )))?;

    //     let component_constraints = ComponentConstraints {
    //         component_id: component_id.clone(),
    //         constraints: result,
    //     };

    //     Ok(component_constraints)
    // }

    // TODO:
    // async fn delete_constraints(
    //     &self,
    //     owner: &ComponentOwner,
    //     component_id: &ComponentId,
    //     constraints_to_remove: &[FunctionSignature],
    // ) -> Result<ComponentConstraints, ComponentError> {
    //     info!(owner = %owner, component_id = %component_id, "Delete constraint");

    //     self.component_repo
    //         .delete_constraints(&owner.to_string(), component_id.0, constraints_to_remove)
    //         .await?;

    //     let result = self
    //         .component_repo
    //         .get_constraint(&owner.to_string(), component_id.0)
    //         .await?
    //         .ok_or(ComponentError::ComponentConstraintCreateError(format!(
    //             "Failed to get constraints for {component_id}"
    //         )))?;

    //     let component_constraints = ComponentConstraints {
    //         component_id: component_id.clone(),
    //         constraints: result,
    //     };

    //     Ok(component_constraints)
    // }

    // TODO:
    // async fn get_component_constraint(
    //     &self,
    //     component_id: &ComponentId,
    //     owner: &ComponentOwner,
    // ) -> Result<Option<FunctionConstraints>, ComponentError> {
    //     info!(component_id = %component_id, "Get component constraint");

    //     let result = self
    //         .component_repo
    //         .get_constraint(&owner.to_string(), component_id.0)
    //         .await?;
    //     Ok(result)
    // }

    // TODO:
    // async fn get_plugin_installations_for_component(
    //     &self,
    //     owner: &ComponentOwner,
    //     component_id: &ComponentId,
    //     component_version: ComponentVersion,
    // ) -> Result<Vec<PluginInstallation>, ComponentError> {
    //     let owner_record: ComponentOwnerRow = owner.clone().into();
    //     let plugin_owner_record = owner_record.into();
    //     let records = self
    //         .component_repo
    //         .get_installed_plugins(&plugin_owner_record, component_id.0, component_version)
    //         .await?;

    //     records
    //         .into_iter()
    //         .map(PluginInstallation::try_from)
    //         .collect::<Result<Vec<_>, _>>()
    //         .map_err(|e| ComponentError::conversion_error("plugin installation", e))
    // }

    // TODO:
    // async fn create_plugin_installation_for_component(
    //     &self,
    //     owner: &ComponentOwner,
    //     component_id: &ComponentId,
    //     installation: PluginInstallationCreation,
    // ) -> Result<PluginInstallation, ComponentError> {
    //     let result = self
    //         .batch_update_plugin_installations_for_component(
    //             owner,
    //             component_id,
    //             &[PluginInstallationAction::Install(installation)],
    //         )
    //         .await?;
    //     Ok(result.into_iter().next().unwrap().unwrap())
    // }

    // TODO:
    // async fn update_plugin_installation_for_component(
    //     &self,
    //     owner: &ComponentOwner,
    //     installation_id: &PluginInstallationId,
    //     component_id: &ComponentId,
    //     update: PluginInstallationUpdate,
    // ) -> Result<(), ComponentError> {
    //     let _ = self
    //         .batch_update_plugin_installations_for_component(
    //             owner,
    //             component_id,
    //             &[PluginInstallationAction::Update(
    //                 PluginInstallationUpdateWithId {
    //                     installation_id: installation_id.clone(),
    //                     priority: update.priority,
    //                     parameters: update.parameters,
    //                 },
    //             )],
    //         )
    //         .await?;
    //     Ok(())
    // }

    // TODO:
    // async fn delete_plugin_installation_for_component(
    //     &self,
    //     owner: &ComponentOwner,
    //     installation_id: &PluginInstallationId,
    //     component_id: &ComponentId,
    // ) -> Result<(), ComponentError> {
    //     let _ = self
    //         .batch_update_plugin_installations_for_component(
    //             owner,
    //             component_id,
    //             &[PluginInstallationAction::Uninstall(PluginUninstallation {
    //                 installation_id: installation_id.clone(),
    //             })],
    //         )
    //         .await;
    //     Ok(())
    // }

    // TODO:
    // async fn batch_update_plugin_installations_for_component(
    //     &self,
    //     owner: &ComponentOwner,
    //     component_id: &ComponentId,
    //     actions: &[PluginInstallationAction],
    // ) -> Result<Vec<Option<PluginInstallation>>, ComponentError> {
    //     let mut component: Component = self
    //         .get_latest_version(component_id, owner)
    //         .await?
    //         .ok_or(ComponentError::UnknownComponentId(component_id.clone()))?;

    //     component.bump_version();
    //     // reuse the untransformed object store key from the previous version
    //     component.regenerate_transformed_object_store_key();
    //     component.reset_transformations();

    //     let mut result = Vec::new();

    //     for action in actions {
    //         match action {
    //             PluginInstallationAction::Install(installation) => {
    //                 let plugin_definition = self
    //                     .plugin_service
    //                     .get(
    //                         &PluginOwner::from(owner.clone()),
    //                         &installation.name,
    //                         &installation.version,
    //                     )
    //                     .await?
    //                     .ok_or(ComponentError::PluginNotFound {
    //                         account_id: owner.account_id.clone(),
    //                         plugin_name: installation.name.clone(),
    //                         plugin_version: installation.version.clone(),
    //                     })?;

    //                 {
    //                     let installation_allowed = plugin_definition
    //                         .scope
    //                         .valid_in_component(component_id, &owner.environment_id);

    //                     if !installation_allowed {
    //                         Err(ComponentError::InvalidPluginScope {
    //                             plugin_name: installation.name.clone(),
    //                             plugin_version: installation.version.clone(),
    //                             details: format!("not available for component {}", component_id.0),
    //                         })?
    //                     };
    //                 }

    //                 let plugin_installation = PluginInstallation {
    //                     id: PluginInstallationId::new_v4(),
    //                     plugin_id: plugin_definition.id,
    //                     priority: installation.priority,
    //                     parameters: installation.parameters.clone(),
    //                 };

    //                 component
    //                     .installed_plugins
    //                     .push(plugin_installation.clone());

    //                 result.push(Some(plugin_installation));
    //             }
    //             PluginInstallationAction::Update(update) => {
    //                 let existing = component
    //                     .installed_plugins
    //                     .iter_mut()
    //                     .find(|ip| ip.id == update.installation_id);

    //                 let Some(existing) = existing else {
    //                     Err(ComponentError::PluginInstallationNotFound {
    //                         installation_id: update.installation_id.clone(),
    //                     })?
    //                 };

    //                 existing.priority = update.priority;
    //                 existing.parameters = update.parameters.clone();
    //                 result.push(None);
    //             }
    //             PluginInstallationAction::Uninstall(uninstallation) => {
    //                 result.push(None);

    //                 let len_before = component.installed_plugins.len();
    //                 component
    //                     .installed_plugins
    //                     .retain(|ip| ip.id != uninstallation.installation_id);

    //                 if component.installed_plugins.len() == len_before {
    //                     // we failed to find the installation
    //                     Err(ComponentError::PluginInstallationNotFound {
    //                         installation_id: uninstallation.installation_id.clone(),
    //                     })?
    //                 };
    //             }
    //         }
    //     }

    //     let data = self
    //         .object_store
    //         .get(
    //             &component.environment_id,
    //             &component.user_object_store_key(),
    //         )
    //         .await
    //         .map_err(|err| {
    //             ComponentError::component_store_error("Failed to download user component", err)
    //         })?;

    //     let (component, transformed_data) = self.apply_transformations(component, data).await?;

    //     self.object_store
    //         .put(
    //             &component.environment_id,
    //             &component.transformed_object_store_key(),
    //             transformed_data,
    //         )
    //         .await
    //         .map_err(|e| {
    //             ComponentError::component_store_error("Failed to upload protected component", e)
    //         })?;

    //     let record = ComponentRecord::try_from_model(component.clone())
    //         .map_err(|e| ComponentError::conversion_error("record", e))?;

    //     let create_result = self.component_repo.create(&record).await;

    //     match create_result {
    //         Err(RepoError::UniqueViolation(_)) => Err(ComponentError::ConcurrentUpdate {
    //             component_id: component_id.clone(),
    //             version: component.versioned_component_id.version,
    //         })?,
    //         Err(other) => Err(other)?,
    //         Ok(()) => {}
    //     };

    //     Ok(result)
    // }
    async fn upload_user_component(
        &self,
        component: &Component,
        data: Vec<u8>,
    ) -> Result<(), ComponentError> {
        self.object_store
            .put(
                &component.environment_id,
                &component.full_object_store_key(),
                data,
            )
            .await?;
        Ok(())
    }

    async fn upload_transformed_component(
        &self,
        component: &Component,
        data: Vec<u8>,
    ) -> Result<(), ComponentError> {
        self.object_store
            .put(
                &component.environment_id,
                &component.full_transformed_object_store_key(),
                data,
            )
            .await?;
        Ok(())
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
                key,
                permissions: options.permissions
            });
        }

        Ok(result)
    }

    async fn initial_component_files_for_updated_component(
        &self,
        environment_id: &EnvironmentId,
        previous: Vec<InitialComponentFile>,
        removed_files: Vec<ComponentFilePath>,
        new_files_archive: Option<NamedTempFile>,
        new_file_options: BTreeMap<ComponentFilePath, ComponentFileOptions>
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
            result.insert(path.clone(), InitialComponentFile { key, path, permissions: ComponentFilePermissions::default() });
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
    ) -> Result<HashMap<ComponentFilePath, InitialComponentFileKey>, ComponentError> {

        let to_upload =
            self::utils::prepare_component_files_for_upload(archive).await?;

        let tasks = to_upload
            .into_iter()
            .map(|(path, stream)| async move {
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

    // TODO:
    // async fn apply_transformations(
    //     &self,
    //     mut component: Component,
    //     mut data: Vec<u8>,
    // ) -> Result<(Component, Vec<u8>), ComponentError> {
    //     if !component.installed_plugins.is_empty() {
    //         let mut installed_plugins = component.installed_plugins.clone();
    //         installed_plugins.sort_by_key(|p| p.priority);

    //         for installation in installed_plugins {
    //             let plugin = self
    //                 .plugin_service
    //                 .get_by_id(&installation.plugin_id)
    //                 .await?
    //                 .expect("Failed to resolve plugin by id");

    //             match plugin.specs {
    //                 PluginTypeSpecificDefinition::ComponentTransformer(spec) => {
    //                     let span = info_span!("component transformation",
    //                         owner = %component.owner,
    //                         component_id = %component.versioned_component_id,
    //                         plugin_id = %installation.plugin_id,
    //                         plugin_installation_id = %installation.id,
    //                     );

    //                     (component, data) = self
    //                         .apply_component_transformer_plugin(
    //                             component,
    //                             data,
    //                             spec.transform_url,
    //                             &installation.parameters,
    //                         )
    //                         .instrument(span)
    //                         .await?;
    //                 }
    //                 PluginTypeSpecificDefinition::Library(spec) => {
    //                     let span = info_span!("library plugin",
    //                         owner = %component.owner,
    //                         component_id = %component.versioned_component_id,
    //                         plugin_id = %installation.plugin_id,
    //                         plugin_installation_id = %installation.id,
    //                     );
    //                     data = self
    //                         .apply_library_plugin(&component, &data, spec)
    //                         .instrument(span)
    //                         .await?;
    //                 }
    //                 PluginTypeSpecificDefinition::App(spec) => {
    //                     let span = info_span!("app plugin",
    //                         owner = %component.owner,
    //                         component_id = %component.versioned_component_id,
    //                         plugin_id = %installation.plugin_id,
    //                         plugin_installation_id = %installation.id,
    //                     );
    //                     data = self
    //                         .apply_app_plugin(&component, &data, spec)
    //                         .instrument(span)
    //                         .await?;
    //                 }
    //                 PluginTypeSpecificDefinition::OplogProcessor(_) => (),
    //             }
    //         }
    //     }

    //     component.metadata = ComponentMetadata::analyse_component(
    //         &data,
    //         component.metadata.dynamic_linking,
    //         component.metadata.agent_types,
    //     )
    //     .map_err(ComponentError::ComponentProcessingError)?;

    //     Ok((component, data))
    // }

    // async fn apply_component_transformer_plugin(
    //     &self,
    //     mut component: Component,
    //     data: Vec<u8>,
    //     url: String,
    //     parameters: &HashMap<String, String>,
    // ) -> Result<(Component, Vec<u8>), ComponentError> {
    //     info!(%url, "Applying component transformation plugin");

    //     let response = self
    //         .transformer_plugin_caller
    //         .call_remote_transformer_plugin(&component, &data, url, parameters)
    //         .await
    //         .map_err(ComponentError::TransformationFailed)?;

    //     let data = response.data.map(|b64| b64.0).unwrap_or(data);

    //     for (k, v) in response.env.unwrap_or_default() {
    //         component.transformed_env.insert(k, v);
    //     }

    //     let mut files = component.transformed_files;
    //     for file in response.additional_files.unwrap_or_default() {
    //         let content_stream = Bytes::from(file.content.0)
    //             .map_item(|i| i.map_err(widen_infallible::<String>))
    //             .map_error(widen_infallible::<String>);

    //         let key = self
    //             .initial_component_files_service
    //             .put_if_not_exists(&component.owner.project_id, content_stream)
    //             .await
    //             .map_err(|e| {
    //                 ComponentError::initial_component_file_upload_error(
    //                     "Failed to upload component files",
    //                     e,
    //                 )
    //             })?;

    //         let item = InitialComponentFile {
    //             key,
    //             path: file.path,
    //             permissions: file.permissions,
    //         };

    //         files.retain_mut(|f| f.path != item.path);
    //         files.push(item)
    //     }
    //     component.transformed_files = files;

    //     Ok((component, data))
    // }

    // async fn apply_library_plugin(
    //     &self,
    //     component: &Component,
    //     data: &[u8],
    //     plugin_spec: LibraryPluginDefinition,
    // ) -> Result<Vec<u8>, ComponentError> {
    //     info!(%component.versioned_component_id, "Applying library plugin");

    //     let plug_bytes = self
    //         .plugin_wasm_files_service
    //         .get(&component.owner.account_id, &plugin_spec.blob_storage_key)
    //         .await
    //         .map_err(|e| {
    //             ComponentError::PluginApplicationFailed(format!("Failed to fetch plugin wasm: {e}"))
    //         })?
    //         .ok_or(ComponentError::PluginApplicationFailed(
    //             "Plugin data not found".to_string(),
    //         ))?;

    //     let composed = self::utils::compose_components(data, &plug_bytes).map_err(|e| {
    //         ComponentError::PluginApplicationFailed(format!(
    //             "Failed to compose plugin with component: {e}"
    //         ))
    //     })?;

    //     Ok(composed)
    // }

    // async fn apply_app_plugin(
    //     &self,
    //     component: &Component,
    //     data: &[u8],
    //     plugin_spec: AppPluginDefinition,
    // ) -> Result<Vec<u8>, ComponentError> {
    //     info!(%component.versioned_component_id, "Applying app plugin");

    //     let socket_bytes = self
    //         .plugin_wasm_files_service
    //         .get(&component.environment_id, &plugin_spec.blob_storage_key)
    //         .await
    //         .map_err(|e| {
    //             ComponentError::PluginApplicationFailed(format!("Failed to fetch plugin wasm: {e}"))
    //         })?
    //         .ok_or(ComponentError::PluginApplicationFailed(
    //             "Plugin data not found".to_string(),
    //         ))?;

    //     let composed = self::utils::compose_components(&socket_bytes, data).context("Failed to compose plugin with component")?;

    //     Ok(composed)
    // }
}

impl Debug for ComponentService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentServiceDefault").finish()
    }
}
