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

use test_r::{inherit_test_dep, test, test_dep};

use crate::all::repo::sqlite::SqliteDb;
use crate::all::repo::{constraint_data, get_component_data};
use crate::Tracing;
use golem_common::model::component::DefaultComponentOwner;
use golem_common::model::plugin::{DefaultPluginOwner, DefaultPluginScope};
use golem_common::model::{
    ComponentFilePath, ComponentFilePathWithPermissions, ComponentFilePermissions, ComponentId,
    ComponentType,
};
use golem_common::SafeDisplay;
use golem_component_service_base::config::ComponentStoreLocalConfig;
use golem_component_service_base::model::InitialComponentFilesArchiveAndPermissions;
use golem_component_service_base::repo::component::{
    ComponentRepo, DbComponentRepo, LoggedComponentRepo,
};
use golem_component_service_base::repo::plugin::{DbPluginRepo, LoggedPluginRepo, PluginRepo};
use golem_component_service_base::service::component::{
    ComponentError, ComponentService, ComponentServiceDefault, ConflictReport, ConflictingFunction,
};
use golem_component_service_base::service::component_compilation::{
    ComponentCompilationService, ComponentCompilationServiceDisabled,
};
use golem_component_service_base::service::component_object_store;
use golem_component_service_base::service::component_object_store::ComponentObjectStore;
use golem_component_service_base::service::plugin::{PluginService, PluginServiceDefault};
use golem_service_base::model::ComponentName;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
use golem_service_base::storage::blob::BlobStorage;
use golem_wasm_ast::analysis::analysed_type::{str, u64};
use rib::RegistryKey;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

inherit_test_dep!(Tracing);

#[test_dep]
async fn db_pool() -> SqliteDb {
    SqliteDb::new().await
}

#[test_dep]
fn sqlite_component_repo(
    db: &SqliteDb,
) -> Arc<dyn ComponentRepo<DefaultComponentOwner> + Send + Sync> {
    Arc::new(LoggedComponentRepo::new(DbComponentRepo::new(
        db.pool.clone(),
    )))
}

#[test_dep]
fn sqlite_plugin_repo(
    db: &SqliteDb,
) -> Arc<dyn PluginRepo<DefaultPluginOwner, DefaultPluginScope> + Send + Sync> {
    Arc::new(LoggedPluginRepo::new(DbPluginRepo::new(db.pool.clone())))
}

#[test_dep]
fn object_store() -> Arc<dyn ComponentObjectStore + Send + Sync> {
    Arc::new(
        component_object_store::FsComponentObjectStore::new(&ComponentStoreLocalConfig {
            root_path: "/tmp/component".to_string(),
            object_prefix: Uuid::new_v4().to_string(),
        })
        .unwrap(),
    )
}

#[test_dep]
fn component_compilation_service() -> Arc<dyn ComponentCompilationService + Send + Sync> {
    Arc::new(ComponentCompilationServiceDisabled)
}

#[test_dep]
async fn blob_storage() -> Arc<dyn BlobStorage + Send + Sync> {
    Arc::new(
        FileSystemBlobStorage::new(&PathBuf::from(format!("/tmp/blob-{}", Uuid::new_v4())))
            .await
            .expect("Failed to create blob storage"),
    )
}

#[test_dep]
fn initial_component_files_service(
    blob_storage: &Arc<dyn BlobStorage + Send + Sync>,
) -> Arc<InitialComponentFilesService> {
    Arc::new(InitialComponentFilesService::new(blob_storage.clone()))
}

#[test_dep]
fn plugin_service(
    plugin_repo: &Arc<dyn PluginRepo<DefaultPluginOwner, DefaultPluginScope> + Send + Sync>,
) -> Arc<dyn PluginService<DefaultPluginOwner, DefaultPluginScope> + Send + Sync> {
    Arc::new(PluginServiceDefault::new(plugin_repo.clone()))
}

#[test_dep]
fn component_service(
    component_repo: &Arc<dyn ComponentRepo<DefaultComponentOwner> + Send + Sync>,
    object_store: &Arc<dyn ComponentObjectStore + Send + Sync>,
    component_compilation_service: &Arc<dyn ComponentCompilationService + Send + Sync>,
    initial_component_files_service: &Arc<InitialComponentFilesService>,
    plugin_service: &Arc<dyn PluginService<DefaultPluginOwner, DefaultPluginScope> + Send + Sync>,
    _tracing: &Tracing,
) -> Arc<dyn ComponentService<DefaultComponentOwner> + Send + Sync> {
    Arc::new(ComponentServiceDefault::new(
        component_repo.clone(),
        object_store.clone(),
        component_compilation_service.clone(),
        initial_component_files_service.clone(),
        plugin_service.clone(),
    ))
}

const COMPONENT_ARCHIVE: &str = "../test-components/cli-project-yaml/data.zip";

