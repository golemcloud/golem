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
use crate::repo::model::card::CardRecord;
use crate::repo::model::component::{ComponentRepoError, ComponentRevisionRecord};
use crate::services::account_usage::AccountUsageService;
use crate::services::component::utils::prepare_component_files_for_upload;
use crate::services::component_compilation::ComponentCompilationService;
use crate::services::component_object_store::ComponentObjectStore;
use crate::services::deployment::authorize_environment_permission;
use crate::services::environment::EnvironmentError;
use crate::services::environment::EnvironmentService;
use crate::services::environment_plugin_grant::{
    EnvironmentPluginGrantError, EnvironmentPluginGrantService,
};
use crate::services::registry_change_notifier::{
    RegistryChangeNotifier, RequiresNotificationSignalExt,
};
use crate::services::run_cpu_bound_work;
use anyhow::Context;
use golem_common::base_model::component_metadata::{
    AgentTypeProvisionConfig, ComponentProvisionConfig,
};
use golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantWithDetails;
use golem_common::model::agent::AgentConfigSource;
use golem_common::model::agent::{AgentFileContentHash, AgentTypeName, InitialAgentFileUpload};
use golem_common::model::card::owner::ComponentOwnerPattern;
use golem_common::model::card::{
    CardManagedBy, CardManagedByAgentInitial, CardManagedByComponentInitial, ClassPermissionTarget,
    ComponentResourcePattern, ComponentVerb, DelegationSurface, EnvironmentVerb, PermissionTarget,
    PolymorphicCard, negative_permission_envelopes_for_recipient_patterns,
    permission_envelopes_for_recipient_patterns,
};
use golem_common::model::component::{
    AgentFilePath, ArchiveFilePath, ComponentCreation, ComponentId, ComponentName,
    ComponentRevision, ComponentUpdate, InitialAgentFile, InstalledPlugin, PluginInstallation,
    PluginInstallationAction, ToolDeploymentConfigCreation, ToolDeploymentConfigUpdate,
    ToolProvisionConfigCreation, ToolProvisionConfigUpdate,
};
use golem_common::model::component::{
    AgentTypeProvisionConfigCreation, AgentTypeProvisionConfigUpdate,
};
use golem_common::model::component_metadata::{ComponentMetadata, ComponentProcessingError};
use golem_common::model::diff::Hash;
use golem_common::model::environment::{Environment, EnvironmentId};
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::model::tool::{ToolDeploymentMetadata, ToolName, ToolProvisionConfig};
use golem_common::model::worker::AgentConfigEntryDto;
use golem_common::model::worker::TypedAgentConfigEntry;
use golem_common::schema::SchemaValue;
use golem_common::schema::agent::{
    AgentTypeSchema, ComponentConfigSchema, typed_schema_value_with_projected_defs,
};
use golem_common::schema::tool::Tool;
use golem_common::schema::tool::validation::validate_tool;
use golem_common::schema::validation::{is_equivalent_cross_graph, validate_graph, validate_value};
use golem_schema::schema::render::from_untrusted_json_value;
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
    object_store: Arc<ComponentObjectStore>,
    component_compilation: Arc<dyn ComponentCompilationService>,
    initial_agent_files_service: Arc<InitialAgentFilesService>,
    account_usage_service: Arc<AccountUsageService>,
    environment_service: Arc<EnvironmentService>,
    environment_plugin_grant_service: Arc<EnvironmentPluginGrantService>,
    registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
}

impl ComponentWriteService {
    pub fn new(
        component_repo: Arc<dyn ComponentRepo>,
        object_store: Arc<ComponentObjectStore>,
        component_compilation: Arc<dyn ComponentCompilationService>,
        initial_agent_files_service: Arc<InitialAgentFilesService>,
        account_usage_service: Arc<AccountUsageService>,
        environment_service: Arc<EnvironmentService>,
        environment_plugin_grant_service: Arc<EnvironmentPluginGrantService>,
        registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
    ) -> Self {
        Self {
            component_repo,
            object_store,
            component_compilation,
            initial_agent_files_service,
            account_usage_service,
            environment_service,
            environment_plugin_grant_service,
            registry_change_notifier,
        }
    }

    pub async fn upload_initial_agent_file(
        &self,
        environment_id: EnvironmentId,
        data: Arc<NamedTempFile>,
        auth: &AuthCtx,
    ) -> Result<InitialAgentFileUpload, ComponentError> {
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

        let size = data.length().await?;
        let stream = data.map_item(|item| item.map(|bytes| bytes.to_vec()));
        store_initial_agent_file_stream(
            self.initial_agent_files_service.as_ref(),
            &environment,
            stream,
            size,
            auth,
        )
        .await
    }

    fn prepare_initial_permission_card_record(
        &self,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        agent_type_name: &AgentTypeName,
        initial_permissions: &golem_common::model::component::AgentTypeInitialPermissions,
        auth: &AuthCtx,
    ) -> Result<(PolymorphicCard, CardRecord), ComponentError> {
        let delegation_surface =
            auth.delegation_surface_for_card_derivation("create agent initial permission card")?;
        let card = prepare_agent_initial_card_for_minting(
            agent_type_name,
            initial_permissions,
            delegation_surface,
        )?;
        let record = CardRecord::polymorphic_creation(
            card,
            Some(CardManagedBy::AgentInitial(CardManagedByAgentInitial {
                component_id,
                component_revision,
                agent_type: agent_type_name.clone(),
            })),
        );
        let card = record.clone().try_into()?;

        Ok((card, record))
    }

