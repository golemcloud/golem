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

use crate::config::BuiltinPluginsConfig;
use crate::repo::plugin::PluginRepo;
use crate::services::application::{ApplicationError, ApplicationService};
use crate::services::component::{ComponentError, ComponentService, ComponentWriteService};
use crate::services::deployment::{DeploymentService, DeploymentWriteService};
use crate::services::environment::{EnvironmentError, EnvironmentService};

use crate::services::plugin_registration::{PluginRegistrationError, PluginRegistrationService};
use golem_common::model::account::AccountId;
use golem_common::model::application::{
    Application, ApplicationCreation, ApplicationId, ApplicationName,
};
use golem_common::model::base64::Base64;
use golem_common::model::component::{ComponentCreation, ComponentName, ComponentUpdate};
use golem_common::model::deployment::{DeploymentCreation, DeploymentVersion};
use golem_common::model::diff;
use golem_common::model::environment::{
    Environment, EnvironmentCreation, EnvironmentId, EnvironmentName,
};
use golem_common::model::plugin_registration::{
    OplogProcessorPluginSpec, PluginRegistrationCreation, PluginSpecDto,
};
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::component::Component;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

const SYSTEM_APP_NAME: &str = "golem-system";
const SYSTEM_ENV_NAME: &str = "builtin-plugins";

struct BuiltinPluginDescriptor {
    component_name: &'static str,
    plugin_name: &'static str,
    version: &'static str,
    description: &'static str,
    wasm_bytes: &'static [u8],
}

impl BuiltinPluginDescriptor {
    fn plugin_spec(&self, component: &Component) -> PluginSpecDto {
        PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
            component_id: component.id,
            component_revision: component.revision,
        })
    }
}

// Build with `cargo make build-plugins` first to ensure the WASM files exist.
static BUILTIN_PLUGINS: &[BuiltinPluginDescriptor] = &[BuiltinPluginDescriptor {
    component_name: "otlp:exporter",
    plugin_name: "golem-otlp-exporter",
    version: "1.5.0",
    description: "Built-in OTLP exporter oplog processor plugin",
    wasm_bytes: include_bytes!("../../../plugins/otlp-exporter.wasm"),
}];

pub async fn provision_builtin_plugins(
    config: &BuiltinPluginsConfig,
    builtin_plugin_owner_account_id: AccountId,
    plugin_repo: &Arc<dyn PluginRepo>,
    application_service: &Arc<ApplicationService>,
    environment_service: &Arc<EnvironmentService>,
    component_service: &Arc<ComponentService>,
    component_write_service: &Arc<ComponentWriteService>,
    deployment_service: &Arc<DeploymentService>,
    deployment_write_service: &Arc<DeploymentWriteService>,
    plugin_registration_service: &Arc<PluginRegistrationService>,
) -> anyhow::Result<()> {
    if !config.enabled() {
        return Ok(());
    }

    let auth = AuthCtx::system();

    let app = get_or_create_application(
        application_service,
        builtin_plugin_owner_account_id,
        &ApplicationName(SYSTEM_APP_NAME.to_string()),
        &auth,
    )
    .await?;

    let env = get_or_create_environment(
        environment_service,
        app.id,
        &EnvironmentName(SYSTEM_ENV_NAME.to_string()),
        &auth,
    )
    .await?;

    let mut components = Vec::new();
    for descriptor in BUILTIN_PLUGINS {
        let component = upload_or_update_component(
            component_service,
            component_write_service,
            env.id,
            &ComponentName(descriptor.component_name.to_string()),
            descriptor.wasm_bytes,
            &auth,
        )
        .await?;
        components.push((descriptor, component));
    }

    deploy_environment(deployment_service, deployment_write_service, env.id, &auth).await;

    for (descriptor, component) in &components {
        register_plugin(
            plugin_registration_service,
            plugin_repo,
            builtin_plugin_owner_account_id,
            descriptor,
            component,
            &auth,
        )
        .await?;
    }

    tracing::info!("Built-in plugins provisioned successfully");
    Ok(())
}

async fn get_or_create_application(
    application_service: &Arc<ApplicationService>,
    account_id: AccountId,
    app_name: &ApplicationName,
    auth: &AuthCtx,
) -> anyhow::Result<Application> {
    match application_service
        .get_in_account(account_id, app_name, auth)
        .await
    {
        Ok(app) => Ok(app),
        Err(ApplicationError::ApplicationByNameNotFound(_)) => {
            match application_service
                .create(
                    account_id,
                    ApplicationCreation {
                        name: app_name.clone(),
                    },
                    auth,
                )
                .await
            {
                Ok(app) => Ok(app),
                Err(ApplicationError::ApplicationWithNameAlreadyExists) => application_service
                    .get_in_account(account_id, app_name, auth)
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!("Failed to re-read application '{app_name}': {e}")
                    }),
                Err(other) => Err(anyhow::anyhow!(
                    "Failed to create application '{app_name}': {other}"
                )),
            }
        }
        Err(other) => Err(anyhow::anyhow!(
            "Failed to look up application '{app_name}': {other}"
        )),
    }
}

