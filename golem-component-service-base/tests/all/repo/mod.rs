// Copyright 2024-2025 Golem Cloud
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
use golem_common::model::component::{ComponentOwner, DefaultComponentOwner};
use golem_common::model::component_constraint::FunctionConstraintCollection;
use golem_common::model::plugin::{
    ComponentPluginInstallationTarget, ComponentPluginScope, ComponentTransformerDefinition,
    DefaultPluginOwner, DefaultPluginScope, OplogProcessorDefinition, PluginDefinition,
    PluginInstallation, PluginOwner, PluginTypeSpecificDefinition,
};
use golem_common::model::{
    AccountId, ComponentId, ComponentType, Empty, HasAccountId, PluginInstallationId,
};
use golem_common::repo::component::DefaultComponentOwnerRow;
use golem_common::repo::plugin::{DefaultPluginOwnerRow, DefaultPluginScopeRow};
use golem_common::repo::plugin_installation::ComponentPluginInstallationRow;
use golem_common::repo::RowMeta;
use golem_component_service_base::model::Component;
use golem_component_service_base::repo::component::{ComponentRecord, ComponentRepo};
use golem_component_service_base::repo::plugin::PluginRepo;
use golem_service_base::model::{ComponentName, VersionedComponentId};
use golem_service_base::repo::plugin_installation::PluginInstallationRecord;
use golem_service_base::repo::RepoError;
use poem_openapi::NewType;
use poem_openapi::__private::serde_json;
use serde::{Deserialize, Serialize};
use sqlx::query_builder::Separated;
use sqlx::{Database, Encode, QueryBuilder};
use std::collections::HashMap;
use std::fmt::Display;
use std::str::FromStr;
use std::sync::Arc;
use test_r::{inherit_test_dep, sequential_suite};
use uuid::Uuid;

pub mod constraint_data;
pub mod postgres;
pub mod sqlite;

inherit_test_dep!(Tracing);

sequential_suite!(postgres);
sequential_suite!(sqlite);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, NewType)]
struct UuidOwner(Uuid);

impl Display for UuidOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for UuidOwner {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(UuidOwner(Uuid::parse_str(s).map_err(|e| e.to_string())?))
    }
}

impl HasAccountId for UuidOwner {
    fn account_id(&self) -> AccountId {
        AccountId {
            value: self.0.to_string(),
        }
    }
}

impl ComponentOwner for UuidOwner {
    type Row = UuidOwnerRow;
    type PluginOwner = UuidOwner;
}

impl PluginOwner for UuidOwner {
    type Row = UuidOwnerRow;
}

#[derive(sqlx::FromRow, Debug, Clone)]
struct UuidOwnerRow {
    id: Uuid,
}

impl Display for UuidOwnerRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.id)
    }
}

impl TryFrom<UuidOwnerRow> for UuidOwner {
    type Error = String;

    fn try_from(value: UuidOwnerRow) -> Result<Self, Self::Error> {
        Ok(UuidOwner(value.id))
    }
}

impl From<UuidOwner> for UuidOwnerRow {
    fn from(value: UuidOwner) -> Self {
        UuidOwnerRow { id: value.0 }
    }
}

impl<DB: Database> RowMeta<DB> for UuidOwnerRow
where
    Uuid: for<'q> Encode<'q, DB> + sqlx::Type<DB>,
{
    fn add_column_list<Sep: Display>(builder: &mut Separated<DB, Sep>) {
        builder.push("owner_id");
    }

    fn add_where_clause<'a>(&'a self, builder: &mut QueryBuilder<'a, DB>) {
        builder.push("owner_id = ");
        builder.push_bind(self.id);
    }

    fn push_bind<'a, Sep: Display>(&'a self, builder: &mut Separated<'_, 'a, DB, Sep>) {
        builder.push_bind(self.id);
    }
}

pub(crate) fn get_component_data(name: &str) -> Vec<u8> {
    let path = format!("../test-components/{}.wasm", name);
    std::fs::read(path).unwrap()
}

