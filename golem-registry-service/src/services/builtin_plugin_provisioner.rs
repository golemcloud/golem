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
use crate::services::environment::{EnvironmentError, EnvironmentService};
use crate::services::environment_plugin_grant::{
    EnvironmentPluginGrantError, EnvironmentPluginGrantService,
};
use crate::services::deployment::{DeploymentService, DeploymentWriteService};
use crate::services::plugin_registration::{PluginRegistrationError, PluginRegistrationService};
use golem_common::model::account::AccountId;
use golem_common::model::application::{ApplicationCreation, ApplicationName};
use golem_common::model::base64::Base64;
use golem_common::model::component::{ComponentCreation, ComponentName};
use golem_common::model::deployment::{DeploymentCreation, DeploymentVersion};
use golem_common::model::environment::{EnvironmentCreation, EnvironmentName};
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantCreation;
use golem_common::model::plugin_registration::{
    OplogProcessorPluginSpec, PluginRegistrationCreation, PluginSpecDto,
};
use golem_service_base::model::auth::AuthCtx;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

const SYSTEM_APP_NAME: &str = "golem-system";
const SYSTEM_ENV_NAME: &str = "builtin-plugins";
const OTLP_COMPONENT_NAME: &str = "otlp:exporter";
const OTLP_PLUGIN_NAME: &str = "golem-otlp-exporter";
const OTLP_PLUGIN_VERSION: &str = "1.5.0";

