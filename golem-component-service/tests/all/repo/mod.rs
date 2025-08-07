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

use crate::Tracing;
use golem_common::model::component::{ComponentOwner, VersionedComponentId};
use golem_common::model::component_constraint::FunctionConstraints;
use golem_common::model::plugin::PluginScope;
use golem_common::model::plugin::{
    ComponentPluginScope, ComponentTransformerDefinition, OplogProcessorDefinition,
    PluginDefinition, PluginTypeSpecificDefinition,
};
use golem_common::model::{AccountId, ComponentId, ComponentType, Empty, PluginId, ProjectId};
use golem_common::repo::{PluginOwnerRow, PluginScopeRow};
use golem_component_service::model::{Component, ComponentByNameAndVersion, VersionType};
use golem_component_service::repo::component::{ComponentRecord, ComponentRepo};
use golem_component_service::repo::plugin::PluginRepo;
use golem_service_base::model::ComponentName;
use golem_service_base::repo::RepoError;
use std::collections::HashMap;
use std::sync::Arc;
use test_r::{inherit_test_dep, sequential_suite};
use tracing::info;
use uuid::{uuid, Uuid};

pub mod constraint_data;
pub mod postgres;
pub mod sqlite;

inherit_test_dep!(Tracing);

sequential_suite!(postgres);
sequential_suite!(sqlite);

fn random_component_owner() -> ComponentOwner {
    ComponentOwner {
        account_id: AccountId {
            value: Uuid::new_v4().to_string(),
        },
        project_id: ProjectId(Uuid::new_v4()),
    }
}

pub(crate) fn get_component_data(name: &str) -> Vec<u8> {
    let path = format!("../test-components/{name}.wasm");
    std::fs::read(path).unwrap()
}

pub(crate) fn test_component_owner() -> ComponentOwner {
    ComponentOwner {
        project_id: ProjectId(uuid!("981d4914-6992-4237-a2b3-06d7b53ed6d4")),
        account_id: AccountId {
            value: "7857d4f5-a7e1-4a26-9ff9-7755898f6dce".to_string(),
        },
    }
}

async fn test_repo_component_id_unique(component_repo: Arc<dyn ComponentRepo>) {
    let owner1 = random_component_owner();
    let owner2 = random_component_owner();

    let component_name1 = ComponentName("shopping-cart-component-id-unique".to_string());
    let data = get_component_data("shopping-cart");

    let component1 = Component::new(
        ComponentId::new_v4(),
        component_name1,
        ComponentType::Durable,
        &data,
        vec![],
        vec![],
        HashMap::new(),
        owner1.clone(),
        HashMap::new(),
        vec![],
    )
    .unwrap();

    let mut component2 = component1.clone();
    component2.versioned_component_id.version = 1;

    let result1 = component_repo
        .create(&ComponentRecord::try_from_model(component1.clone()).unwrap())
        .await;
    let result2 = component_repo
        .create(&ComponentRecord::try_from_model(component2.clone()).unwrap())
        .await;
    let result3 = component_repo
        .create(
            &ComponentRecord::try_from_model(Component {
                owner: owner2.clone(),
                ..component1.clone()
            })
            .unwrap(),
        )
        .await;

    assert!(result1.is_ok());
    assert!(result2.is_ok());
    assert!(result3.is_err());
}

async fn test_repo_component_name_unique_in_namespace(component_repo: Arc<dyn ComponentRepo>) {
    let owner1 = random_component_owner();
    let owner2 = random_component_owner();

    let component_name1 =
        ComponentName("shopping-cart-component-name-unique-in-namespace".to_string());
    let data = get_component_data("shopping-cart");

    let component1 = Component::new(
        ComponentId::new_v4(),
        component_name1.clone(),
        ComponentType::Durable,
        &data,
        vec![],
        vec![],
        HashMap::new(),
        owner1.clone(),
        HashMap::new(),
        vec![],
    )
    .unwrap();
    let component2 = Component::new(
        ComponentId::new_v4(),
        component_name1,
        ComponentType::Durable,
        &data,
        vec![],
        vec![],
        HashMap::new(),
        owner2.clone(),
        HashMap::new(),
        vec![],
    )
    .unwrap();

    // Component with `component_name1` in `namespace1`
    let result1 = component_repo
        .create(&ComponentRecord::try_from_model(component1.clone()).unwrap())
        .await;

    // Another component with the same name in `namespace1`
    let result2 = component_repo
        .create(
            &ComponentRecord::try_from_model(Component {
                versioned_component_id: VersionedComponentId {
                    component_id: ComponentId::new_v4(),
                    version: 0,
                },
                ..component1.clone()
            })
            .unwrap(),
        )
        .await;

    // Another component with `component_name1` but in `namespace2`
    let result3 = component_repo
        .create(&ComponentRecord::try_from_model(component2.clone()).unwrap())
        .await;

    info!("{:?}", result1);
    info!("{:?}", result2);
    info!("{:?}", result3);

    assert!(result1.is_ok());
    assert!(result2.is_err());
    assert!(result3.is_ok());
}