async fn test_repo_component_id_unique(
    component_repo: Arc<dyn ComponentRepo<UuidOwner> + Sync + Send>,
) {
    let owner1 = UuidOwner(Uuid::new_v4());
    let owner2 = UuidOwner(Uuid::new_v4());

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
    )
    .unwrap();

    let mut component2 = component1.clone();
    component2.versioned_component_id.version = 1;

    let result1 = component_repo
        .create(&ComponentRecord::try_from_model(component1.clone(), true).unwrap())
        .await;
    let result2 = component_repo
        .create(&ComponentRecord::try_from_model(component2.clone(), true).unwrap())
        .await;
    let result3 = component_repo
        .create(
            &ComponentRecord::try_from_model(
                Component {
                    owner: owner2.clone(),
                    ..component1.clone()
                },
                true,
            )
            .unwrap(),
        )
        .await;

    assert!(result1.is_ok());
    assert!(result2.is_ok());
    assert!(result3.is_err());
}

async fn test_repo_component_name_unique_in_namespace(
    component_repo: Arc<dyn ComponentRepo<UuidOwner> + Sync + Send>,
) {
    let owner1 = UuidOwner(Uuid::new_v4());
    let owner2 = UuidOwner(Uuid::new_v4());

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
    )
    .unwrap();

    // Component with `component_name1` in `namespace1`
    let result1 = component_repo
        .create(&ComponentRecord::try_from_model(component1.clone(), true).unwrap())
        .await;

    // Another component with the same name in `namespace1`
    let result2 = component_repo
        .create(
            &ComponentRecord::try_from_model(
                Component {
                    versioned_component_id: VersionedComponentId {
                        component_id: ComponentId::new_v4(),
                        version: 0,
                    },
                    ..component1.clone()
                },
                true,
            )
            .unwrap(),
        )
        .await;

    // Another component with `component_name1` but in `namespace2`
    let result3 = component_repo
        .create(&ComponentRecord::try_from_model(component2.clone(), true).unwrap())
        .await;

    println!("{:?}", result1);
    println!("{:?}", result2);
    println!("{:?}", result3);

    assert!(result1.is_ok());
    assert!(result2.is_err());
    assert!(result3.is_ok());
}

async fn test_repo_component_delete(
    component_repo: Arc<dyn ComponentRepo<DefaultComponentOwner> + Sync + Send>,
) {
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
        DefaultComponentOwner,
    )
    .unwrap();

    let result1 = component_repo
        .create(&ComponentRecord::try_from_model(component1.clone(), true).unwrap())
        .await;

    let result2 = component_repo
        .get(
            &DefaultComponentOwner.to_string(),
            &component1.versioned_component_id.component_id.0,
        )
        .await;

    let result3 = component_repo
        .delete(
            &DefaultComponentOwner.to_string(),
            &component1.versioned_component_id.component_id.0,
        )
        .await;

    let result4 = component_repo
        .get(
            &DefaultComponentOwner.to_string(),
            &component1.versioned_component_id.component_id.0,
        )
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