#[test]
#[tracing::instrument]
async fn test_services(
    component_service: &Arc<dyn ComponentService<DefaultComponentOwner> + Send + Sync>,
) {
    let component_name1 = ComponentName("shopping-cart-services".to_string());
    let component_name2 = ComponentName("rust-echo-services".to_string());

    let component1 = component_service
        .create(
            &ComponentId::new_v4(),
            &component_name1,
            ComponentType::Durable,
            get_component_data("shopping-cart"),
            None,
            vec![],
            &DefaultComponentOwner,
        )
        .await
        .unwrap();

    let component2 = component_service
        .create(
            &ComponentId::new_v4(),
            &component_name2,
            ComponentType::Durable,
            get_component_data("rust-echo"),
            None,
            vec![],
            &DefaultComponentOwner,
        )
        .await
        .unwrap();

    let component1_result = component_service
        .get_by_version(&component1.versioned_component_id, &DefaultComponentOwner)
        .await
        .unwrap();
    assert!(component1_result.is_some());

    let component2_result = component_service
        .get_by_version(&component2.versioned_component_id, &DefaultComponentOwner)
        .await
        .unwrap();
    assert!(component2_result.is_some());
    assert_eq!(component2_result.unwrap(), component2);

    let component1_result = component_service
        .get_latest_version(
            &component1.versioned_component_id.component_id,
            &DefaultComponentOwner,
        )
        .await
        .unwrap();
    assert!(component1_result.is_some());
    assert_eq!(component1_result.unwrap(), component1);

    let component1_result = component_service
        .get(
            &component1.versioned_component_id.component_id,
            &DefaultComponentOwner,
        )
        .await
        .unwrap();
    assert_eq!(component1_result.len(), 1);

    // Create constraints
    let component_constraints = constraint_data::get_shopping_cart_component_constraint1(
        &DefaultComponentOwner,
        &component1.versioned_component_id.component_id,
    );

    let component1_constrained = component_service
        .create_or_update_constraint(&component_constraints)
        .await;

    assert!(component1_constrained.is_ok());

    // Get Constraint
    let component1_constrained = component_service
        .get_component_constraint(
            &component1.versioned_component_id.component_id,
            &DefaultComponentOwner,
        )
        .await
        .unwrap();

    assert!(component1_constrained.is_some());

    // Update Constraint
    let component_constraints = constraint_data::get_shopping_cart_component_constraint2(
        &DefaultComponentOwner,
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
            None,
            &DefaultComponentOwner,
        )
        .await
        .unwrap();

    let component1_result = component_service
        .get_latest_version(
            &component1.versioned_component_id.component_id,
            &DefaultComponentOwner,
        )
        .await
        .unwrap();
    assert!(component1_result.is_some());
    assert_eq!(component1_result.unwrap(), component1v2);

    let component1_result = component_service
        .get(
            &component1.versioned_component_id.component_id,
            &DefaultComponentOwner,
        )
        .await
        .unwrap();
    assert_eq!(component1_result.len(), 2);

    let component1_result = component_service
        .get_owner(&component1.versioned_component_id.component_id)
        .await
        .unwrap();
    assert!(component1_result.is_some());
    assert_eq!(component1_result.unwrap(), DefaultComponentOwner);

    let component2_result = component_service
        .get_owner(&component2.versioned_component_id.component_id)
        .await
        .unwrap();
    assert!(component2_result.is_some());
    assert_eq!(component2_result.unwrap(), DefaultComponentOwner);

    let component1_result = component_service
        .download(
            &component1v2.versioned_component_id.component_id,
            Some(component1v2.versioned_component_id.version),
            &DefaultComponentOwner,
        )
        .await
        .unwrap();
    assert!(!component1_result.is_empty());

    let component2_result = component_service
        .download(
            &component2.versioned_component_id.component_id,
            None,
            &DefaultComponentOwner,
        )
        .await
        .unwrap();
    assert!(!component2_result.is_empty());

    let component1_result = component_service
        .download(
            &component1v2.versioned_component_id.component_id,
            Some(component1v2.versioned_component_id.version),
            &DefaultComponentOwner,
        )
        .await;
    assert!(component1_result.is_ok());

    let component1_result = component_service
        .download(
            &component1v2.versioned_component_id.component_id,
            Some(10000000),
            &DefaultComponentOwner,
        )
        .await;
    assert!(component1_result.is_err());

    let component2_result = component_service
        .download(
            &component1v2.versioned_component_id.component_id,
            None,
            &DefaultComponentOwner,
        )
        .await;
    assert!(component2_result.is_ok());

    let component1_result = component_service
        .find_id_by_name(&component1.component_name, &DefaultComponentOwner)
        .await
        .unwrap();
    assert_eq!(
        component1_result,
        Some(component1.versioned_component_id.component_id.clone())
    );

    let component2_result = component_service
        .find_id_by_name(&component2.component_name, &DefaultComponentOwner)
        .await
        .unwrap();
    assert_eq!(
        component2_result,
        Some(component2.versioned_component_id.component_id.clone())
    );

    let component1_result = component_service
        .find_by_name(
            Some(component1.component_name.clone()),
            &DefaultComponentOwner,
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
            &DefaultComponentOwner,
        )
        .await
        .unwrap();
    assert_eq!(component2_result, vec![component2.clone()]);

    let component_result = component_service
        .find_by_name(None, &DefaultComponentOwner)
        .await
        .unwrap();

    assert!(component_result.contains(&component1));
    assert!(component_result.contains(&component1v2));
    assert!(component_result.contains(&component2));

    component_service
        .delete(
            &component1v2.versioned_component_id.component_id,
            &DefaultComponentOwner,
        )
        .await
        .unwrap();

    let component1_result = component_service
        .get(
            &component1.versioned_component_id.component_id,
            &DefaultComponentOwner,
        )
        .await
        .unwrap();
    assert!(component1_result.is_empty());

    let component1_result = component_service
        .download(
            &component1v2.versioned_component_id.component_id,
            Some(component1v2.versioned_component_id.version),
            &DefaultComponentOwner,
        )
        .await;
    assert!(component1_result.is_err());
}