async fn test_repo_component_find_by_names(component_repo: Arc<dyn ComponentRepo>) {
    let component_name1 = ComponentName("shopping-cart".to_string());
    let data = get_component_data("shopping-cart");

    let component1 = Component::new(
        ComponentId::new_v4(),
        component_name1,
        ComponentType::Durable,
        &data,
        vec![],
        vec![],
        HashMap::new(),
        test_component_owner(),
        HashMap::new(),
        vec![],
    )
    .unwrap();

    component_repo
        .create(&ComponentRecord::try_from_model(component1.clone()).unwrap())
        .await
        .unwrap();

    let component_name2 = ComponentName("rust-echo".to_string());
    let data = get_component_data("rust-echo");

    let component2 = Component::new(
        ComponentId::new_v4(),
        component_name2,
        ComponentType::Durable,
        &data,
        vec![],
        vec![],
        HashMap::new(),
        test_component_owner(),
        HashMap::new(),
        vec![],
    )
    .unwrap();

    // rust-echo version:0
    component_repo
        .create(&ComponentRecord::try_from_model(component2.clone()).unwrap())
        .await
        .unwrap();

    // rust-echo: version: 1
    let mut component2_1 = component2.clone();
    component2_1.bump_version();
    component_repo
        .create(&ComponentRecord::try_from_model(component2_1.clone()).unwrap())
        .await
        .unwrap();

    // component 1 has only version 0
    let component1_latest = component_repo
        .get_by_names(
            &test_component_owner().to_string(),
            &[ComponentByNameAndVersion {
                component_name: component1.component_name.clone(),
                version_type: VersionType::Latest,
            }],
        )
        .await
        .unwrap();

    assert_eq!(component1_latest.len(), 1);
    assert_eq!(component1_latest[0].name, component1.component_name.0);
    assert_eq!(component1_latest[0].version, 0);

    let component1_version0 = component_repo
        .get_by_names(
            &test_component_owner().to_string(),
            &[ComponentByNameAndVersion {
                component_name: component1.component_name.clone(),
                version_type: VersionType::Exact(0),
            }],
        )
        .await
        .unwrap();

    assert_eq!(component1_version0.len(), 1);
    assert_eq!(component1_latest[0].name, component1.component_name.0);
    assert_eq!(component1_version0[0].version, 0);

    let component1_version1 = component_repo
        .get_by_names(
            &test_component_owner().to_string(),
            &[ComponentByNameAndVersion {
                component_name: component1.component_name.clone(),
                version_type: VersionType::Exact(1),
            }],
        )
        .await
        .unwrap();

    assert!(component1_version1.is_empty());

    // component 2 (this has version 0 and latest version 1)
    let component2_latest = component_repo
        .get_by_names(
            &test_component_owner().to_string(),
            &[ComponentByNameAndVersion {
                component_name: component2.component_name.clone(),
                version_type: VersionType::Latest,
            }],
        )
        .await
        .unwrap();

    assert_eq!(component2_latest.len(), 1);
    assert_eq!(component2_latest[0].name, component2.component_name.0);
    assert_eq!(component2_latest[0].version, 1);

    let component2_version0 = component_repo
        .get_by_names(
            &test_component_owner().to_string(),
            &[ComponentByNameAndVersion {
                component_name: component2.component_name.clone(),
                version_type: VersionType::Exact(0),
            }],
        )
        .await
        .unwrap();

    assert_eq!(component2_version0.len(), 1);
    assert_eq!(component2_version0[0].name, component2.component_name.0);
    assert_eq!(component2_version0[0].version, 0);

    let component2_version1 = component_repo
        .get_by_names(
            &test_component_owner().to_string(),
            &[ComponentByNameAndVersion {
                component_name: component2.component_name.clone(),
                version_type: VersionType::Exact(1),
            }],
        )
        .await
        .unwrap();

    assert_eq!(component2_version1.len(), 1);
    assert_eq!(component2_version1[0].name, component2.component_name.0);
    assert_eq!(component2_version1[0].version, 1);

    let component1_and_component_2_latest = component_repo
        .get_by_names(
            &test_component_owner().to_string(),
            &[
                ComponentByNameAndVersion {
                    component_name: component1.component_name.clone(),
                    version_type: VersionType::Latest,
                },
                ComponentByNameAndVersion {
                    component_name: component2.component_name.clone(),
                    version_type: VersionType::Latest,
                },
            ],
        )
        .await
        .unwrap();

    assert_eq!(component1_and_component_2_latest.len(), 2);
    assert_eq!(
        component1_and_component_2_latest[0].name,
        component2.component_name.0
    );
    assert_eq!(component1_and_component_2_latest[0].version, 1);
    assert_eq!(
        component1_and_component_2_latest[1].name,
        component1.component_name.0
    );
    assert_eq!(component1_and_component_2_latest[1].version, 0);

    let component1_and_component_2_exact = component_repo
        .get_by_names(
            &test_component_owner().to_string(),
            &[
                ComponentByNameAndVersion {
                    component_name: component1.component_name.clone(),
                    version_type: VersionType::Exact(0),
                },
                ComponentByNameAndVersion {
                    component_name: component2.component_name.clone(),
                    version_type: VersionType::Exact(0),
                },
            ],
        )
        .await
        .unwrap();

    assert_eq!(component1_and_component_2_exact.len(), 2);
    assert_eq!(
        component1_and_component_2_exact[0].name,
        component2.component_name.0
    );
    assert_eq!(component1_and_component_2_exact[0].version, 0);
    assert_eq!(
        component1_and_component_2_exact[1].name,
        component1.component_name.0
    );
    assert_eq!(component1_and_component_2_exact[1].version, 0);

    let component1_component_2_latest_and_exact = component_repo
        .get_by_names(
            &test_component_owner().to_string(),
            &[
                ComponentByNameAndVersion {
                    component_name: component1.component_name.clone(),
                    version_type: VersionType::Latest,
                },
                ComponentByNameAndVersion {
                    component_name: component2.component_name.clone(),
                    version_type: VersionType::Exact(0),
                },
            ],
        )
        .await
        .unwrap();

    assert_eq!(component1_component_2_latest_and_exact.len(), 2);
    assert_eq!(
        component1_component_2_latest_and_exact[0].name,
        component2.component_name.0
    );
    assert_eq!(component1_component_2_latest_and_exact[0].version, 0);
    assert_eq!(
        component1_component_2_latest_and_exact[1].name,
        component1.component_name.0
    );
    assert_eq!(component1_component_2_latest_and_exact[1].version, 0);

    // invalid search
    let invalid_search = component_repo
        .get_by_names(
            &test_component_owner().to_string(),
            &[
                ComponentByNameAndVersion {
                    component_name: component1.component_name.clone(),
                    version_type: VersionType::Exact(1),
                },
                ComponentByNameAndVersion {
                    component_name: component2.component_name.clone(),
                    version_type: VersionType::Exact(2),
                },
            ],
        )
        .await
        .unwrap();

    assert!(invalid_search.is_empty())
}

