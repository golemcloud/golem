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

use anyhow::anyhow;
use golem_client::api::{
    RegistryServiceClient, RegistryServiceCreateComponentError, RegistryServiceGetComponentError,
    RegistryServiceGetEnvironmentComponentError, RegistryServiceUpdateComponentError,
};
use golem_common::model::agent::{
    AgentConstructor, AgentMethod, AgentMode, AgentType, ComponentModelElementSchema, DataSchema,
    DeployedRegisteredAgentType, ElementSchema, NamedElementSchema, NamedElementSchemas,
    RegisteredAgentTypeImplementer,
};
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
use golem_wasm::analysis::{AnalysedType, TypeStr, TypeU32};
use pretty_assertions::{assert_eq, assert_ne};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use test_r::{inherit_test_dep, test};
use tokio::fs::File;
use tracing::{debug, info};

inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn create_and_get_component(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    {
        let fetched_component = client.get_component(&component.id.0).await?;
        assert_eq!(fetched_component, component);
    }

    {
        let fetched_component = client
            .get_environment_component(&env.id.0, &component.component_name.0)
            .await?;
        assert_eq!(fetched_component, component);
    }

    {
        let fetched_components = client.get_environment_components(&env.id.0).await?;
        assert_eq!(fetched_components.values, vec![component]);
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn update_component(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    let component = user.component(&env.id, "update-test-v1").store().await?;
    let updated_component = user
        .update_component_with(
            &component.id,
            component.revision,
            Some("update-test-v2"),
            vec![],
            vec![],
            None,
            None,
        )
        .await?;

    assert_eq!(updated_component.id, component.id);
    assert_ne!(updated_component.wasm_hash, component.wasm_hash);

    {
        let fetched_component = client.get_component(&component.id.0).await?;
        assert_eq!(fetched_component, updated_component);
    }

    {
        let fetched_component = client
            .get_environment_component(&env.id.0, &component.component_name.0)
            .await?;
        assert_eq!(fetched_component, updated_component);
    }

    {
        let fetched_components = client.get_environment_components(&env.id.0).await?;
        assert_eq!(fetched_components.values, vec![updated_component]);
    }
    Ok(())
}

#[test]
#[tracing::instrument]
async fn component_update_with_wrong_revision_is_rejected(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    let component = user.component(&env.id, "update-test-v1").store().await?;
    let result = client
        .update_component(
            &component.id.0,
            &ComponentUpdate {
                current_revision: component.revision.next()?,
                removed_files: Vec::new(),
                new_file_options: BTreeMap::new(),
                dynamic_linking: None,
                env: None,
                agent_types: None,
                plugin_updates: Vec::new(),
            },
            None::<File>,
            None::<File>,
        )
        .await;

    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceUpdateComponentError::Error409(_)
        ))
    ));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn delete_component(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    let component = user.component(&env.id, "update-test-v1").store().await?;
    client
        .delete_component(&component.id.0, component.revision.into())
        .await?;

    {
        let result = client.get_component(&component.id.0).await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetComponentError::Error404(_)
            ))
        ));
    }

    {
        let result = client
            .get_environment_component(&env.id.0, &component.component_name.0)
            .await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetEnvironmentComponentError::Error404(_)
            ))
        ));
    }

    {
        let fetched_components = client.get_environment_components(&env.id.0).await?;
        assert_eq!(fetched_components.values, vec![]);
    }
    Ok(())
}

