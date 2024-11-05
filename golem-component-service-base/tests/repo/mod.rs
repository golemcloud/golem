// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::Tracing;
use golem_common::model::component_constraint::FunctionConstraintCollection;
use golem_common::model::plugin::{DefaultPluginOwner, DefaultPluginScope};
use golem_common::model::{ComponentId, ComponentType};
use golem_common::SafeDisplay;
use golem_component_service_base::model::Component;
use golem_component_service_base::repo::component::ComponentRepo;
use golem_component_service_base::repo::plugin::PluginRepo;
use golem_component_service_base::service::component::{
    ComponentError, ComponentService, ComponentServiceDefault, ConflictReport, ConflictingFunction,
};
use golem_component_service_base::service::component_compilation::{
    ComponentCompilationService, ComponentCompilationServiceDisabled,
};
use golem_service_base::auth::DefaultNamespace;
use golem_service_base::config::ComponentStoreLocalConfig;
use golem_service_base::model::{ComponentName, VersionedComponentId};
use golem_service_base::service::component_object_store;
use golem_wasm_ast::analysis::analysed_type::{str, u64};
use rib::RegistryKey;
use std::sync::Arc;
use test_r::inherit_test_dep;
use uuid::Uuid;

mod constraint_data;
mod postgres;
mod sqlite;

inherit_test_dep!(Tracing);

fn get_component_data(name: &str) -> Vec<u8> {
    let path = format!("../test-components/{}.wasm", name);
    std::fs::read(path).unwrap()
}