async fn test_repo_component_delete(component_repo: Arc<dyn ComponentRepo>) {
    let component_name1 = ComponentName("shopping-cart1-component-delete".to_string());
    let data = get_component_data("shopping-cart");

    let component1 = Component::new(
        ComponentId::new_v4(),
        component_name1,
        ComponentType::Durable,
        &data,
        vec![],
        vec![],
        HashMap::new(),
        test_component_owner(),
        HashMap::new(),
        vec![],
    )
    .unwrap();

    let result1 = component_repo
        .create(&ComponentRecord::try_from_model(component1.clone()).unwrap())
        .await;

    let result2 = component_repo
        .get(
            &test_component_owner().to_string(),
            component1.versioned_component_id.component_id.0,
        )
        .await;

    let result3 = component_repo
        .delete(
            &test_component_owner().to_string(),
            component1.versioned_component_id.component_id.0,
        )
        .await;

    let result4 = component_repo
        .get(
            &test_component_owner().to_string(),
            component1.versioned_component_id.component_id.0,
        )
        .await;

    info!("{:?}", result1);
    info!("{:?}", result2);
    info!("{:?}", result3);
    info!("{:?}", result4);

    assert!(result1.is_ok());
    assert!(result2.is_ok());
    assert_eq!(result2.unwrap().len(), 1);
    assert!(result3.is_ok());
    assert!(result4.is_ok());
    assert!(result4.unwrap().is_empty());
}