#[test]
#[tracing::instrument]
async fn create_component_with_plugins_and_update_installations(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
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
                plugin_registration_id: library_plugin.id,
            },
        )
        .await?;

    let plugin_parameters = BTreeMap::from_iter(vec![("foo".to_string(), "bar".to_string())]);

    let component = user
        .component(&env.id, "app_and_library_app")
        .with_parametrized_plugin(&library_plugin_grant.id, 0, plugin_parameters.clone())
        .store()
        .await?;

    assert_eq!(component.installed_plugins.len(), 1);

    let installed_plugin = &component.installed_plugins[0];
    assert_eq!(installed_plugin.priority.0, 0);
    assert_eq!(installed_plugin.parameters, plugin_parameters);

    // update priority of plugin
    let component_v2 = client
        .update_component(
            &component.id.0,
            &ComponentUpdate {
                current_revision: component.revision,
                removed_files: Vec::new(),
                new_file_options: BTreeMap::new(),
                dynamic_linking: None,
                env: None,
                agent_types: None,
                plugin_updates: vec![PluginInstallationAction::Update(PluginInstallationUpdate {
                    environment_plugin_grant_id: installed_plugin.environment_plugin_grant_id,
                    new_priority: Some(PluginPriority(1)),
                    new_parameters: None,
                })],
            },
            None::<Vec<u8>>,
            None::<Vec<u8>>,
        )
        .await?;

    assert_eq!(component_v2.installed_plugins.len(), 1);

    let installed_plugin = &component_v2.installed_plugins[0];
    assert_eq!(installed_plugin.priority.0, 1);
    assert_eq!(installed_plugin.parameters, plugin_parameters);

    // update priority of plugin
    let component_v3 = client
        .update_component(
            &component.id.0,
            &ComponentUpdate {
                current_revision: component_v2.revision,
                removed_files: Vec::new(),
                new_file_options: BTreeMap::new(),
                dynamic_linking: None,
                env: None,
                agent_types: None,
                plugin_updates: vec![PluginInstallationAction::Uninstall(PluginUninstallation {
                    environment_plugin_grant_id: installed_plugin.environment_plugin_grant_id,
                })],
            },
            None::<Vec<u8>>,
            None::<Vec<u8>>,
        )
        .await?;

    assert_eq!(component_v3.installed_plugins.len(), 0);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn update_component_with_plugin(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
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
                plugin_registration_id: library_plugin.id,
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
                removed_files: Vec::new(),
                new_file_options: BTreeMap::new(),
                dynamic_linking: None,
                env: None,
                agent_types: None,
                plugin_updates: vec![PluginInstallationAction::Install(PluginInstallation {
                    environment_plugin_grant_id: library_plugin_grant.id,
                    priority: PluginPriority(0),
                    parameters: plugin_parameters.clone(),
                })],
            },
            None::<Vec<u8>>,
            None::<Vec<u8>>,
        )
        .await?;

    assert_eq!(updated_component.installed_plugins.len(), 1);

    {
        let installed_plugin = &updated_component.installed_plugins[0];
        assert_eq!(installed_plugin.priority.0, 0);
        assert_eq!(installed_plugin.parameters, plugin_parameters);
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn create_component_with_ifs_files(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    let component = client
        .create_component(
            &env.id.0,
            &ComponentCreation {
                component_name: ComponentName("golem:it".to_string()),
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
                    .join("it_initial_file_system_release.wasm"),
            )
            .await?,
            Some(
                tokio::fs::File::open(
                    deps.component_directory()
                        .join("initial-file-system/files/archive.zip"),
                )
                .await?,
            ),
        )
        .await?;

    assert_eq!(component.files.len(), 2);
    assert_eq!(
        component
            .files
            .iter()
            .filter(|cf| cf.permissions == ComponentFilePermissions::ReadWrite)
            .count(),
        1
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn component_recreation(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    let component = user.component(&env.id, "update-test-v1").store().await?;
    client
        .delete_component(&component.id.0, component.revision.into())
        .await?;

    let recreated_component = user.component(&env.id, "update-test-v1").store().await?;
    assert_eq!(recreated_component.id, component.id);
    assert_eq!(
        recreated_component.revision,
        component.revision.next()?.next()?
    );

    client
        .delete_component(&component.id.0, recreated_component.revision.into())
        .await?;

    Ok(())
}

#[test]
#[tracing::instrument]
async fn list_agent_types(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    let agent_type = AgentType {
        type_name: golem_common::model::agent::AgentTypeName("CounterAgent".to_string()),
        description: "".to_string(),
        constructor: AgentConstructor {
            name: None,
            description: "".to_string(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "name".to_string(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: AnalysedType::Str(TypeStr),
                    }),
                }],
            }),
        },
        methods: vec![AgentMethod {
            name: "increment".to_string(),
            description: "".to_string(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
            output_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "return-value".to_string(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: AnalysedType::U32(TypeU32),
                    }),
                }],
            }),
            http_endpoint: Vec::new(),
        }],
        dependencies: vec![],
        mode: AgentMode::Durable,
        http_mount: None,
    };

    let component = client
        .create_component(
            &env.id.0,
            &ComponentCreation {
                component_name: ComponentName("golem:it".to_string()),
                file_options: BTreeMap::new(),
                dynamic_linking: HashMap::new(),
                env: BTreeMap::new(),
                agent_types: vec![agent_type.clone()],
                plugins: Vec::new(),
            },
            tokio::fs::File::open(
                deps.component_directory()
                    .join("it_agent_counters_release.wasm"),
            )
            .await?,
            None::<Vec<u8>>,
        )
        .await?;

    assert_eq!(
        component.metadata.agent_types(),
        std::slice::from_ref(&agent_type)
    );

    let deployment = user.deploy_environment(&env.id).await?;

    let agent_types = client
        .list_deployment_agent_types(&env.id.0, deployment.revision.into())
        .await?;

    assert_eq!(
        agent_types.values,
        vec![DeployedRegisteredAgentType {
            agent_type,
            implemented_by: RegisteredAgentTypeImplementer {
                component_id: component.id,
                component_revision: component.revision,
            },
            webhook_prefix_authority_and_path: None
        }]
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn create_component_with_duplicate_plugin_priorities_fails(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let plugin_1 = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "plugin-1".to_string(),
                version: "1.0.0".to_string(),
                description: "".to_string(),
                icon: Base64(Vec::new()),
                homepage: "".to_string(),
                spec: PluginSpecDto::Library(Empty {}),
            },
            Some(
                tokio::fs::read(
                    deps.component_directory()
                        .join("app_and_library_library.wasm"),
                )
                .await?,
            ),
        )
        .await?;

    let plugin_2 = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "plugin-2".to_string(),
                version: "1.0.0".to_string(),
                description: "".to_string(),
                icon: Base64(Vec::new()),
                homepage: "".to_string(),
                spec: PluginSpecDto::Library(Empty {}),
            },
            Some(
                tokio::fs::read(
                    deps.component_directory()
                        .join("app_and_library_library.wasm"),
                )
                .await?,
            ),
        )
        .await?;

    let grant_1 = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: plugin_1.id,
            },
        )
        .await?;

    let grant_2 = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: plugin_2.id,
            },
        )
        .await?;

    let result = client
        .create_component(
            &env.id.0,
            &ComponentCreation {
                component_name: ComponentName("duplicate-priority".to_string()),
                file_options: BTreeMap::new(),
                dynamic_linking: HashMap::new(),
                env: BTreeMap::new(),
                agent_types: Vec::new(),
                plugins: vec![
                    PluginInstallation {
                        environment_plugin_grant_id: grant_1.id,
                        priority: PluginPriority(0),
                        parameters: BTreeMap::new(),
                    },
                    PluginInstallation {
                        environment_plugin_grant_id: grant_2.id,
                        priority: PluginPriority(0),
                        parameters: BTreeMap::new(),
                    },
                ],
            },
            tokio::fs::File::open(deps.component_directory().join("app_and_library_app.wasm"))
                .await?,
            None::<Vec<u8>>,
        )
        .await;

    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceCreateComponentError::Error409(_)
        ))
    ));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn create_component_with_duplicate_plugin_grant_ids_fails(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "plugin".to_string(),
                version: "1.0.0".to_string(),
                description: "".to_string(),
                icon: Base64(Vec::new()),
                homepage: "".to_string(),
                spec: PluginSpecDto::Library(Empty {}),
            },
            Some(
                tokio::fs::read(
                    deps.component_directory()
                        .join("app_and_library_library.wasm"),
                )
                .await?,
            ),
        )
        .await?;

    let grant = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: plugin.id,
            },
        )
        .await?;

    let result = client
        .create_component(
            &env.id.0,
            &ComponentCreation {
                component_name: ComponentName("duplicate-grant".to_string()),
                file_options: BTreeMap::new(),
                dynamic_linking: HashMap::new(),
                env: BTreeMap::new(),
                agent_types: Vec::new(),
                plugins: vec![
                    PluginInstallation {
                        environment_plugin_grant_id: grant.id,
                        priority: PluginPriority(0),
                        parameters: BTreeMap::new(),
                    },
                    PluginInstallation {
                        environment_plugin_grant_id: grant.id,
                        priority: PluginPriority(1),
                        parameters: BTreeMap::new(),
                    },
                ],
            },
            tokio::fs::File::open(deps.component_directory().join("app_and_library_app.wasm"))
                .await?,
            None::<Vec<u8>>,
        )
        .await;

    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceCreateComponentError::Error409(_)
        ))
    ));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn update_component_with_duplicate_plugin_priorities_fails(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let plugin_1 = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "plugin-1".to_string(),
                version: "1.0.0".to_string(),
                description: "".to_string(),
                icon: Base64(Vec::new()),
                homepage: "".to_string(),
                spec: PluginSpecDto::Library(Empty {}),
            },
            Some(
                tokio::fs::read(
                    deps.component_directory()
                        .join("app_and_library_library.wasm"),
                )
                .await?,
            ),
        )
        .await?;

    let plugin_2 = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "plugin-2".to_string(),
                version: "1.0.0".to_string(),
                description: "".to_string(),
                icon: Base64(Vec::new()),
                homepage: "".to_string(),
                spec: PluginSpecDto::Library(Empty {}),
            },
            Some(
                tokio::fs::read(
                    deps.component_directory()
                        .join("app_and_library_library.wasm"),
                )
                .await?,
            ),
        )
        .await?;

    let grant_1 = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: plugin_1.id,
            },
        )
        .await?;

    let grant_2 = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: plugin_2.id,
            },
        )
        .await?;

    let component = user
        .component(&env.id, "app_and_library_app")
        .store()
        .await?;

    let result = client
        .update_component(
            &component.id.0,
            &ComponentUpdate {
                current_revision: component.revision,
                removed_files: Vec::new(),
                new_file_options: BTreeMap::new(),
                dynamic_linking: None,
                env: None,
                agent_types: None,
                plugin_updates: vec![
                    PluginInstallationAction::Install(PluginInstallation {
                        environment_plugin_grant_id: grant_1.id,
                        priority: PluginPriority(0),
                        parameters: BTreeMap::new(),
                    }),
                    PluginInstallationAction::Install(PluginInstallation {
                        environment_plugin_grant_id: grant_2.id,
                        priority: PluginPriority(0),
                        parameters: BTreeMap::new(),
                    }),
                ],
            },
            None::<Vec<u8>>,
            None::<Vec<u8>>,
        )
        .await;

    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceUpdateComponentError::Error409(_)
        ))
    ));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn update_component_with_duplicate_plugin_grant_ids_fails(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "plugin".to_string(),
                version: "1.0.0".to_string(),
                description: "".to_string(),
                icon: Base64(Vec::new()),
                homepage: "".to_string(),
                spec: PluginSpecDto::Library(Empty {}),
            },
            Some(
                tokio::fs::read(
                    deps.component_directory()
                        .join("app_and_library_library.wasm"),
                )
                .await?,
            ),
        )
        .await?;

    let grant = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: plugin.id,
            },
        )
        .await?;

    let component = user
        .component(&env.id, "app_and_library_app")
        .store()
        .await?;

    let result = client
        .update_component(
            &component.id.0,
            &ComponentUpdate {
                current_revision: component.revision,
                removed_files: Vec::new(),
                new_file_options: BTreeMap::new(),
                dynamic_linking: None,
                env: None,
                agent_types: None,
                plugin_updates: vec![
                    PluginInstallationAction::Install(PluginInstallation {
                        environment_plugin_grant_id: grant.id,
                        priority: PluginPriority(0),
                        parameters: BTreeMap::new(),
                    }),
                    PluginInstallationAction::Install(PluginInstallation {
                        environment_plugin_grant_id: grant.id,
                        priority: PluginPriority(1),
                        parameters: BTreeMap::new(),
                    }),
                ],
            },
            None::<Vec<u8>>,
            None::<Vec<u8>>,
        )
        .await;

    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceUpdateComponentError::Error409(_)
        ))
    ));

    Ok(())
}