async fn test_repo_component_constraints(
    component_repo: Arc<dyn ComponentRepo<UuidOwner> + Sync + Send>,
) {
    let owner1 = UuidOwner(Uuid::new_v4());

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
    )
    .unwrap();

    let component_constraint_initial = constraint_data::get_shopping_cart_component_constraint1(
        &owner1,
        &component1.versioned_component_id.component_id,
    );

    let component_constraint_initial_db_record = component_constraint_initial.try_into().unwrap();

    // Create Component
    let component_create_result = component_repo
        .create(&ComponentRecord::try_from_model(component1.clone(), true).unwrap())
        .await;

    // Create Constraint
    let component_constraint_create_result = component_repo
        .create_or_update_constraint(&component_constraint_initial_db_record)
        .await;

    // Get constraint
    let result_constraint_get = component_repo
        .get_constraint(
            &owner1.to_string(),
            &component1.versioned_component_id.component_id.0,
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
            &component1.versioned_component_id.component_id.0,
        )
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
    component_repo: Arc<dyn ComponentRepo<DefaultComponentOwner> + Sync + Send>,
    plugin_repo: Arc<dyn PluginRepo<DefaultPluginOwner, DefaultPluginScope> + Send + Sync>,
) -> Result<(), RepoError> {
    let owner: DefaultComponentOwner = DefaultComponentOwner;
    let plugin_owner_row: DefaultPluginOwnerRow = DefaultPluginOwner.into();

    let component_id = ComponentId::new_v4();
    let component_id2 = ComponentId::new_v4();
    let scope1: DefaultPluginScopeRow = DefaultPluginScope::Component(ComponentPluginScope {
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
    )
    .unwrap();

    component_repo
        .create(&ComponentRecord::try_from_model(component1.clone(), true).unwrap())
        .await?;
    component_repo
        .create(&ComponentRecord::try_from_model(component2.clone(), true).unwrap())
        .await?;

    let all1 = plugin_repo.get_all(&plugin_owner_row).await?;
    let scoped1 = plugin_repo
        .get_for_scope(&plugin_owner_row, &[scope1.clone()])
        .await?;
    let named1 = plugin_repo
        .get_all_with_name(&plugin_owner_row, "plugin1")
        .await?;

    let plugin1 = PluginDefinition {
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
        scope: DefaultPluginScope::Global(Empty {}),
        owner: DefaultPluginOwner,
    };
    let plugin1_row = plugin1.clone().into();

    let plugin2 = PluginDefinition {
        name: "plugin2".to_string(),
        version: "v1".to_string(),
        description: "the first test plugin".to_string(),
        icon: vec![5, 6, 7, 8],
        homepage: "https://plugin2.com".to_string(),
        specs: PluginTypeSpecificDefinition::OplogProcessor(OplogProcessorDefinition {
            component_id: component_id2.clone(),
            component_version: 0,
        }),
        scope: DefaultPluginScope::Component(ComponentPluginScope {
            component_id: component_id.clone(),
        }),
        owner: DefaultPluginOwner,
    };
    let plugin2_row = plugin2.clone().into();

    plugin_repo.create(&plugin1_row).await?;
    plugin_repo.create(&plugin2_row).await?;

    let all2 = plugin_repo.get_all(&plugin_owner_row).await?;
    let scoped2 = plugin_repo
        .get_for_scope(&plugin_owner_row, &[scope1.clone()])
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
        .collect::<Result<Vec<PluginDefinition<DefaultPluginOwner, DefaultPluginScope>>, String>>()
        .unwrap();
    defs.sort_by_key(|def| def.name.clone());

    let scoped = scoped2
        .into_iter()
        .map(|p| p.try_into())
        .collect::<Result<Vec<PluginDefinition<DefaultPluginOwner, DefaultPluginScope>>, String>>()
        .unwrap();

    let named = named2
        .into_iter()
        .map(|p| p.try_into())
        .collect::<Result<Vec<PluginDefinition<DefaultPluginOwner, DefaultPluginScope>>, String>>()
        .unwrap();

    let after_delete = all3
        .into_iter()
        .map(|p| p.try_into())
        .collect::<Result<Vec<PluginDefinition<DefaultPluginOwner, DefaultPluginScope>>, String>>()
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

async fn test_default_component_plugin_installation(
    component_repo: Arc<dyn ComponentRepo<DefaultComponentOwner> + Sync + Send>,
    plugin_repo: Arc<dyn PluginRepo<DefaultPluginOwner, DefaultPluginScope> + Send + Sync>,
) -> Result<(), RepoError> {
    let component_owner: DefaultComponentOwner = DefaultComponentOwner;
    let component_owner_row: DefaultComponentOwnerRow = component_owner.clone().into();
    let plugin_owner_row: DefaultPluginOwnerRow = component_owner_row.into();

    let plugin_owner: DefaultPluginOwner = DefaultPluginOwner;

    let component_id = ComponentId::new_v4();

    let component1 = Component::new(
        component_id.clone(),
        ComponentName("default-component-plugin-installation-component1".to_string()),
        ComponentType::Ephemeral,
        &get_component_data("shopping-cart"),
        vec![],
        vec![],
        HashMap::new(),
        component_owner.clone(),
    )
    .unwrap();

    let plugin1 = PluginDefinition {
        name: "plugin2".to_string(),
        version: "v2".to_string(),
        description: "another test plugin".to_string(),
        icon: vec![1, 2, 3, 4],
        homepage: "https://plugin2.com".to_string(),
        specs: PluginTypeSpecificDefinition::ComponentTransformer(ComponentTransformerDefinition {
            provided_wit_package: Some("wit".to_string()),
            json_schema: Some("schema".to_string()),
            validate_url: "https://plugin2.com/validate".to_string(),
            transform_url: "https://plugin2.com/transform".to_string(),
        }),
        scope: DefaultPluginScope::Global(Empty {}),
        owner: plugin_owner.clone(),
    };
    let plugin1_row = plugin1.clone().into();

    component_repo
        .create(&ComponentRecord::try_from_model(component1.clone(), true).unwrap())
        .await?;
    plugin_repo.create(&plugin1_row).await?;

    let target1 = ComponentPluginInstallationTarget {
        component_id: component_id.clone(),
        component_version: 0,
    };
    let target1_row: ComponentPluginInstallationRow = target1.clone().into();

    let installations1 = component_repo
        .get_installed_plugins(&plugin_owner_row, &component_id.0, 0)
        .await?;

    let installation1 = PluginInstallation {
        id: PluginInstallationId::new_v4(),
        name: plugin1.name.clone(),
        version: plugin1.version.clone(),
        priority: 1000,
        parameters: HashMap::from_iter(vec![("param1".to_string(), "value1".to_string())]),
    };
    let installation1_row = PluginInstallationRecord::try_from(
        installation1.clone(),
        plugin_owner_row.clone(),
        target1_row.clone(),
    )
    .unwrap();

    component_repo.install_plugin(&installation1_row).await?;

    let installation2 = PluginInstallation {
        id: PluginInstallationId::new_v4(),
        name: plugin1.name.clone(),
        version: plugin1.version.clone(),
        priority: 800,
        parameters: HashMap::default(),
    };
    let installation2_row = PluginInstallationRecord::try_from(
        installation2.clone(),
        plugin_owner_row.clone(),
        target1_row.clone(),
    )
    .unwrap();

    component_repo.install_plugin(&installation2_row).await?;

    let installations2 = component_repo
        .get_installed_plugins(&plugin_owner_row, &component_id.0, 2)
        .await?;

    println!("{:?}", installations2);

    let latest_installation2_id = installations2
        .iter()
        .find(|installation| installation.priority == 800)
        .unwrap()
        .installation_id;
    let new_params: HashMap<String, String> =
        HashMap::from_iter(vec![("param2".to_string(), "value2".to_string())]);
    component_repo
        .update_plugin_installation(
            &plugin_owner_row,
            &component_id.0,
            &latest_installation2_id,
            600,
            serde_json::to_vec(&new_params).unwrap(),
        )
        .await?;

    let installations3 = component_repo
        .get_installed_plugins(&plugin_owner_row, &component_id.0, 3)
        .await?;

    let latest_installation1_id = installations3
        .iter()
        .find(|installation| installation.priority == 1000)
        .unwrap()
        .installation_id;
    component_repo
        .uninstall_plugin(&plugin_owner_row, &component_id.0, &latest_installation1_id)
        .await?;

    let installations4 = component_repo
        .get_installed_plugins(&plugin_owner_row, &component_id.0, 4)
        .await?;

    assert_eq!(installations1.len(), 0);
    assert_eq!(installations2.len(), 2);
    assert_eq!(installations3.len(), 2);
    assert_eq!(installations4.len(), 1);
    assert_eq!(installations4[0].priority, 600);

    Ok(())
}