async fn test_repo_component_constraints(component_repo: Arc<dyn ComponentRepo>) {
    let owner1 = random_component_owner();

    let component_name1 = ComponentName("shopping-cart-component-constraints".to_string());

    // It has a function golem:it/api.{initialize-cart}(user-id: string)
    let data = get_component_data("shopping-cart");

    let component1 = Component::new(
        ComponentId::new_v4(),
        component_name1,
        ComponentType::Durable,
        &data,
        vec![],
        vec![],
        HashMap::new(),
        owner1.clone(),
        HashMap::new(),
        vec![],
    )
    .unwrap();

    let component_constraint_initial = constraint_data::get_shopping_cart_component_constraint1(
        &owner1,
        &component1.versioned_component_id.component_id,
    );

    let component_constraint_initial_db_record = component_constraint_initial.try_into().unwrap();

    // Create Component
    let component_create_result = component_repo
        .create(&ComponentRecord::try_from_model(component1.clone()).unwrap())
        .await;

    // Create Constraint
    let component_constraint_create_result = component_repo
        .create_or_update_constraint(&component_constraint_initial_db_record)
        .await;

    // Get constraint
    let result_constraint_get = component_repo
        .get_constraint(
            &owner1.to_string(),
            component1.versioned_component_id.component_id.0,
        )
        .await
        .unwrap();

    let expected_initial_constraint =
        Some(constraint_data::get_shopping_cart_worker_functions_constraint1());

    let component_constraint_later = constraint_data::get_shopping_cart_component_constraint2(
        &owner1,
        &component1.versioned_component_id.component_id,
    );

    let component_constraint_later_db_record = component_constraint_later.try_into().unwrap();

    // Update constraint
    let component_constraint_update_result = component_repo
        .create_or_update_constraint(&component_constraint_later_db_record)
        .await;

    // Get updated constraint
    let result_constraint_get_updated = component_repo
        .get_constraint(
            &owner1.to_string(),
            component1.versioned_component_id.component_id.0,
        )
        .await
        .unwrap();

    let expected_updated_constraint = {
        let mut function_constraints =
            constraint_data::get_shopping_cart_worker_functions_constraint2().constraints;
        function_constraints
            .extend(constraint_data::get_shopping_cart_worker_functions_constraint1().constraints);
        Some(FunctionConstraints {
            constraints: function_constraints,
        })
    };

    assert!(component_create_result.is_ok());
    assert!(component_constraint_create_result.is_ok());
    assert_eq!(result_constraint_get, expected_initial_constraint);
    assert!(component_constraint_update_result.is_ok());
    assert_eq!(result_constraint_get_updated, expected_updated_constraint);
}