    fn prepare_component_initial_permission_card_record(
        &self,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        initial_permissions: &golem_common::model::component::AgentTypeInitialPermissions,
        auth: &AuthCtx,
    ) -> Result<(PolymorphicCard, CardRecord), ComponentError> {
        let delegation_surface = auth
            .delegation_surface_for_card_derivation("create component initial permission card")?;
        let card = prepare_initial_card_for_minting(initial_permissions, delegation_surface)
            .map_err(|message| ComponentError::InvalidComponentInitialPermissionCard { message })?;
        let record = CardRecord::polymorphic_creation(
            card,
            Some(CardManagedBy::ComponentInitial(
                CardManagedByComponentInitial {
                    component_id,
                    component_revision,
                },
            )),
        );
        let card = record.clone().try_into()?;
        Ok((card, record))
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
            .component_provision_config
            .files
            .keys()
            .cloned()
            .chain(
                component_creation
                    .agent_type_provision_configs
                    .values()
                    .flat_map(|c| c.files.keys().cloned())
                    .chain(
                        component_creation
                            .tool_deployment_configs
                            .values()
                            .flat_map(|config| config.provision.files.keys().cloned()),
                    ),
            )
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
            .component_provision_config
            .plugin_installations
            .iter()
            .map(|p| p.environment_plugin_grant_id)
            .chain(
                component_creation
                    .agent_type_provision_configs
                    .values()
                    .flat_map(|c| {
                        c.plugin_installations
                            .iter()
                            .map(|p| p.environment_plugin_grant_id)
                    })
                    .chain(
                        component_creation
                            .tool_deployment_configs
                            .values()
                            .flat_map(|config| {
                                config
                                    .provision
                                    .plugin_installations
                                    .iter()
                                    .map(|plugin| plugin.environment_plugin_grant_id)
                            }),
                    ),
            )
            .collect();
        let resolved_grants = self
            .resolve_all_plugin_grants(&environment, all_grant_ids, auth)
            .await?;

        let mut provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig> =
            BTreeMap::new();
        let mut cards_to_create = Vec::new();

        validate_component_config_schema(
            &component_creation.config_schema,
            &component_creation.agent_types,
        )?;
        let component_config = validate_and_transform_component_config_entries(
            &component_creation.config_schema,
            component_creation.component_provision_config.config.clone(),
        )?;
        let component_files = resolve_component_files_for_creation(
            &component_creation.component_provision_config.files,
            &uploaded_files,
        )?;
        let component_plugins = resolve_plugins_for_creation(
            &component_creation
                .component_provision_config
                .plugin_installations,
            &resolved_grants,
        )?;
        let (component_initial_permissions, component_card_record) = self
            .prepare_component_initial_permission_card_record(
                component_id,
                ComponentRevision::INITIAL,
                &component_creation
                    .component_provision_config
                    .initial_permissions,
                auth,
            )?;
        cards_to_create.push(component_card_record);
        let component_provision_config = ComponentProvisionConfig {
            initial_permissions: component_initial_permissions,
            env: component_creation.component_provision_config.env.clone(),
            config: component_config,
            plugins: component_plugins,
            files: component_files,
        };

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
            let (initial_permission, card_record) = self.prepare_initial_permission_card_record(
                component_id,
                ComponentRevision::INITIAL,
                agent_type_name,
                &creation.initial_permissions,
                auth,
            )?;
            cards_to_create.push(card_record);

            provision_configs.insert(
                agent_type_name.clone(),
                AgentTypeProvisionConfig {
                    initial_permissions: initial_permission,
                    env: creation.env.clone(),
                    config,
                    plugins,
                    files,
                },
            );
        }

        let tool_deployment_metadata = resolve_tool_deployment_metadata_for_creation(
            component_creation.tools,
            component_creation.tool_deployment_configs,
            &uploaded_files,
            &resolved_grants,
        )?;

        let component_metadata = analyze_and_validate_component_wasm(
            component_creation.config_schema,
            component_provision_config,
            component_creation.agent_types,
            wasm.clone(),
            provision_configs,
            tool_deployment_metadata,
        )
        .await?;
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
                cards_to_create,
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

        let ComponentUpdate {
            current_revision,
            config_schema,
            component_provision_config,
            agent_types: agent_type_update,
            agent_type_provision_config_updates,
            tools: tool_update,
            tool_deployment_config_updates,
            allow_incompatible_config,
        } = component_update;

        if allow_incompatible_config && environment.compatibility_check {
            return Err(ComponentError::ResetOverrideRequiresCompatibilityCheckDisabled);
        }

        let mut component = component_record.try_into_model()?;

        if current_revision != component.revision {
            Err(ComponentError::ConcurrentUpdate)?
        };

        component.revision = component.revision.next()?;

        let environment_id = component.environment_id;
        let component_id = component.id;

        info!(environment_id = %environment_id, "Update component");

        let agent_types_changed = agent_type_update.is_some();
        let tools_changed = tool_update.is_some();

        // When no agent type update is supplied, fall back to the schema-native
        // agent types already stored on the existing component metadata.
        let agent_types = match agent_type_update {
            Some(agent_types) => agent_types,
            None => component.metadata.agent_types().to_vec(),
        };
        let final_config_schema =
            config_schema.unwrap_or_else(|| component.metadata.config_schema().clone());
        validate_component_config_schema(&final_config_schema, &agent_types)?;
        let pending_component_provision_config = component_provision_config;
        let mut final_component_provision_config =
            component.metadata.component_provision_config().clone();

        let (tool_definitions, mut final_tool_deployment_metadata) =
            tool_state_for_update(component.metadata.tools(), tool_update)?;

        let mut final_provision_configs = component.metadata.agent_type_provision_configs().clone();
        let mut cards_to_create = Vec::new();
        if agent_types_changed {
            final_provision_configs =
                provision_configs_for_agent_types(&agent_types, final_provision_configs);
        }

        let referenced_paths: HashSet<ArchiveFilePath> = pending_component_provision_config
            .iter()
            .flat_map(|config| config.files.keys().cloned())
            .chain(
                agent_type_provision_config_updates
                    .iter()
                    .flat_map(|updates| updates.values())
                    .flat_map(|update| update.files_to_add_or_update.keys().cloned())
                    .chain(
                        tool_deployment_config_updates
                            .iter()
                            .flat_map(|updates| updates.values())
                            .filter_map(|update| update.provision.as_ref())
                            .flat_map(|update| update.files_to_add_or_update.keys().cloned()),
                    ),
            )
            .collect();
        let uploaded_files = match new_files_archive {
            Some(archive) => {
                self.upload_agent_files(environment_id, archive, &referenced_paths)
                    .await?
            }
            None => HashMap::new(),
        };

        if let Some(config) = pending_component_provision_config {
            let grant_ids: HashSet<_> = config
                .plugin_installations
                .iter()
                .map(|plugin| plugin.environment_plugin_grant_id)
                .collect();
            let grants = self
                .resolve_all_plugin_grants(&environment, grant_ids, auth)
                .await?;
            let (initial_permissions, card_record) = self
                .prepare_component_initial_permission_card_record(
                    component_id,
                    component.revision,
                    &config.initial_permissions,
                    auth,
                )?;
            cards_to_create.push(card_record);
            final_component_provision_config = ComponentProvisionConfig {
                initial_permissions,
                env: config.env,
                config: validate_and_transform_component_config_entries(
                    &final_config_schema,
                    config.config,
                )?,
                plugins: resolve_plugins_for_creation(&config.plugin_installations, &grants)?,
                files: resolve_component_files_for_creation(&config.files, &uploaded_files)?,
            };
        } else {
            final_component_provision_config.config =
                validate_and_transform_component_config_entries(
                    &final_config_schema,
                    final_component_provision_config
                        .config
                        .into_iter()
                        .map(AgentConfigEntryDto::from)
                        .collect(),
                )?;
        }

        let mut provision_configs_changed = false;