async fn test_component_constraint_incompatible_updates(
    component_repo: Arc<dyn ComponentRepo + Sync + Send>,
) {
    let object_store: Arc<dyn component_object_store::ComponentObjectStore + Sync + Send> =
        Arc::new(
            component_object_store::FsComponentObjectStore::new(&ComponentStoreLocalConfig {
                root_path: "/tmp/component".to_string(),
                object_prefix: Uuid::new_v4().to_string(),
            })
            .unwrap(),
        );

    let compilation_service: Arc<dyn ComponentCompilationService + Sync + Send> =
        Arc::new(ComponentCompilationServiceDisabled);

    let component_service: Arc<dyn ComponentService<DefaultNamespace> + Sync + Send> =
        Arc::new(ComponentServiceDefault::new(
            component_repo.clone(),
            object_store.clone(),
            compilation_service.clone(),
        ));

    let component_name = ComponentName("shopping-cart-constraint-incompatible-updates".to_string());

    // Create a shopping cart
    component_service
        .create(
            &ComponentId::new_v4(),
            &component_name,
            ComponentType::Durable,
            get_component_data("shopping-cart"),
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();

    let component_id = ComponentId::new_v4();

    let missing_function_constraint =
        constraint_data::get_random_constraint(&DefaultNamespace::default(), &component_id);

    let incompatible_constraint =
        constraint_data::get_incompatible_constraint(&DefaultNamespace::default(), &component_id);

    // Create a constraint with an unknown function (for the purpose of test), and this will act as an existing constraint of component
    component_service
        .create_or_update_constraint(&missing_function_constraint)
        .await
        .unwrap();

    // Create a constraint with an unknown function (for the purpose of test), and this will get added to the existing constraints of component
    component_service
        .create_or_update_constraint(&incompatible_constraint)
        .await
        .unwrap();

    // Update the component of shopping cart that has functions that are incompatible with the existing constraints
    let component_update_error = component_service
        .update(
            &component_id,
            get_component_data("shopping-cart"),
            None,
            &DefaultNamespace::default(),
        )
        .await
        .unwrap_err()
        .to_safe_string();

    let expected_error = ComponentError::ComponentConstraintConflictError(ConflictReport {
        missing_functions: vec![RegistryKey::FunctionName("foo".to_string())],
        conflicting_functions: vec![ConflictingFunction {
            function: RegistryKey::FunctionNameWithInterface {
                interface_name: "golem:it/api".to_string(),
                function_name: "initialize-cart".to_string(),
            },
            existing_parameter_types: vec![u64()],
            new_parameter_types: vec![str()],
            existing_result_types: vec![str()],
            new_result_types: vec![],
        }],
    })
    .to_safe_string();

    assert_eq!(component_update_error, expected_error)
}

async fn test_services(component_repo: Arc<dyn ComponentRepo + Sync + Send>) {
    let object_store: Arc<dyn component_object_store::ComponentObjectStore + Sync + Send> =
        Arc::new(
            component_object_store::FsComponentObjectStore::new(&ComponentStoreLocalConfig {
                root_path: "/tmp/component".to_string(),
                object_prefix: Uuid::new_v4().to_string(),
            })
            .unwrap(),
        );

    let compilation_service: Arc<dyn ComponentCompilationService + Sync + Send> =
        Arc::new(ComponentCompilationServiceDisabled);

    let component_service: Arc<dyn ComponentService<DefaultNamespace> + Sync + Send> =
        Arc::new(ComponentServiceDefault::new(
            component_repo.clone(),
            object_store.clone(),
            compilation_service.clone(),
        ));

    let component_name1 = ComponentName("shopping-cart-services".to_string());
    let component_name2 = ComponentName("rust-echo-services".to_string());

    let component1 = component_service
        .create(
            &ComponentId::new_v4(),
            &component_name1,
            ComponentType::Durable,
            get_component_data("shopping-cart"),
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();

    let component2 = component_service
        .create(
            &ComponentId::new_v4(),
            &component_name2,
            ComponentType::Durable,
            get_component_data("rust-echo"),
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();

    let component1_result = component_service
        .get_by_version(
            &component1.versioned_component_id,
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert!(component1_result.is_some());

    let component2_result = component_service
        .get_by_version(
            &component2.versioned_component_id,
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert!(component2_result.is_some());
    assert_eq!(component2_result.unwrap(), component2);

    let component1_result = component_service
        .get_latest_version(
            &component1.versioned_component_id.component_id,
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert!(component1_result.is_some());
    assert_eq!(component1_result.unwrap(), component1);

    let component1_result = component_service
        .get(
            &component1.versioned_component_id.component_id,
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert_eq!(component1_result.len(), 1);

    // Create constraints
    let component_constraints = constraint_data::get_shopping_cart_component_constraint1(
        &DefaultNamespace::default(),
        &component1.versioned_component_id.component_id,
    );

    let component1_constrained = component_service
        .create_or_update_constraint(&component_constraints)
        .await;

    assert!(component1_constrained.is_ok());

    // Get Constraint
    let component1_constrained = component_service
        .get_component_constraint(&component1.versioned_component_id.component_id)
        .await
        .unwrap();

    assert!(component1_constrained.is_some());

    // Update Constraint
    let component_constraints = constraint_data::get_shopping_cart_component_constraint2(
        &DefaultNamespace::default(),
        &component1.versioned_component_id.component_id,
    );

    let component1_constrained = component_service
        .create_or_update_constraint(&component_constraints)
        .await
        .unwrap();

    assert_eq!(
        component1_constrained
            .constraints
            .function_constraints
            .len(),
        2
    );

    let component1v2 = component_service
        .update(
            &component1.versioned_component_id.component_id,
            get_component_data("shopping-cart"),
            None,
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();

    let component1_result = component_service
        .get_latest_version(
            &component1.versioned_component_id.component_id,
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert!(component1_result.is_some());
    assert_eq!(component1_result.unwrap(), component1v2);

    let component1_result = component_service
        .get(
            &component1.versioned_component_id.component_id,
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert_eq!(component1_result.len(), 2);

    let component1_result = component_service
        .get_namespace(&component1.versioned_component_id.component_id)
        .await
        .unwrap();
    assert!(component1_result.is_some());
    assert_eq!(component1_result.unwrap(), DefaultNamespace::default());

    let component2_result = component_service
        .get_namespace(&component2.versioned_component_id.component_id)
        .await
        .unwrap();
    assert!(component2_result.is_some());
    assert_eq!(component2_result.unwrap(), DefaultNamespace::default());

    let component1_result = component_service
        .download(
            &component1v2.versioned_component_id.component_id,
            Some(component1v2.versioned_component_id.version),
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert!(!component1_result.is_empty());

    let component2_result = component_service
        .download(
            &component2.versioned_component_id.component_id,
            None,
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert!(!component2_result.is_empty());

    let component1_result = component_service
        .get_protected_data(
            &component1v2.versioned_component_id.component_id,
            Some(component1v2.versioned_component_id.version),
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert!(component1_result.is_some());

    let component1_result = component_service
        .get_protected_data(
            &component1v2.versioned_component_id.component_id,
            Some(10000000),
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert!(component1_result.is_none());

    let component2_result = component_service
        .get_protected_data(
            &component1v2.versioned_component_id.component_id,
            None,
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert!(component2_result.is_some());

    let component1_result = component_service
        .find_id_by_name(&component1.component_name, &DefaultNamespace::default())
        .await
        .unwrap();
    assert_eq!(
        component1_result,
        Some(component1.versioned_component_id.component_id.clone())
    );

    let component2_result = component_service
        .find_id_by_name(&component2.component_name, &DefaultNamespace::default())
        .await
        .unwrap();
    assert_eq!(
        component2_result,
        Some(component2.versioned_component_id.component_id.clone())
    );

    let component1_result = component_service
        .find_by_name(
            Some(component1.component_name.clone()),
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        component1_result,
        vec![component1.clone(), component1v2.clone()]
    );

    let component2_result = component_service
        .find_by_name(
            Some(component2.component_name.clone()),
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert_eq!(component2_result, vec![component2.clone()]);

    let component_result = component_service
        .find_by_name(None, &DefaultNamespace::default())
        .await
        .unwrap();
    assert_eq!(component_result.len(), 3);

    component_service
        .delete(
            &component1v2.versioned_component_id.component_id,
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();

    let component1_result = component_service
        .get(
            &component1.versioned_component_id.component_id,
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert!(component1_result.is_empty());

    let component1_result = component_service
        .get_protected_data(
            &component1v2.versioned_component_id.component_id,
            Some(component1v2.versioned_component_id.version),
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert!(component1_result.is_none());
}

async fn test_repo_component_id_unique(component_repo: Arc<dyn ComponentRepo + Sync + Send>) {
    let namespace1 = Uuid::new_v4().to_string();
    let namespace2 = Uuid::new_v4().to_string();

    let component_name1 = ComponentName("shopping-cart-component-id-unique".to_string());
    let data = get_component_data("shopping-cart");

    let component1 = Component::new(
        &ComponentId::new_v4(),
        &component_name1,
        ComponentType::Durable,
        &data,
        &namespace1,
    )
    .unwrap();

    let result1 = component_repo
        .create(&component1.clone().try_into().unwrap())
        .await;
    let result2 = component_repo
        .create(&component1.clone().next_version().try_into().unwrap())
        .await;
    let result3 = component_repo
        .create(
            &Component {
                namespace: namespace2.clone(),
                ..component1.clone()
            }
            .try_into()
            .unwrap(),
        )
        .await;

    assert!(result1.is_ok());
    assert!(result2.is_ok());
    assert!(result3.is_err());
}

async fn test_repo_component_name_unique_in_namespace(
    component_repo: Arc<dyn ComponentRepo + Sync + Send>,
) {
    let namespace1 = Uuid::new_v4().to_string();
    let namespace2 = Uuid::new_v4().to_string();

    let component_name1 =
        ComponentName("shopping-cart-component-name-unique-in-namespace".to_string());
    let data = get_component_data("shopping-cart");

    let component1 = Component::new(
        &ComponentId::new_v4(),
        &component_name1,
        ComponentType::Durable,
        &data,
        &namespace1,
    )
    .unwrap();
    let component2 = Component::new(
        &ComponentId::new_v4(),
        &component_name1,
        ComponentType::Durable,
        &data,
        &namespace2,
    )
    .unwrap();

    // Component with `component_name1` in `namespace1`
    let result1 = component_repo
        .create(&component1.clone().try_into().unwrap())
        .await;

    // Another component with the same name in `namespace1`
    let result2 = component_repo
        .create(
            &Component {
                versioned_component_id: VersionedComponentId {
                    component_id: ComponentId::new_v4(),
                    version: 0,
                },
                ..component1.clone()
            }
            .try_into()
            .unwrap(),
        )
        .await;

    // Another component with `component_name1` but in `namespace2`
    let result3 = component_repo
        .create(&component2.clone().try_into().unwrap())
        .await;

    println!("{:?}", result1);
    println!("{:?}", result2);
    println!("{:?}", result3);

    assert!(result1.is_ok());
    assert!(result2.is_err());
    assert!(result3.is_ok());
}

async fn test_repo_component_delete(component_repo: Arc<dyn ComponentRepo + Sync + Send>) {
    let namespace1 = Uuid::new_v4().to_string();

    let component_name1 = ComponentName("shopping-cart1-component-delete".to_string());
    let data = get_component_data("shopping-cart");

    let component1 = Component::new(
        &ComponentId::new_v4(),
        &component_name1,
        ComponentType::Durable,
        &data,
        &namespace1,
    )
    .unwrap();

    let result1 = component_repo
        .create(&component1.clone().try_into().unwrap())
        .await;

    let result2 = component_repo
        .get(&component1.versioned_component_id.component_id.0)
        .await;

    let result3 = component_repo
        .delete(
            &namespace1,
            &component1.versioned_component_id.component_id.0,
        )
        .await;

    let result4 = component_repo
        .get(&component1.versioned_component_id.component_id.0)
        .await;

    println!("{:?}", result1);
    println!("{:?}", result2);
    println!("{:?}", result3);
    println!("{:?}", result4);

    assert!(result1.is_ok());
    assert!(result2.is_ok());
    assert_eq!(result2.unwrap().len(), 1);
    assert!(result3.is_ok());
    assert!(result4.is_ok());
    assert!(result4.unwrap().is_empty());
}

async fn test_repo_component_constraints(component_repo: Arc<dyn ComponentRepo + Sync + Send>) {
    let namespace1 = Uuid::new_v4().to_string();

    let component_name1 = ComponentName("shopping-cart-component-constraints".to_string());

    // It has a function golem:it/api.{initialize-cart}(user-id: string)
    let data = get_component_data("shopping-cart");

    let component1 = Component::new(
        &ComponentId::new_v4(),
        &component_name1,
        ComponentType::Durable,
        &data,
        &namespace1,
    )
    .unwrap();

    let component_constraint_initial = constraint_data::get_shopping_cart_component_constraint1(
        &namespace1,
        &component1.versioned_component_id.component_id,
    );

    let component_constraint_initial_db_record = component_constraint_initial.try_into().unwrap();

    // Create Component
    let component_create_result = component_repo
        .create(&component1.clone().try_into().unwrap())
        .await;

    // Create Constraint
    let component_constraint_create_result = component_repo
        .create_or_update_constraint(&component_constraint_initial_db_record)
        .await;

    // Get constraint
    let result_constraint_get = component_repo
        .get_constraint(&component1.versioned_component_id.component_id)
        .await
        .unwrap();

    let expected_initial_constraint =
        Some(constraint_data::get_shopping_cart_worker_functions_constraint1());

    let component_constraint_later = constraint_data::get_shopping_cart_component_constraint2(
        &namespace1,
        &component1.versioned_component_id.component_id,
    );

    let component_constraint_later_db_record = component_constraint_later.try_into().unwrap();

    // Update constraint
    let component_constraint_update_result = component_repo
        .create_or_update_constraint(&component_constraint_later_db_record)
        .await;

    // Get updated constraint
    let result_constraint_get_updated = component_repo
        .get_constraint(&component1.versioned_component_id.component_id)
        .await
        .unwrap();

    let expected_updated_constraint = {
        let mut function_constraints =
            constraint_data::get_shopping_cart_worker_functions_constraint2().function_constraints;
        function_constraints.extend(
            constraint_data::get_shopping_cart_worker_functions_constraint1().function_constraints,
        );
        Some(FunctionConstraintCollection {
            function_constraints,
        })
    };

    assert!(component_create_result.is_ok());
    assert!(component_constraint_create_result.is_ok());
    assert_eq!(result_constraint_get, expected_initial_constraint);
    assert!(component_constraint_update_result.is_ok());
    assert_eq!(result_constraint_get_updated, expected_updated_constraint);
}

async fn test_default_plugin_repo(
    plugin_repo: Arc<dyn PluginRepo<DefaultPluginOwner, DefaultPluginScope> + Send + Sync>,
) {
}
