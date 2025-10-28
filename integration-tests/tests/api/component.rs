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

use super::Tracing;
use anyhow::anyhow;
use assert2::assert;
use golem_client::api::RegistryServiceClient;
use golem_common::model::base64::Base64;
use golem_common::model::component::{
    ComponentCreation, ComponentFileOptions, ComponentFilePath, ComponentFilePermissions,
    ComponentName, ComponentUpdate, PluginInstallation, PluginInstallationAction,
    PluginInstallationUpdate, PluginPriority, PluginUninstallation,
};
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantCreation;
use golem_common::model::plugin_registration::{
    ComponentTransformerPluginSpec, PluginRegistrationCreation, PluginSpecDto,
};
use golem_common::model::Empty;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use test_r::{inherit_test_dep, test};
use tracing::{debug, info};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn create_and_get_component(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    let component = user.component(&env.id, "shopping-cart").store().await?;

    let component_from_get = client.get_component(&component.id.0).await?;

    assert!(component_from_get == component);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn update_component(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component_1 = user.component(&env.id, "update-test-v1").store().await?;
    let component_2 = user
        .update_component_with(
            &component_1.id,
            component_1.revision,
            Some("update-test-v2"),
            None,
            vec![],
            vec![],
            None,
            None,
        )
        .await?;

    assert!(component_2.id == component_1.id);
    assert!(component_2.wasm_hash != component_1.wasm_hash);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn create_component_with_plugins_and_update_installations(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let library_plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "test-library-plugin".to_string(),
                version: "1.0.0".to_string(),
                description: "description".to_string(),
                icon: Base64(Vec::new()),
                homepage: "https://golem.cloud".to_string(),
                spec: PluginSpecDto::Library(Empty {}),
            },
            Some(
                tokio::fs::File::open(
                    deps.component_directory()
                        .join("app_and_library_library.wasm"),
                )
                .await?,
            ),
        )
        .await?;

    let library_plugin_grant = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: library_plugin.id.clone(),
            },
        )
        .await?;

    let plugin_parameters = BTreeMap::from_iter(vec![("foo".to_string(), "bar".to_string())]);

    let component = user
        .component(&env.id, "app_and_library_app")
        .with_parametrized_plugin(&library_plugin_grant.id, 0, plugin_parameters.clone())
        .store()
        .await?;

    assert!(component.installed_plugins.len() == 1);

    let installed_plugin = &component.installed_plugins[0];
    assert!(installed_plugin.priority.0 == 0);
    assert!(installed_plugin.parameters == plugin_parameters);

    // update priority of plugin
    let component_v2 = client
        .update_component(
            &component.id.0,
            &ComponentUpdate {
                current_revision: component.revision,
                component_type: None,
                removed_files: Vec::new(),
                new_file_options: BTreeMap::new(),
                dynamic_linking: None,
                env: None,
                agent_types: None,
                plugin_updates: vec![PluginInstallationAction::Update(PluginInstallationUpdate {
                    plugin_priority: installed_plugin.priority,
                    new_priority: Some(PluginPriority(1)),
                    new_parameters: None,
                })],
            },
            None::<Vec<u8>>,
            None::<Vec<u8>>,
        )
        .await?;

    assert!(component_v2.installed_plugins.len() == 1);

    let installed_plugin = &component_v2.installed_plugins[0];
    assert!(installed_plugin.priority.0 == 1);
    assert!(installed_plugin.parameters == plugin_parameters);

    // update priority of plugin
    let component_v3 = client
        .update_component(
            &component.id.0,
            &ComponentUpdate {
                current_revision: component_v2.revision,
                component_type: None,
                removed_files: Vec::new(),
                new_file_options: BTreeMap::new(),
                dynamic_linking: None,
                env: None,
                agent_types: None,
                plugin_updates: vec![PluginInstallationAction::Uninstall(PluginUninstallation {
                    plugin_priority: installed_plugin.priority,
                })],
            },
            None::<Vec<u8>>,
            None::<Vec<u8>>,
        )
        .await?;

    assert!(component_v3.installed_plugins.len() == 0);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn update_component_with_plugin(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let library_plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "test-library-plugin".to_string(),
                version: "1.0.0".to_string(),
                description: "description".to_string(),
                icon: Base64(Vec::new()),
                homepage: "https://golem.cloud".to_string(),
                spec: PluginSpecDto::Library(Empty {}),
            },
            Some(
                tokio::fs::File::open(
                    deps.component_directory()
                        .join("app_and_library_library.wasm"),
                )
                .await?,
            ),
        )
        .await?;

    let library_plugin_grant = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: library_plugin.id.clone(),
            },
        )
        .await?;

    let plugin_parameters = BTreeMap::from_iter(vec![("foo".to_string(), "bar".to_string())]);

    let component = user
        .component(&env.id, "app_and_library_app")
        .store()
        .await?;

    let updated_component = client
        .update_component(
            &component.id.0,
            &ComponentUpdate {
                current_revision: component.revision,
                component_type: None,
                removed_files: Vec::new(),
                new_file_options: BTreeMap::new(),
                dynamic_linking: None,
                env: None,
                agent_types: None,
                plugin_updates: vec![PluginInstallationAction::Install(PluginInstallation {
                    environment_plugin_grant_id: library_plugin_grant.id.clone(),
                    priority: PluginPriority(0),
                    parameters: plugin_parameters.clone(),
                })],
            },
            None::<Vec<u8>>,
            None::<Vec<u8>>,
        )
        .await?;

    assert!(updated_component.installed_plugins.len() == 1);

    {
        let installed_plugin = &updated_component.installed_plugins[0];
        assert!(installed_plugin.priority.0 == 0);
        assert!(installed_plugin.parameters == plugin_parameters);
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn install_component_transformer_plugin(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    use axum::extract::Multipart;
    use axum::routing::post;
    use axum::Router;

    async fn transform(mut multipart: Multipart) -> axum::Json<serde_json::Value> {
        while let Some(field) = multipart.next_field().await.unwrap() {
            let name = field.name().unwrap().to_string();
            let data = field.bytes().await.unwrap();
            debug!("Length of `{}` is {} bytes", name, data.len());

            match name.as_str() {
                "component" => {
                    info!("Received component data");
                }
                "metadata" => {
                    let json =
                        std::str::from_utf8(&data).expect("Failed to parse metadata as UTF-8");
                    info!("Metadata: {}", json);
                }
                _ => {
                    let value = std::str::from_utf8(&data).expect("Failed to parse field as UTF-8");
                    info!("Configuration field: {} = {}", name, value);
                }
            }
        }

        let response = json!({
            "env": {
                "TEST_ENV_VAR_2": "value_2"
            }
        });

        axum::Json(response)
    }

    let app = Router::new().route("/transform", post(transform));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server_handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let component_transformer_plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "test-library-plugin".to_string(),
                version: "1.0.0".to_string(),
                description: "description".to_string(),
                icon: Base64(Vec::new()),
                homepage: "https://golem.cloud".to_string(),
                spec: PluginSpecDto::ComponentTransformer(ComponentTransformerPluginSpec {
                    provided_wit_package: None,
                    json_schema: None,
                    validate_url: "not-used".to_string(),
                    transform_url: format!("http://localhost:{port}/transform"),
                }),
            },
            None::<Vec<u8>>,
        )
        .await?;

    let component_transformer_plugin_grant = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: component_transformer_plugin.id.clone(),
            },
        )
        .await?;

    let component = user
        .component(&env.id, "environment-service")
        .with_env(vec![("TEST_ENV_VAR_1".to_string(), "value_1".to_string())])
        .with_plugin(&component_transformer_plugin_grant.id, 0)
        .store()
        .await?;

    server_handle.abort();

    assert!(component.installed_plugins.len() == 1);
    let installed_plugin = &component.installed_plugins[0];
    assert!(installed_plugin.priority.0 == 0);

    assert!(
        component.env
            == BTreeMap::from_iter(vec![
                ("TEST_ENV_VAR_1".to_string(), "value_1".to_string()),
                ("TEST_ENV_VAR_2".to_string(), "value_2".to_string())
            ])
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn create_component_with_ifs_files(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    let component = client
        .create_component(
            &env.id.0,
            &ComponentCreation {
                component_name: ComponentName("ifs-test".to_string()),
                component_type: None,
                file_options: BTreeMap::from_iter(vec![(
                    ComponentFilePath::from_abs_str("/bar/baz.txt").map_err(|e| anyhow!(e))?,
                    ComponentFileOptions {
                        permissions: ComponentFilePermissions::ReadWrite,
                    },
                )]),
                dynamic_linking: HashMap::new(),
                env: BTreeMap::new(),
                agent_types: Vec::new(),
                plugins: Vec::new(),
            },
            tokio::fs::File::open(
                deps.component_directory()
                    .join("initial-file-read-write.wasm"),
            )
            .await?,
            Some(
                tokio::fs::File::open(
                    deps.component_directory()
                        .join("initial-file-read-write/files/archive.zip"),
                )
                .await?,
            ),
        )
        .await?;

    assert!(component.files.len() == 2);
    assert!(
        component
            .files
            .iter()
            .filter(|cf| cf.permissions == ComponentFilePermissions::ReadWrite)
            .count()
            == 1
    );

    Ok(())
}