        if let Some(updates) = agent_type_provision_config_updates {
            provision_configs_changed = true;

            for (agent_type_name, update) in updates {
                let agent_type = agent_types
                    .iter()
                    .find(|agent_type| agent_type.type_name == agent_type_name)
                    .ok_or_else(|| {
                        ComponentError::UndeclaredAgentTypeInProvisionConfig(
                            agent_type_name.clone(),
                        )
                    })?;

                let updated = if let Some(existing) =
                    final_provision_configs.get(&agent_type_name).cloned()
                {
                    self.apply_provision_config_update(
                        &agent_type_name,
                        component.id,
                        component.revision,
                        existing,
                        update,
                        &uploaded_files,
                        agent_type,
                        &environment,
                        auth,
                        &mut cards_to_create,
                    )
                    .await?
                } else {
                    self.create_provision_config_from_update(
                        &agent_type_name,
                        component.id,
                        component.revision,
                        update,
                        &uploaded_files,
                        agent_type,
                        &environment,
                        auth,
                        &mut cards_to_create,
                    )
                    .await?
                };

                final_provision_configs.insert(agent_type_name, updated);
            }
        }

        let mut tool_deployment_configs_changed = false;
        if let Some(updates) = tool_deployment_config_updates {
            tool_deployment_configs_changed = true;
            for (tool_name, update) in updates {
                let definition = tool_definitions.get(&tool_name).ok_or_else(|| {
                    ComponentError::UndeclaredToolInDeploymentConfig(tool_name.clone())
                })?;
                let existing = final_tool_deployment_metadata.remove(&tool_name);
                let updated = self
                    .apply_tool_deployment_config_update(
                        &tool_name,
                        definition.clone(),
                        existing,
                        update,
                        &uploaded_files,
                        &environment,
                        auth,
                    )
                    .await?;
                final_tool_deployment_metadata.insert(tool_name, updated);
            }
        }

        for tool_name in tool_definitions.keys() {
            if !final_tool_deployment_metadata.contains_key(tool_name) {
                return Err(ComponentError::MissingToolDeploymentConfig(
                    tool_name.clone(),
                ));
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
                final_config_schema.clone(),
                final_component_provision_config.clone(),
                agent_types,
                new_wasm.clone(),
                final_provision_configs,
                final_tool_deployment_metadata,
            )
            .await?;
            component.metadata = metadata;
        } else if agent_types_changed || tools_changed {
            // TODO: skip the download here
            let old_data = self
                .object_store
                .get(environment_id, &component.object_store_key)
                .await?;

            let metadata = analyze_and_validate_component_wasm(
                final_config_schema.clone(),
                final_component_provision_config.clone(),
                agent_types,
                Arc::from(old_data),
                final_provision_configs,
                final_tool_deployment_metadata,
            )
            .await?;
            component.metadata = metadata;
        } else {
            component.metadata = component
                .metadata
                .with_component_config(final_config_schema, final_component_provision_config);
            if provision_configs_changed {
                component.metadata = component
                    .metadata
                    .with_provision_configs(final_provision_configs);
            }
            if tool_deployment_configs_changed {
                component.metadata = component
                    .metadata
                    .with_tools(final_tool_deployment_metadata);
            }
        }

        validate_component_metadata_invariants(&component.metadata)?;

        let record = ComponentRevisionRecord::from_model(component, auth.actor_account_id());

        let stored_component: Component = self
            .component_repo
            .update(record, cards_to_create)
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
                ComponentRepoError::ComponentSourceInUse => {
                    ComponentError::ComponentSourceInUse(component_id)
                }
                other => other.into(),
            })?
            .signal_new_events_available(self.registry_change_notifier.as_ref());

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
        component_id: ComponentId,
        component_revision: ComponentRevision,
        existing: AgentTypeProvisionConfig,
        update: AgentTypeProvisionConfigUpdate,
        uploaded_files: &HashMap<ArchiveFilePath, (AgentFileContentHash, u64)>,
        agent_type: &AgentTypeSchema,
        environment: &Environment,
        auth: &AuthCtx,
        cards_to_create: &mut Vec<CardRecord>,
    ) -> Result<AgentTypeProvisionConfig, ComponentError> {
        let initial_permission = if let Some(initial_permission_update) = update.initial_permissions
        {
            let (initial_permission, card_record) = self.prepare_initial_permission_card_record(
                component_id,
                component_revision,
                agent_type_name,
                &initial_permission_update,
                auth,
            )?;
            cards_to_create.push(card_record);
            initial_permission
        } else {
            existing.initial_permissions
        };

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
            initial_permissions: initial_permission,
            env,
            config,
            plugins,
            files,
        })
    }

    async fn create_provision_config_from_update(
        &self,
        agent_type_name: &AgentTypeName,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        update: AgentTypeProvisionConfigUpdate,
        uploaded_files: &HashMap<ArchiveFilePath, (AgentFileContentHash, u64)>,
        agent_type: &AgentTypeSchema,
        environment: &Environment,
        auth: &AuthCtx,
        cards_to_create: &mut Vec<CardRecord>,
    ) -> Result<AgentTypeProvisionConfig, ComponentError> {
        let Some(ref initial_permission_update) = update.initial_permissions else {
            return Err(ComponentError::NewAgentTypeMissingInitialPermissions(
                agent_type_name.clone(),
            ));
        };
        let (initial_permission, card_record) = self.prepare_initial_permission_card_record(
            component_id,
            component_revision,
            agent_type_name,
            initial_permission_update,
            auth,
        )?;
        cards_to_create.push(card_record);

        let files = resolve_files_for_update(agent_type_name, &update, uploaded_files)?;

        let config =
            validate_and_transform_config_entries(agent_type, update.config.unwrap_or_default())?;

        let plugins = self
            .update_plugin_installations(environment, Vec::new(), update.plugin_updates, auth)
            .await?;

        Ok(AgentTypeProvisionConfig {
            initial_permissions: initial_permission,
            env: update.env.unwrap_or_default(),
            config,
            plugins,
            files,
        })
    }

    async fn apply_tool_deployment_config_update(
        &self,
        tool_name: &ToolName,
        definition: Tool,
        existing: Option<ToolDeploymentMetadata>,
        update: ToolDeploymentConfigUpdate,
        uploaded_files: &HashMap<ArchiveFilePath, (AgentFileContentHash, u64)>,
        environment: &Environment,
        auth: &AuthCtx,
    ) -> Result<ToolDeploymentMetadata, ComponentError> {
        let existing_provision = existing.as_ref().map(|metadata| metadata.provision.clone());
        let provision = match (existing_provision, update.provision) {
            (Some(existing), Some(update)) => {
                let files = resolve_tool_files_for_update(
                    tool_name,
                    existing.files,
                    &update,
                    uploaded_files,
                )?;
                let config = update.config.unwrap_or(existing.config);
                let env = update.env.unwrap_or(existing.env);
                let plugins = self
                    .update_plugin_installations(
                        environment,
                        existing.plugins,
                        update.plugin_updates,
                        auth,
                    )
                    .await?;
                ToolProvisionConfig {
                    config,
                    env,
                    plugins,
                    files,
                }
            }
            (Some(existing), None) => existing,
            (None, Some(update)) => {
                let files =
                    resolve_tool_files_for_update(tool_name, Vec::new(), &update, uploaded_files)?;
                let plugins = self
                    .update_plugin_installations(
                        environment,
                        Vec::new(),
                        update.plugin_updates,
                        auth,
                    )
                    .await?;
                ToolProvisionConfig {
                    config: update.config.unwrap_or_else(|| {
                        golem_common::base_model::json::NormalizedJsonValue::new(serde_json::json!(
                            {}
                        ))
                    }),
                    env: update.env.unwrap_or_default(),
                    plugins,
                    files,
                }
            }
            (None, None) => {
                return Err(ComponentError::MissingToolDeploymentConfig(
                    tool_name.clone(),
                ));
            }
        };

        let old_environment_binding = existing
            .as_ref()
            .and_then(|metadata| metadata.environment_binding.clone());
        let old_agent_bindings = existing
            .as_ref()
            .map(|metadata| metadata.agent_bindings.clone())
            .unwrap_or_default();
        let old_component_bindings = existing
            .map(|metadata| metadata.component_bindings)
            .unwrap_or_default();

        Ok(ToolDeploymentMetadata {
            definition,
            provision,
            environment_binding: update
                .environment_binding
                .compute_new_value(old_environment_binding),
            component_bindings: update.component_bindings.unwrap_or(old_component_bindings),
            agent_bindings: update.agent_bindings.unwrap_or(old_agent_bindings),
        })
    }
}