pub async fn provision_builtin_plugins(
    config: &BuiltinPluginsConfig,
    root_account_id: AccountId,
    plugin_repo: &Arc<dyn PluginRepo>,
    application_service: &Arc<ApplicationService>,
    environment_service: &Arc<EnvironmentService>,
    component_service: &Arc<ComponentService>,
    component_write_service: &Arc<ComponentWriteService>,
    deployment_service: &Arc<DeploymentService>,
    deployment_write_service: &Arc<DeploymentWriteService>,
    plugin_registration_service: &Arc<PluginRegistrationService>,
    environment_plugin_grant_service: &Arc<EnvironmentPluginGrantService>,
) -> anyhow::Result<()> {
    if !config.enabled {
        return Ok(());
    }

    let wasm_bytes = match &config.otlp_exporter_wasm {
        Some(bytes) => bytes.clone(),
        None => {
            tracing::debug!("No OTLP exporter WASM provided, skipping plugin provisioning");
            return Ok(());
        }
    };

    let auth = AuthCtx::system();
    let component_name = ComponentName(OTLP_COMPONENT_NAME.to_string());

    // 1. Create or find "golem-system" application
    let app_name = ApplicationName(SYSTEM_APP_NAME.to_string());
    let app = match application_service
        .get_in_account(root_account_id, &app_name, &auth)
        .await
    {
        Ok(app) => app,
        Err(ApplicationError::ApplicationByNameNotFound(_)) => {
            match application_service
                .create(
                    root_account_id,
                    ApplicationCreation {
                        name: app_name.clone(),
                    },
                    &auth,
                )
                .await
            {
                Ok(app) => app,
                Err(ApplicationError::ApplicationWithNameAlreadyExists) => application_service
                    .get_in_account(root_account_id, &app_name, &auth)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to re-read system application: {e}"))?,
                Err(other) => {
                    return Err(anyhow::anyhow!("Failed to create application: {other}"));
                }
            }
        }
        Err(other) => {
            return Err(anyhow::anyhow!(
                "Failed to look up system application: {other}"
            ));
        }
    };

    // 2. Create or find "builtin-plugins" environment
    let env_name = EnvironmentName(SYSTEM_ENV_NAME.to_string());
    let env = match environment_service
        .get_in_application(app.id, &env_name, &auth)
        .await
    {
        Ok(env) => env,
        Err(EnvironmentError::EnvironmentByNameNotFound(_)) => {
            match environment_service
                .create(
                    app.id,
                    EnvironmentCreation {
                        name: env_name.clone(),
                        compatibility_check: false,
                        version_check: false,
                        security_overrides: false,
                    },
                    &auth,
                )
                .await
            {
                Ok(env) => env,
                Err(EnvironmentError::EnvironmentWithNameAlreadyExists) => environment_service
                    .get_in_application(app.id, &env_name, &auth)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to re-read system environment: {e}"))?,
                Err(other) => {
                    return Err(anyhow::anyhow!("Failed to create environment: {other}"));
                }
            }
        }
        Err(other) => {
            return Err(anyhow::anyhow!(
                "Failed to look up system environment: {other}"
            ));
        }
    };

    // 3. Upload "otlp-exporter" component if not exists
    let component = match component_write_service
        .create(
            env.id,
            ComponentCreation {
                component_name: component_name.clone(),
                file_options: BTreeMap::new(),
                env: BTreeMap::new(),
                config_vars: BTreeMap::new(),
                local_agent_config: Vec::new(),
                agent_types: Vec::new(),
                plugins: Vec::new(),
            },
            wasm_bytes.to_vec(),
            None,
            &auth,
        )
        .await
    {
        Ok(component) => component,
        Err(ComponentError::ComponentWithNameAlreadyExists(_)) => component_service
            .get_staged_component_by_name(env.id, &component_name, &auth)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get existing OTLP component: {e}"))?,
        Err(other) => {
            return Err(anyhow::anyhow!("Failed to create OTLP component: {other}"));
        }
    };

    // 3b. Deploy the "builtin-plugins" environment so the component is available as deployed
    let plan = deployment_service
        .get_current_deployment_plan(env.id, &auth)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get deployment plan: {e}"))?;
    match deployment_write_service
        .create_deployment(
            env.id,
            DeploymentCreation {
                current_revision: plan.current_revision,
                expected_deployment_hash: plan.deployment_hash,
                version: DeploymentVersion(Uuid::new_v4().to_string()),
                agent_secret_defaults: Vec::new(),
            },
            &auth,
        )
        .await
    {
        Ok(_) => {
            tracing::info!("Deployed builtin-plugins environment");
        }
        Err(e) => {
            tracing::warn!("Failed to deploy builtin-plugins environment (may already be deployed): {e}");
        }
    }

    // 4. Register "golem-otlp-exporter" plugin if not exists
    let plugin = match plugin_registration_service
        .register_plugin(
            root_account_id,
            PluginRegistrationCreation {
                name: OTLP_PLUGIN_NAME.to_string(),
                version: OTLP_PLUGIN_VERSION.to_string(),
                description: "Built-in OTLP exporter oplog processor plugin".to_string(),
                icon: Base64(Vec::new()),
                homepage: "https://golem.cloud".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: component.id,
                    component_revision: component.revision,
                }),
            },
            &auth,
        )
        .await
    {
        Ok(plugin) => plugin,
        Err(PluginRegistrationError::PluginNameAndVersionAlreadyExists) => {
            let record = plugin_repo
                .get_by_name_and_version(root_account_id.0, OTLP_PLUGIN_NAME, OTLP_PLUGIN_VERSION)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to look up existing plugin: {e}"))?
                .ok_or_else(|| anyhow::anyhow!("Plugin exists but could not be loaded"))?;
            record
                .try_into()
                .map_err(|e| anyhow::anyhow!("Failed to convert plugin record: {e}"))?
        }
        Err(other) => {
            return Err(anyhow::anyhow!("Failed to register plugin: {other}"));
        }
    };

    // 5. Grant plugin to all existing environments
    let apps = application_service
        .list_in_account(root_account_id, &auth)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to list applications: {e}"))?;

    for app_entry in &apps {
        let envs = match environment_service
            .list_in_application(app_entry.id, &auth)
            .await
        {
            Ok(envs) => envs,
            Err(e) => {
                tracing::warn!(
                    "Failed to list environments for app {}: {e}",
                    app_entry.name
                );
                continue;
            }
        };

        for env_entry in &envs {
            match environment_plugin_grant_service
                .create(
                    env_entry.id,
                    EnvironmentPluginGrantCreation {
                        plugin_registration_id: plugin.id,
                    },
                    &auth,
                )
                .await
            {
                Ok(_) => {}
                Err(EnvironmentPluginGrantError::GrantForPluginAlreadyExists) => {}
                Err(e) => {
                    tracing::warn!("Failed to grant plugin to env {}: {e}", env_entry.name);
                }
            }
        }
    }

    tracing::info!("Built-in plugins provisioned successfully");
    Ok(())
}