async fn get_or_create_environment(
    environment_service: &Arc<EnvironmentService>,
    app_id: ApplicationId,
    env_name: &EnvironmentName,
    auth: &AuthCtx,
) -> anyhow::Result<Environment> {
    match environment_service
        .get_in_application(app_id, env_name, auth)
        .await
    {
        Ok(env) => Ok(env),
        Err(EnvironmentError::EnvironmentByNameNotFound(_)) => {
            match environment_service
                .create(
                    app_id,
                    EnvironmentCreation {
                        name: env_name.clone(),
                        compatibility_check: false,
                        version_check: false,
                        security_overrides: false,
                    },
                    auth,
                )
                .await
            {
                Ok(env) => Ok(env),
                Err(EnvironmentError::EnvironmentWithNameAlreadyExists) => environment_service
                    .get_in_application(app_id, env_name, auth)
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!("Failed to re-read environment '{env_name}': {e}")
                    }),
                Err(other) => Err(anyhow::anyhow!(
                    "Failed to create environment '{env_name}': {other}"
                )),
            }
        }
        Err(other) => Err(anyhow::anyhow!(
            "Failed to look up environment '{env_name}': {other}"
        )),
    }
}

async fn upload_or_update_component(
    component_service: &Arc<ComponentService>,
    component_write_service: &Arc<ComponentWriteService>,
    env_id: EnvironmentId,
    component_name: &ComponentName,
    wasm_bytes: &[u8],
    auth: &AuthCtx,
) -> anyhow::Result<Component> {
    let embedded_wasm_hash = diff::Hash::new(blake3::hash(wasm_bytes));

    match component_write_service
        .create(
            env_id,
            ComponentCreation {
                component_name: component_name.clone(),
                agent_types: Vec::new(),
                agent_type_provision_configs: BTreeMap::new(),
            },
            wasm_bytes.to_vec(),
            None,
            auth,
        )
        .await
    {
        Ok(component) => Ok(component),
        Err(ComponentError::ComponentWithNameAlreadyExists(_)) => {
            let existing = component_service
                .get_staged_component_by_name(env_id, component_name, auth)
                .await
                .map_err(|e| {
                    anyhow::anyhow!("Failed to get existing component '{component_name}': {e}")
                })?;

            if existing.wasm_hash == embedded_wasm_hash {
                tracing::info!(
                    "Component '{component_name}' is already up to date, skipping update"
                );
                Ok(existing)
            } else {
                tracing::info!("Component '{component_name}' has changed, updating");
                component_write_service
                    .update(
                        existing.id,
                        ComponentUpdate {
                            current_revision: existing.revision,
                            agent_types: None,
                            agent_type_provision_config_updates: None,
                        },
                        Some(wasm_bytes.to_vec()),
                        None,
                        auth,
                    )
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!("Failed to update component '{component_name}': {e}")
                    })
            }
        }
        Err(other) => Err(anyhow::anyhow!(
            "Failed to create component '{component_name}': {other}"
        )),
    }
}

async fn deploy_environment(
    deployment_service: &Arc<DeploymentService>,
    deployment_write_service: &Arc<DeploymentWriteService>,
    env_id: EnvironmentId,
    auth: &AuthCtx,
) {
    let plan = match deployment_service
        .get_current_deployment_plan(env_id, auth)
        .await
    {
        Ok(plan) => plan,
        Err(e) => {
            tracing::warn!("Failed to get deployment plan for environment {env_id}: {e}");
            return;
        }
    };

    match deployment_write_service
        .create_deployment(
            env_id,
            DeploymentCreation {
                current_revision: plan.current_revision,
                expected_deployment_hash: plan.deployment_hash,
                version: DeploymentVersion(Uuid::new_v4().to_string()),
                agent_secret_defaults: Vec::new(),
                quota_resource_defaults: Vec::new(),
                retry_policy_defaults: Vec::new(),
            },
            auth,
        )
        .await
    {
        Ok(_) => {
            tracing::info!("Deployed environment {env_id}");
        }
        Err(e) => {
            tracing::warn!("Failed to deploy environment {env_id} (may already be deployed): {e}");
        }
    }
}

async fn register_plugin(
    plugin_registration_service: &Arc<PluginRegistrationService>,
    plugin_repo: &Arc<dyn PluginRepo>,
    account_id: AccountId,
    descriptor: &BuiltinPluginDescriptor,
    component: &Component,
    auth: &AuthCtx,
) -> anyhow::Result<()> {
    let plugin_name = descriptor.plugin_name;
    let plugin_version = descriptor.version;
    match plugin_registration_service
        .register_plugin(
            account_id,
            PluginRegistrationCreation {
                name: plugin_name.to_string(),
                version: plugin_version.to_string(),
                description: descriptor.description.to_string(),
                icon: Base64(Vec::new()),
                homepage: "https://golem.cloud".to_string(),
                spec: descriptor.plugin_spec(component),
            },
            auth,
        )
        .await
    {
        Ok(_) => Ok(()),
        Err(PluginRegistrationError::PluginNameAndVersionAlreadyExists) => {
            let _record = plugin_repo
                .get_by_name_and_version(account_id.0, plugin_name, plugin_version)
                .await
                .map_err(|e| {
                    anyhow::anyhow!("Failed to look up existing plugin '{plugin_name}': {e}")
                })?
                .ok_or_else(|| {
                    anyhow::anyhow!("Plugin '{plugin_name}' exists but could not be loaded")
                })?;
            Ok(())
        }
        Err(other) => Err(anyhow::anyhow!(
            "Failed to register plugin '{plugin_name}': {other}"
        )),
    }
}