async fn test_default_plugin_repo(
    component_repo: Arc<dyn ComponentRepo>,
    plugin_repo: Arc<dyn PluginRepo>,
) -> Result<(), RepoError> {
    let owner: ComponentOwner = test_component_owner();
    let plugin_owner_row: PluginOwnerRow = PluginOwnerRow {
        account_id: owner.account_id.value.clone(),
    };

    let component_id = ComponentId::new_v4();
    let component_id2 = ComponentId::new_v4();
    let scope1: PluginScopeRow = PluginScope::Component(ComponentPluginScope {
        component_id: component_id.clone(),
    })
    .into();

    let component1 = Component::new(
        component_id.clone(),
        ComponentName("default-plugin-repo-component1".to_string()),
        ComponentType::Ephemeral,
        &get_component_data("shopping-cart"),
        vec![],
        vec![],
        HashMap::new(),
        owner.clone(),
        HashMap::new(),
        vec![],
    )
    .unwrap();
    let component2 = Component::new(
        component_id2.clone(),
        ComponentName("default-plugin-repo-component2".to_string()),
        ComponentType::Durable,
        &get_component_data("shopping-cart"),
        vec![],
        vec![],
        HashMap::new(),
        owner.clone(),
        HashMap::new(),
        vec![],
    )
    .unwrap();

    component_repo
        .create(&ComponentRecord::try_from_model(component1.clone()).unwrap())
        .await?;
    component_repo
        .create(&ComponentRecord::try_from_model(component2.clone()).unwrap())
        .await?;

    let all1 = plugin_repo.get_all(&plugin_owner_row).await?;
    let scoped1 = plugin_repo
        .get_for_scope(&plugin_owner_row, std::slice::from_ref(&scope1))
        .await?;
    let named1 = plugin_repo
        .get_all_with_name(&plugin_owner_row, "plugin1")
        .await?;

    let plugin1 = PluginDefinition {
        id: PluginId(uuid!("76493C6B-16DA-4DC8-86B7-EF58035DDD7C")),
        name: "plugin1".to_string(),
        version: "v1".to_string(),
        description: "the first test plugin".to_string(),
        icon: vec![1, 2, 3, 4],
        homepage: "https://plugin1.com".to_string(),
        specs: PluginTypeSpecificDefinition::ComponentTransformer(ComponentTransformerDefinition {
            provided_wit_package: Some("wit".to_string()),
            json_schema: Some("schema".to_string()),
            validate_url: "https://plugin1.com/validate".to_string(),
            transform_url: "https://plugin1.com/transform".to_string(),
        }),
        scope: PluginScope::Global(Empty {}),
        owner: test_component_owner().into(),
        deleted: false,
    };
    let plugin1_row = plugin1.clone().into();

    let plugin2 = PluginDefinition {
        id: PluginId(uuid!("3DFBAAF6-D40F-4FC4-8F33-6ED4C25213B1")),
        name: "plugin2".to_string(),
        version: "v1".to_string(),
        description: "the first test plugin".to_string(),
        icon: vec![5, 6, 7, 8],
        homepage: "https://plugin2.com".to_string(),
        specs: PluginTypeSpecificDefinition::OplogProcessor(OplogProcessorDefinition {
            component_id: component_id2.clone(),
            component_version: 0,
        }),
        scope: PluginScope::Component(ComponentPluginScope {
            component_id: component_id.clone(),
        }),
        owner: test_component_owner().into(),
        deleted: false,
    };
    let plugin2_row = plugin2.clone().into();

    plugin_repo.create(&plugin1_row).await?;
    plugin_repo.create(&plugin2_row).await?;

    let all2 = plugin_repo.get_all(&plugin_owner_row).await?;
    let scoped2 = plugin_repo
        .get_for_scope(&plugin_owner_row, std::slice::from_ref(&scope1))
        .await?;
    let named2 = plugin_repo
        .get_all_with_name(&plugin_owner_row, "plugin1")
        .await?;

    plugin_repo
        .delete(&plugin_owner_row, "plugin1", "v1")
        .await?;

    let all3 = plugin_repo.get_all(&plugin_owner_row).await?;

    let mut defs = all2
        .into_iter()
        .map(|p| p.try_into())
        .collect::<Result<Vec<PluginDefinition>, String>>()
        .unwrap();
    defs.sort_by_key(|def| def.name.clone());

    let scoped = scoped2
        .into_iter()
        .map(|p| p.try_into())
        .collect::<Result<Vec<PluginDefinition>, String>>()
        .unwrap();

    let named = named2
        .into_iter()
        .map(|p| p.try_into())
        .collect::<Result<Vec<PluginDefinition>, String>>()
        .unwrap();

    let after_delete = all3
        .into_iter()
        .map(|p| p.try_into())
        .collect::<Result<Vec<PluginDefinition>, String>>()
        .unwrap();

    assert!(scoped1.is_empty());
    assert!(named1.is_empty());

    assert_eq!(defs.len(), all1.len() + 2);
    assert_eq!(scoped.len(), 1);
    assert_eq!(named.len(), 1);

    assert!(defs.contains(&plugin1));
    assert!(defs.contains(&plugin2));

    assert_eq!(scoped[0], plugin2);
    assert_eq!(named[0], plugin1);

    assert_eq!(after_delete.len(), all1.len() + 1);
    assert!(after_delete.iter().any(|p| p == &plugin2));

    Ok(())
}