#[test]
#[tracing::instrument]
async fn test_initial_component_file_upload(
    component_service: &Arc<dyn ComponentService<DefaultComponentOwner> + Send + Sync>,
) {
    let data = get_component_data("shopping-cart");

    let component_name = ComponentName("shopping-cart-initial-component-file-upload".to_string());
    let component_id = ComponentId::new_v4();

    let named_temp_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::copy(COMPONENT_ARCHIVE, &named_temp_file).unwrap();

    let component = component_service
        .create(
            &component_id,
            &component_name,
            ComponentType::Durable,
            data,
            Some(InitialComponentFilesArchiveAndPermissions {
                archive: named_temp_file,
                files: vec![ComponentFilePathWithPermissions {
                    path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
                    permissions: ComponentFilePermissions::ReadWrite,
                }],
            }),
            vec![],
            &DefaultComponentOwner,
        )
        .await
        .unwrap();

    let result = component_service
        .get_by_version(&component.versioned_component_id, &DefaultComponentOwner)
        .await
        .unwrap();

    assert!(result.is_some());

    let result = result
        .unwrap()
        .files
        .into_iter()
        .map(|f| (f.path, f.permissions))
        .collect::<Vec<_>>();
    assert_eq!(result.len(), 2);
    assert!(result.contains(&(
        ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
        ComponentFilePermissions::ReadWrite
    )));
    assert!(result.contains(&(
        ComponentFilePath::from_abs_str("/bar/baz.txt").unwrap(),
        ComponentFilePermissions::ReadOnly
    )));
}

#[test]
#[tracing::instrument]
async fn test_initial_component_file_data_sharing(
    component_service: &Arc<dyn ComponentService<DefaultComponentOwner> + Send + Sync>,
) {
    let data = get_component_data("shopping-cart");

    let component_name = ComponentName("test_initial_component_file_data_sharing".to_string());
    let component_id = ComponentId::new_v4();

    let named_temp_file1 = tempfile::NamedTempFile::new().unwrap();
    std::fs::copy(COMPONENT_ARCHIVE, &named_temp_file1).unwrap();

    let component1 = component_service
        .create(
            &component_id,
            &component_name,
            ComponentType::Durable,
            data.clone(),
            Some(InitialComponentFilesArchiveAndPermissions {
                archive: named_temp_file1,
                files: vec![],
            }),
            vec![],
            &DefaultComponentOwner,
        )
        .await
        .unwrap();

    let named_temp_file2 = tempfile::NamedTempFile::new().unwrap();
    std::fs::copy(COMPONENT_ARCHIVE, &named_temp_file2).unwrap();

    let component2 = component_service
        .update(
            &component_id,
            data,
            None,
            Some(InitialComponentFilesArchiveAndPermissions {
                archive: named_temp_file2,
                files: vec![ComponentFilePathWithPermissions {
                    path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
                    permissions: ComponentFilePermissions::ReadWrite,
                }],
            }),
            &DefaultComponentOwner,
        )
        .await
        .unwrap();

    assert_eq!(component1.files.len(), 2);
    assert_eq!(component2.files.len(), 2);

    // the uploads contain the same files, so their keys should be the same
    let component1_keys = component1
        .files
        .into_iter()
        .map(|f| f.key.0)
        .collect::<HashSet<_>>();
    let component2_keys = component2
        .files
        .into_iter()
        .map(|f| f.key.0)
        .collect::<HashSet<_>>();
    assert_eq!(component1_keys, component2_keys);
}

#[test]
#[tracing::instrument]
async fn test_component_constraint_incompatible_updates(
    component_service: &Arc<dyn ComponentService<DefaultComponentOwner> + Send + Sync>,
) {
    let component_name = ComponentName("shopping-cart-constraint-incompatible-updates".to_string());

    // Create a shopping cart
    component_service
        .create(
            &ComponentId::new_v4(),
            &component_name,
            ComponentType::Durable,
            get_component_data("shopping-cart"),
            None,
            vec![],
            &DefaultComponentOwner,
        )
        .await
        .unwrap();

    let component_id = ComponentId::new_v4();

    let missing_function_constraint =
        constraint_data::get_random_constraint(&DefaultComponentOwner, &component_id);

    let incompatible_constraint =
        constraint_data::get_incompatible_constraint(&DefaultComponentOwner, &component_id);

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
            None,
            &DefaultComponentOwner,
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