fn tool_definitions_by_name(tools: Vec<Tool>) -> Result<BTreeMap<ToolName, Tool>, ComponentError> {
    let mut result = BTreeMap::new();
    for tool in tools {
        let raw_name = tool.name().ok_or(ComponentError::MissingToolName)?;
        let name =
            ToolName::try_from(raw_name).map_err(|message| ComponentError::InvalidToolName {
                name: raw_name.to_string(),
                message,
            })?;
        if let Err(errors) = validate_tool(&tool) {
            return Err(ComponentError::InvalidTool {
                tool: name.to_string(),
                errors: errors.into_iter().map(|error| error.to_string()).collect(),
            });
        }
        if result.insert(name.clone(), tool).is_some() {
            return Err(ComponentError::DuplicateToolName(name));
        }
    }
    Ok(result)
}

type ToolState = (
    BTreeMap<ToolName, Tool>,
    BTreeMap<ToolName, ToolDeploymentMetadata>,
);

fn tool_state_for_update(
    existing: &BTreeMap<ToolName, ToolDeploymentMetadata>,
    replacement: Option<Vec<Tool>>,
) -> Result<ToolState, ComponentError> {
    match replacement {
        None => Ok((
            existing
                .iter()
                .map(|(name, metadata)| (name.clone(), metadata.definition.clone()))
                .collect(),
            existing.clone(),
        )),
        Some(tools) => {
            let definitions = tool_definitions_by_name(tools)?;
            let metadata = existing
                .iter()
                .filter_map(|(name, metadata)| {
                    definitions.get(name).map(|definition| {
                        let mut metadata = metadata.clone();
                        metadata.definition = definition.clone();
                        (name.clone(), metadata)
                    })
                })
                .collect();
            Ok((definitions, metadata))
        }
    }
}

fn resolve_tool_deployment_metadata_for_creation(
    tools: Vec<Tool>,
    mut configs: BTreeMap<ToolName, ToolDeploymentConfigCreation>,
    uploaded_files: &HashMap<ArchiveFilePath, (AgentFileContentHash, u64)>,
    resolved_grants: &HashMap<EnvironmentPluginGrantId, EnvironmentPluginGrantWithDetails>,
) -> Result<BTreeMap<ToolName, ToolDeploymentMetadata>, ComponentError> {
    let definitions = tool_definitions_by_name(tools)?;
    let mut result = BTreeMap::new();

    for (name, definition) in definitions {
        let config = configs
            .remove(&name)
            .ok_or_else(|| ComponentError::MissingToolDeploymentConfig(name.clone()))?;
        let provision = resolve_tool_provision_config_for_creation(
            &name,
            config.provision,
            uploaded_files,
            resolved_grants,
        )?;
        result.insert(
            name,
            ToolDeploymentMetadata {
                definition,
                provision,
                environment_binding: config.environment_binding,
                component_bindings: config.component_bindings,
                agent_bindings: config.agent_bindings,
            },
        );
    }

    if let Some((name, _)) = configs.into_iter().next() {
        return Err(ComponentError::UndeclaredToolInDeploymentConfig(name));
    }

    Ok(result)
}

fn resolve_tool_provision_config_for_creation(
    tool_name: &ToolName,
    creation: ToolProvisionConfigCreation,
    uploaded_files: &HashMap<ArchiveFilePath, (AgentFileContentHash, u64)>,
    resolved_grants: &HashMap<EnvironmentPluginGrantId, EnvironmentPluginGrantWithDetails>,
) -> Result<ToolProvisionConfig, ComponentError> {
    Ok(ToolProvisionConfig {
        config: creation.config,
        env: creation.env,
        plugins: resolve_plugins_for_creation(&creation.plugin_installations, resolved_grants)?,
        files: resolve_tool_files_for_creation(tool_name, &creation.files, uploaded_files)?,
    })
}

fn resolve_tool_files_for_creation(
    tool_name: &ToolName,
    files: &BTreeMap<ArchiveFilePath, golem_common::model::component::AgentFileOptions>,
    uploaded_files: &HashMap<ArchiveFilePath, (AgentFileContentHash, u64)>,
) -> Result<Vec<InitialAgentFile>, ComponentError> {
    let mut result = BTreeMap::new();
    for (archive_path, options) in files {
        let (content_hash, size) = uploaded_files.get(archive_path).ok_or_else(|| {
            ComponentError::ToolFileNotFoundInArchive {
                tool: tool_name.clone(),
                archive_path: archive_path.clone(),
            }
        })?;
        let file = InitialAgentFile {
            path: options.target_path.clone(),
            content_hash: *content_hash,
            permissions: options.permissions,
            size: *size,
        };
        if result.insert(options.target_path.clone(), file).is_some() {
            return Err(ComponentError::ConflictingToolFileTarget {
                tool: tool_name.clone(),
                target_path: options.target_path.to_abs_string(),
            });
        }
    }
    Ok(result.into_values().collect())
}

fn resolve_tool_files_for_update(
    tool_name: &ToolName,
    existing: Vec<InitialAgentFile>,
    update: &ToolProvisionConfigUpdate,
    uploaded_files: &HashMap<ArchiveFilePath, (AgentFileContentHash, u64)>,
) -> Result<Vec<InitialAgentFile>, ComponentError> {
    let removed = update
        .files_to_remove
        .iter()
        .collect::<HashSet<&AgentFilePath>>();
    let mut files = existing
        .into_iter()
        .filter(|file| !removed.contains(&file.path))
        .map(|file| (file.path.clone(), file))
        .collect::<BTreeMap<_, _>>();

    let mut updated_targets = HashSet::new();
    for (archive_path, options) in &update.files_to_add_or_update {
        if !updated_targets.insert(options.target_path.clone()) {
            return Err(ComponentError::ConflictingToolFileTarget {
                tool: tool_name.clone(),
                target_path: options.target_path.to_abs_string(),
            });
        }
        let (content_hash, size) = uploaded_files.get(archive_path).ok_or_else(|| {
            ComponentError::ToolFileNotFoundInArchive {
                tool: tool_name.clone(),
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

    Ok(files.into_values().collect())
}

fn resolve_files_for_update(
    agent_type_name: &AgentTypeName,
    update: &AgentTypeProvisionConfigUpdate,
    uploaded_files: &HashMap<ArchiveFilePath, (AgentFileContentHash, u64)>,
) -> Result<Vec<InitialAgentFile>, ComponentError> {
    let mut files = HashMap::new();
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

    Ok(files.into_values().collect())
}

fn prepare_agent_initial_card_for_minting(
    agent_type_name: &AgentTypeName,
    initial_permissions: &golem_common::model::component::AgentTypeInitialPermissions,
    delegation_surface: &DelegationSurface,
) -> Result<PolymorphicCard, ComponentError> {
    prepare_initial_card_for_minting(initial_permissions, delegation_surface).map_err(|message| {
        ComponentError::InvalidAgentInitialPermissionCard {
            agent_type: agent_type_name.clone(),
            message,
        }
    })
}

fn prepare_initial_card_for_minting(
    initial_permissions: &golem_common::model::component::AgentTypeInitialPermissions,
    delegation_surface: &DelegationSurface,
) -> Result<PolymorphicCard, String> {
    let mut card = initial_permissions.to_polymorphic_card();
    let lower_positive = permission_envelopes_for_recipient_patterns(&card.lower_positive)
        .map_err(|message| message)?;
    let lower_negative = negative_permission_envelopes_for_recipient_patterns(&card.lower_negative)
        .map_err(|message| message)?;
    let upper_positive = permission_envelopes_for_recipient_patterns(&card.upper_positive)
        .map_err(|message| message)?;
    let upper_negative = negative_permission_envelopes_for_recipient_patterns(&card.upper_negative)
        .map_err(|message| message)?;
    delegation_surface
        .validate_attenuation(
            &lower_positive,
            &lower_negative,
            &upper_positive,
            &upper_negative,
        )
        .map_err(|error| {
            format!("card derivation is not allowed by the creator's cards: {error:?}")
        })
        .map(|parent_ids| {
            card.parent_ids = parent_ids;
        })?;
    Ok(card)
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

fn resolve_component_files_for_creation(
    files: &BTreeMap<ArchiveFilePath, golem_common::model::component::AgentFileOptions>,
    uploaded_files: &HashMap<ArchiveFilePath, (AgentFileContentHash, u64)>,
) -> Result<Vec<InitialAgentFile>, ComponentError> {
    files
        .iter()
        .map(|(archive_path, options)| {
            let (content_hash, size) = uploaded_files.get(archive_path).ok_or_else(|| {
                ComponentError::InvalidComponentConfig(format!(
                    "file '{archive_path}' was not found in the uploaded archive"
                ))
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

fn validate_component_config_schema(
    schema: &ComponentConfigSchema,
    agent_types: &[AgentTypeSchema],
) -> Result<(), ComponentError> {
    validate_graph(&schema.schema).map_err(|errors| {
        ComponentError::InvalidComponentConfig(
            errors.into_iter().map(|error| error.to_string()).join("; "),
        )
    })?;
    let mut declarations = BTreeMap::new();
    for declaration in &schema.declarations {
        if declaration.path.is_empty()
            || declaration
                .path
                .iter()
                .any(|segment| segment.is_empty() || segment.contains('.'))
        {
            return Err(ComponentError::InvalidComponentConfig(format!(
                "invalid declaration path '{}'",
                declaration.path.join(".")
            )));
        }
        let mut graph = schema.schema.clone();
        graph.root = declaration.value_type.clone();
        validate_graph(&graph).map_err(|errors| {
            ComponentError::InvalidComponentConfig(
                errors.into_iter().map(|error| error.to_string()).join("; "),
            )
        })?;
        if declarations
            .insert(&declaration.path, declaration)
            .is_some()
        {
            return Err(ComponentError::InvalidComponentConfig(format!(
                "duplicate declaration path '{}'",
                declaration.path.join(".")
            )));
        }
        for agent in agent_types {
            if let Some(agent_declaration) = agent
                .config
                .iter()
                .find(|candidate| candidate.path == declaration.path)
                && (agent_declaration.source != declaration.source
                    || !is_equivalent_cross_graph(
                        &schema.schema,
                        &declaration.value_type,
                        &agent.schema,
                        &agent_declaration.value_type,
                    ))
            {
                return Err(ComponentError::InvalidComponentConfig(format!(
                    "declaration '{}' conflicts with agent '{}'",
                    declaration.path.join("."),
                    agent.type_name
                )));
            }
        }
    }
    Ok(())
}

fn validate_and_transform_component_config_entries(
    schema: &ComponentConfigSchema,
    config_entries: Vec<AgentConfigEntryDto>,
) -> Result<Vec<TypedAgentConfigEntry>, ComponentError> {
    let mut results = Vec::new();
    let mut seen = HashSet::new();
    for entry in config_entries {
        let declaration = schema
            .declarations
            .iter()
            .find(|declaration| declaration.path == entry.path)
            .ok_or_else(|| {
                ComponentError::InvalidComponentConfig(format!(
                    "config '{}' is not declared",
                    entry.path.join(".")
                ))
            })?;
        if declaration.source != AgentConfigSource::Local {
            return Err(ComponentError::InvalidComponentConfig(format!(
                "secret config '{}' cannot be provided locally",
                entry.path.join(".")
            )));
        }
        if !seen.insert(entry.path.clone()) {
            return Err(ComponentError::InvalidComponentConfig(format!(
                "config '{}' was provided more than once",
                entry.path.join(".")
            )));
        }
        let value =
            from_untrusted_json_value(&schema.schema, &declaration.value_type, &entry.value.0)
                .map_err(|error| {
                    ComponentError::InvalidComponentConfig(format!(
                        "config '{}': {error}",
                        entry.path.join(".")
                    ))
                })?;
        validate_value(&schema.schema, &declaration.value_type, &value).map_err(|errors| {
            ComponentError::InvalidComponentConfig(format!(
                "config '{}': {}",
                entry.path.join("."),
                errors.iter().map(ToString::to_string).join(", ")
            ))
        })?;
        results.push(TypedAgentConfigEntry {
            path: entry.path,
            value: typed_schema_value_with_projected_defs(
                &schema.schema,
                declaration.value_type.clone(),
                value,
            ),
        });
    }
    Ok(results)
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
            from_untrusted_json_value(&agent_type.schema, declared_type, &config_value.value.0)
                .map_err(|err| ComponentError::AgentConfigTypeMismatch {
                    agent: agent_type.type_name.clone(),
                    key: config_value.path.clone(),
                    errors: vec![format!("config value is not a valid schema value: {err}")],
                })?;

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
    config_schema: ComponentConfigSchema,
    component_provision_config: ComponentProvisionConfig,
    agent_types: Vec<AgentTypeSchema>,
    wasm: Arc<[u8]>,
    agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig>,
    tool_deployment_metadata: BTreeMap<ToolName, ToolDeploymentMetadata>,
) -> Result<ComponentMetadata, ComponentError> {
    for agent_type in &agent_types {
        agent_type
            .validate()
            .map_err(ComponentProcessingError::Metadata)?;
        validate_agent_config_declarations(agent_type)?;
    }

    let component_metadata = run_cpu_bound_work(move || {
        ComponentMetadata::analyse_component_with_config(
            &wasm,
            config_schema,
            component_provision_config,
            agent_types,
            agent_type_provision_configs,
            tool_deployment_metadata,
        )
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
        if !metadata
            .agent_type_provision_configs()
            .contains_key(&agent_type.type_name)
        {
            return Err(ComponentError::MissingAgentTypeProvisionConfig(
                agent_type.type_name.clone(),
            ));
        }
    }

    if !metadata.tools().is_empty()
        && metadata.known_exports().tool_guest_interface.as_deref()
            != Some("golem:tool/guest@0.1.0")
    {
        return Err(ComponentError::ToolsRequireSupportedGuestExport {
            found: metadata.known_exports().tool_guest_interface.clone(),
        });
    }

    for (name, tool_metadata) in metadata.tools() {
        let definition_name = tool_metadata
            .definition
            .name()
            .ok_or(ComponentError::MissingToolName)?;
        if definition_name != name.as_str() {
            return Err(ComponentError::ToolDefinitionNameMismatch {
                key: name.clone(),
                definition_name: definition_name.to_string(),
            });
        }
        if let Err(errors) = validate_tool(&tool_metadata.definition) {
            return Err(ComponentError::InvalidTool {
                tool: name.to_string(),
                errors: errors.into_iter().map(|error| error.to_string()).collect(),
            });
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

#[cfg(test)]
async fn store_initial_agent_file(
    initial_agent_files_service: &InitialAgentFilesService,
    environment: &Environment,
    data: Vec<u8>,
    auth: &AuthCtx,
) -> Result<InitialAgentFileUpload, ComponentError> {
    let size = data.len() as u64;
    let stream = data
        .map_item(|item| item.map_err(anyhow::Error::from))
        .map_error(anyhow::Error::from);
    store_initial_agent_file_stream(initial_agent_files_service, environment, stream, size, auth)
        .await
}

async fn store_initial_agent_file_stream(
    initial_agent_files_service: &InitialAgentFilesService,
    environment: &Environment,
    stream: impl ReplayableStream<Item = Result<Vec<u8>, anyhow::Error>, Error = anyhow::Error>,
    size: u64,
    auth: &AuthCtx,
) -> Result<InitialAgentFileUpload, ComponentError> {
    authorize_environment_permission(auth, environment, EnvironmentVerb::Deploy)?;

    let content_hash = initial_agent_files_service
        .put_if_not_exists(environment.id, stream)
        .await?;

    Ok(InitialAgentFileUpload { content_hash, size })
}

#[cfg(test)]
mod initial_agent_file_tests {
    use super::*;
    use futures::TryStreamExt;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::application::{ApplicationId, ApplicationName};
    use golem_common::model::card::owner::EnvironmentOwnerPattern;
    use golem_common::model::card::{EffectiveSurface, EnvironmentResourcePattern, GrantSurface};
    use golem_common::model::environment::{EnvironmentName, EnvironmentRevision};
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use test_r::test;

    fn environment(id: EnvironmentId, name: &str) -> Environment {
        Environment {
            id,
            revision: EnvironmentRevision::INITIAL,
            application_id: ApplicationId::new(),
            application_name: ApplicationName::try_from("app").unwrap(),
            name: EnvironmentName::try_from(name).unwrap(),
            diff_model_version: 0,
            compatibility_check: false,
            version_check: false,
            security_overrides: false,
            owner_account_id: AccountId::new(),
            owner_account_email: AccountEmail::new("owner@example.com"),
            current_deployment: None,
        }
    }

    fn auth_for(environment: &Environment, verb: EnvironmentVerb) -> AuthCtx {
        AuthCtx::agent_with_effective_surface(
            environment.owner_account_id,
            environment.owner_account_email.clone(),
            EffectiveSurface {
                source_card_ids: Vec::new(),
                lower: vec![GrantSurface {
                    positive: vec![PermissionTarget::Environment(ClassPermissionTarget {
                        verb: Some(verb),
                        owner: EnvironmentOwnerPattern::Environment {
                            account: environment.owner_account_email.clone(),
                            application: environment.application_name.clone(),
                            environment: environment.name.clone(),
                        },
                        resource: EnvironmentResourcePattern::Any,
                    })],
                    negative: Vec::new(),
                }],
                upper: Vec::new(),
            },
        )
    }

    #[test]
    async fn upload_requires_deploy_permission_and_stores_by_environment() {
        let files = InitialAgentFilesService::new(Arc::new(InMemoryBlobStorage::new()));
        let allowed = environment(EnvironmentId::new(), "allowed");
        let other = environment(EnvironmentId::new(), "other");
        let content = b"remote tool bridge".to_vec();

        assert!(
            store_initial_agent_file(
                &files,
                &allowed,
                content.clone(),
                &auth_for(&allowed, EnvironmentVerb::View),
            )
            .await
            .is_err()
        );
        assert!(
            store_initial_agent_file(
                &files,
                &other,
                content.clone(),
                &auth_for(&allowed, EnvironmentVerb::Deploy),
            )
            .await
            .is_err()
        );

        let uploaded = store_initial_agent_file(
            &files,
            &allowed,
            content.clone(),
            &auth_for(&allowed, EnvironmentVerb::Deploy),
        )
        .await
        .unwrap();

        assert_eq!(uploaded.size, content.len() as u64);
        assert_eq!(
            uploaded.content_hash,
            AgentFileContentHash(Hash::new(blake3::hash(&content)))
        );
        assert!(
            files
                .exists(allowed.id, uploaded.content_hash)
                .await
                .unwrap()
        );
        assert!(!files.exists(other.id, uploaded.content_hash).await.unwrap());
        let stored = files
            .get(allowed.id, uploaded.content_hash)
            .await
            .unwrap()
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        assert_eq!(stored.concat(), content);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        prepare_agent_initial_card_for_minting, prepare_initial_card_for_minting,
        resolve_tool_deployment_metadata_for_creation, resolve_tool_files_for_update,
        tool_definitions_by_name, tool_state_for_update,
        validate_and_transform_component_config_entries, validate_component_config_schema,
        validate_component_metadata_invariants,
    };
    use crate::services::component::ComponentError;
    use golem_common::model::agent::{AgentConfigSource, AgentFileContentHash, AgentTypeName};
    use golem_common::model::card::recipient::RecipientPattern;
    use golem_common::model::card::{
        CardId, DelegationCard, DelegationSurface, parse_permission, parse_polymorphic_permission,
        permission_envelopes_for_recipient_patterns,
    };
    use golem_common::model::component::{
        AgentFileOptions, AgentFilePath, AgentFilePermissions, AgentTypeInitialPermissions,
        ArchiveFilePath, ToolDeploymentConfigCreation, ToolProvisionConfigCreation,
    };
    use golem_common::model::component_metadata::{ComponentMetadata, KnownExports};
    use golem_common::model::json::NormalizedJsonValue;
    use golem_common::model::tool::{ToolDeploymentMetadata, ToolName, ToolProvisionConfig};
    use golem_common::schema::agent::{AgentConfigDeclarationSchema, ComponentConfigSchema};
    use golem_common::schema::tool::{CommandNode, CommandTree, Doc, Globals, Tool};
    use golem_common::schema::{MetadataEnvelope, SchemaGraph, SchemaType};
    use std::collections::{BTreeMap, HashMap};
    use test_r::test;

    fn bool_component_schema(source: AgentConfigSource) -> ComponentConfigSchema {
        ComponentConfigSchema {
            schema: SchemaGraph::empty(),
            declarations: vec![AgentConfigDeclarationSchema {
                source,
                path: vec!["feature".to_string(), "enabled".to_string()],
                value_type: SchemaType::Bool {
                    metadata: MetadataEnvelope::default(),
                },
            }],
        }
    }

    #[test]
    fn standalone_component_config_accepts_typed_local_value_without_agents() {
        let schema = bool_component_schema(AgentConfigSource::Local);
        validate_component_config_schema(&schema, &[]).unwrap();
        let values = validate_and_transform_component_config_entries(
            &schema,
            vec![golem_common::model::worker::AgentConfigEntryDto {
                path: vec!["feature".to_string(), "enabled".to_string()],
                value: NormalizedJsonValue::new(serde_json::json!(true)),
            }],
        )
        .unwrap();
        assert_eq!(values.len(), 1);
    }

    #[test]
    fn standalone_component_secret_cannot_be_supplied_as_local_value() {
        let schema = bool_component_schema(AgentConfigSource::Secret);
        let error = validate_and_transform_component_config_entries(
            &schema,
            vec![golem_common::model::worker::AgentConfigEntryDto {
                path: vec!["feature".to_string(), "enabled".to_string()],
                value: NormalizedJsonValue::new(serde_json::json!(true)),
            }],
        )
        .unwrap_err();
        assert!(matches!(error, ComponentError::InvalidComponentConfig(_)));
    }

    fn parent_surface(parent_id: CardId) -> DelegationSurface {
        let defaults = AgentTypeInitialPermissions::default_for_recipient(RecipientPattern::Any)
            .to_polymorphic_card();
        let card = DelegationCard {
            source_card_id: Some(parent_id),
            lower_positive: permission_envelopes_for_recipient_patterns(&defaults.lower_positive)
                .unwrap(),
            lower_negative: Vec::new(),
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
        };
        DelegationSurface { cards: vec![card] }
    }

    const EXTERNAL_OWNER_RECIPIENT: &str =
        "owner@example.com/app/prod/component/~external-tool-owner";
    const COMPONENT_AGENTS: &str = "owner@example.com/app/prod/component/*";

    fn external_owner_attenuation_surface(lower: bool) -> DelegationSurface {
        let positive = parse_permission(&format!(
            "agent({COMPONENT_AGENTS}) @ {EXTERNAL_OWNER_RECIPIENT} : invoke : *"
        ))
        .unwrap();
        let negative = parse_permission(&format!(
            "agent({COMPONENT_AGENTS}) @ {EXTERNAL_OWNER_RECIPIENT} : delete :"
        ))
        .unwrap();
        DelegationSurface {
            cards: vec![DelegationCard {
                source_card_id: Some(CardId::new()),
                lower_positive: if lower {
                    vec![positive.clone()]
                } else {
                    Vec::new()
                },
                lower_negative: if lower {
                    vec![negative.clone()]
                } else {
                    Vec::new()
                },
                upper_positive: if lower { Vec::new() } else { vec![positive] },
                upper_negative: if lower { Vec::new() } else { vec![negative] },
            }],
        }
    }

    fn external_owner_initial_permissions(
        lower: bool,
        concrete_negative: bool,
    ) -> AgentTypeInitialPermissions {
        let positive = parse_polymorphic_permission(&format!(
            "agent(?agent) @ {EXTERNAL_OWNER_RECIPIENT} : invoke : *"
        ))
        .unwrap();
        let negative_owner = if concrete_negative {
            COMPONENT_AGENTS
        } else {
            "?agent"
        };
        let negative = parse_polymorphic_permission(&format!(
            "agent({negative_owner}) @ {EXTERNAL_OWNER_RECIPIENT} : delete :"
        ))
        .unwrap();
        if lower {
            AgentTypeInitialPermissions::from_patterns(
                vec![positive],
                vec![negative],
                Vec::new(),
                Vec::new(),
            )
        } else {
            AgentTypeInitialPermissions::from_patterns(
                Vec::new(),
                Vec::new(),
                vec![positive],
                vec![negative],
            )
        }
    }

    fn tool(name: &str, version: &str) -> Tool {
        Tool {
            version: version.to_string(),
            commands: CommandTree {
                nodes: vec![CommandNode {
                    name: name.to_string(),
                    aliases: Vec::new(),
                    doc: Doc::default(),
                    globals: Globals::default(),
                    subcommands: Vec::new(),
                    body: None,
                }],
            },
            schema: SchemaGraph::empty(),
        }
    }

    fn deployment_metadata(definition: Tool) -> ToolDeploymentMetadata {
        ToolDeploymentMetadata {
            definition,
            provision: ToolProvisionConfig::default(),
            environment_binding: None,
            component_bindings: BTreeMap::new(),
            agent_bindings: BTreeMap::new(),
        }
    }

    #[test]
    fn agent_initial_card_inherits_parent_ids_from_creator_surface() {
        let parent_id = CardId::new();
        let initial_permission =
            AgentTypeInitialPermissions::default_for_recipient(RecipientPattern::Any);

        let card = prepare_agent_initial_card_for_minting(
            &AgentTypeName("Cart".to_string()),
            &initial_permission,
            &parent_surface(parent_id),
        )
        .unwrap();

        assert_eq!(card.parent_ids, vec![parent_id]);
    }

    #[test]
    fn agent_initial_card_must_be_subsumed_by_creator_surface() {
        let initial_permission =
            AgentTypeInitialPermissions::default_for_recipient(RecipientPattern::Any);
        let empty_surface = DelegationSurface::default();

        let error = prepare_agent_initial_card_for_minting(
            &AgentTypeName("Cart".to_string()),
            &initial_permission,
            &empty_surface,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ComponentError::InvalidAgentInitialPermissionCard { .. }
        ));
    }

    #[test]
    fn component_initial_card_uses_creator_attenuation_and_delegation() {
        let parent_id = CardId::new();
        let initial_permission =
            AgentTypeInitialPermissions::default_for_recipient(RecipientPattern::Any);

        let card =
            prepare_initial_card_for_minting(&initial_permission, &parent_surface(parent_id))
                .unwrap();
        assert_eq!(card.parent_ids, vec![parent_id]);

        let error =
            prepare_initial_card_for_minting(&initial_permission, &DelegationSurface::default())
                .unwrap_err();
        assert!(error.contains("not allowed by the creator's cards"));
    }

    #[test]
    fn component_initial_card_lower_negative_does_not_widen_external_owner_agent() {
        let relative = external_owner_initial_permissions(true, false);
        assert!(
            prepare_initial_card_for_minting(&relative, &external_owner_attenuation_surface(true),)
                .is_err()
        );

        let concrete = external_owner_initial_permissions(true, true);
        let card =
            prepare_initial_card_for_minting(&concrete, &external_owner_attenuation_surface(true))
                .unwrap();
        assert_eq!(card.lower_negative, concrete.lower_bound.negative);
    }

    #[test]
    fn component_initial_card_upper_negative_does_not_widen_external_owner_agent() {
        let relative = external_owner_initial_permissions(false, false);
        assert!(prepare_initial_card_for_minting(
            &relative,
            &external_owner_attenuation_surface(false),
        )
        .is_err());

        let concrete = external_owner_initial_permissions(false, true);
        let card =
            prepare_initial_card_for_minting(&concrete, &external_owner_attenuation_surface(false))
                .unwrap();
        assert_eq!(card.upper_negative, concrete.upper_bound.negative);
    }

    #[test]
    fn tool_definitions_reject_duplicate_names() {
        let error =
            tool_definitions_by_name(vec![tool("grep", "1"), tool("grep", "2")]).unwrap_err();

        assert!(matches!(error, ComponentError::DuplicateToolName(_)));
    }

    #[test]
    fn tool_creation_requires_an_exact_definition_config_bijection() {
        let grep = tool("grep", "1");
        let grep_name = ToolName::try_from("grep").unwrap();

        let missing = resolve_tool_deployment_metadata_for_creation(
            vec![grep.clone()],
            BTreeMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap_err();
        assert!(matches!(
            missing,
            ComponentError::MissingToolDeploymentConfig(name) if name == grep_name
        ));

        let orphan = resolve_tool_deployment_metadata_for_creation(
            Vec::new(),
            BTreeMap::from([(
                grep_name.clone(),
                ToolDeploymentConfigCreation {
                    provision: ToolProvisionConfigCreation {
                        config: NormalizedJsonValue::new(serde_json::json!({})),
                        env: BTreeMap::new(),
                        plugin_installations: Vec::new(),
                        files: BTreeMap::new(),
                    },
                    environment_binding: None,
                    component_bindings: BTreeMap::new(),
                    agent_bindings: BTreeMap::new(),
                },
            )]),
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap_err();
        assert!(matches!(
            orphan,
            ComponentError::UndeclaredToolInDeploymentConfig(name) if name == grep_name
        ));
    }

    #[test]
    fn tool_update_distinguishes_omitted_and_explicit_empty_replacement() {
        let name = ToolName::try_from("grep").unwrap();
        let existing = BTreeMap::from([(name.clone(), deployment_metadata(tool("grep", "1")))]);

        let (preserved_definitions, preserved_metadata) =
            tool_state_for_update(&existing, None).unwrap();
        assert_eq!(preserved_definitions.len(), 1);
        assert_eq!(preserved_metadata, existing);

        let (cleared_definitions, cleared_metadata) =
            tool_state_for_update(&existing, Some(Vec::new())).unwrap();
        assert!(cleared_definitions.is_empty());
        assert!(cleared_metadata.is_empty());

        let (replaced_definitions, replaced_metadata) =
            tool_state_for_update(&existing, Some(vec![tool("grep", "2")])).unwrap();
        assert_eq!(replaced_definitions[&name].version, "2");
        assert_eq!(replaced_metadata[&name].definition.version, "2");
        assert_eq!(
            replaced_metadata[&name].provision,
            ToolProvisionConfig::default()
        );
    }

    #[test]
    fn tool_metadata_requires_exact_supported_guest_export() {
        let name = ToolName::try_from("grep").unwrap();
        let tools = BTreeMap::from([(name, deployment_metadata(tool("grep", "1")))]);

        for export in [None, Some("golem:tool/guest@0.2.0".to_string())] {
            let metadata = ComponentMetadata::from_parts_with_tools(
                KnownExports {
                    tool_guest_interface: export,
                    ..KnownExports::default()
                },
                Vec::new(),
                None,
                None,
                Vec::new(),
                BTreeMap::new(),
                tools.clone(),
            );
            assert!(matches!(
                validate_component_metadata_invariants(&metadata),
                Err(ComponentError::ToolsRequireSupportedGuestExport { .. })
            ));
        }

        let metadata = ComponentMetadata::from_parts_with_tools(
            KnownExports {
                tool_guest_interface: Some("golem:tool/guest@0.1.0".to_string()),
                ..KnownExports::default()
            },
            Vec::new(),
            None,
            None,
            Vec::new(),
            BTreeMap::new(),
            tools,
        );
        validate_component_metadata_invariants(&metadata).unwrap();
    }

    #[test]
    fn tool_file_update_rejects_duplicate_targets() {
        let target_path = AgentFilePath::from_abs_str("/config.json").unwrap();
        let update = golem_common::model::component::ToolProvisionConfigUpdate {
            config: None,
            env: None,
            plugin_updates: Vec::new(),
            files_to_add_or_update: BTreeMap::from([
                (
                    ArchiveFilePath::from_abs_str("/first.json").unwrap(),
                    AgentFileOptions {
                        target_path: target_path.clone(),
                        permissions: AgentFilePermissions::ReadOnly,
                    },
                ),
                (
                    ArchiveFilePath::from_abs_str("/second.json").unwrap(),
                    AgentFileOptions {
                        target_path,
                        permissions: AgentFilePermissions::ReadOnly,
                    },
                ),
            ]),
            files_to_remove: Vec::new(),
            file_permission_updates: BTreeMap::new(),
        };
        let uploaded_files = HashMap::from([
            (
                ArchiveFilePath::from_abs_str("/first.json").unwrap(),
                (
                    AgentFileContentHash(golem_common::model::diff::Hash::empty()),
                    1,
                ),
            ),
            (
                ArchiveFilePath::from_abs_str("/second.json").unwrap(),
                (
                    AgentFileContentHash(golem_common::model::diff::Hash::empty()),
                    1,
                ),
            ),
        ]);

        assert!(matches!(
            resolve_tool_files_for_update(
                &ToolName::try_from("grep").unwrap(),
                Vec::new(),
                &update,
                &uploaded_files,
            ),
            Err(ComponentError::ConflictingToolFileTarget { .. })
        ));
    }
}
