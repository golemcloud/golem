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

use crate::all::repo::sqlite::SqliteDb;
use crate::all::repo::{constraint_data, get_component_data, test_component_owner};
use crate::Tracing;
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::model::plugin::PluginScope;
use golem_common::model::plugin::{
    ComponentTransformerDefinition, OplogProcessorDefinition, PluginInstallation,
    PluginInstallationCreation,
};
use golem_common::model::{
    ComponentFilePath, ComponentFilePathWithPermissions, ComponentFilePermissions, ComponentId,
    ComponentType, Empty,
};
use golem_common::{widen_infallible, SafeDisplay};
use golem_component_service::error::ComponentError;
use golem_component_service::model::plugin::{
    AppPluginCreation, LibraryPluginCreation, PluginDefinitionCreation, PluginTypeSpecificCreation,
    PluginWasmFileReference,
};
use golem_component_service::model::{
    Component, ComponentByNameAndVersion, ConflictReport, ConflictingFunction,
    InitialComponentFilesArchiveAndPermissions, ParameterTypeConflict, ReturnTypeConflict,
    VersionType,
};
use golem_component_service::repo::component::{
    ComponentRepo, DbComponentRepo, LoggedComponentRepo,
};
use golem_component_service::repo::plugin::{DbPluginRepo, LoggedPluginRepo, PluginRepo};
use golem_component_service::service::component::ComponentService;
use golem_component_service::service::component::{ComponentServiceDefault, LazyComponentService};
use golem_component_service::service::component_compilation::{
    ComponentCompilationService, ComponentCompilationServiceDisabled,
};
use golem_component_service::service::component_object_store;
use golem_component_service::service::component_object_store::ComponentObjectStore;
use golem_component_service::service::plugin::PluginService;
use golem_component_service::service::transformer_plugin_caller::{
    ComponentTransformerResponse, TransformationFailedReason, TransformerPluginCaller,
};
use golem_service_base::clients::limit::LimitService;
use golem_service_base::model::ComponentName;
use golem_service_base::replayable_stream::ReplayableStream;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
use golem_service_base::storage::blob::BlobStorage;
use golem_wasm_ast::analysis::analysed_type::{str, u64};
use golem_wasm_ast::analysis::{AnalysedExport, AnalysedInstance};
use http::StatusCode;
use rib::{FullyQualifiedFunctionName, FunctionName, InterfaceName, PackageName};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use test_r::{inherit_test_dep, test, test_dep};
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(Arc<dyn LimitService>);

#[test_dep]
async fn db_pool() -> SqliteDb {
    SqliteDb::new().await
}

#[test_dep]
fn sqlite_component_repo(db: &SqliteDb) -> Arc<dyn ComponentRepo> {
    Arc::new(LoggedComponentRepo::new(DbComponentRepo::new(
        db.pool.clone(),
    )))
}

#[test_dep]
fn sqlite_plugin_repo(db: &SqliteDb) -> Arc<dyn PluginRepo> {
    Arc::new(LoggedPluginRepo::new(DbPluginRepo::new(db.pool.clone())))
}

#[test_dep]
fn object_store(
    blob_storage: &Arc<dyn BlobStorage + Send + Sync>,
) -> Arc<dyn ComponentObjectStore> {
    Arc::new(component_object_store::BlobStorageComponentObjectStore::new(blob_storage.clone()))
}

#[test_dep]
fn component_compilation_service() -> Arc<dyn ComponentCompilationService> {
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
fn plugin_wasm_files_service(
    blob_storage: &Arc<dyn BlobStorage + Send + Sync>,
) -> Arc<PluginWasmFilesService> {
    Arc::new(PluginWasmFilesService::new(blob_storage.clone()))
}

#[derive(Debug)]
struct FailingTransformerPluginCaller;

#[async_trait]
impl TransformerPluginCaller for FailingTransformerPluginCaller {
    async fn call_remote_transformer_plugin(
        &self,
        _component: &Component,
        _data: &[u8],
        _url: String,
        _parameters: &HashMap<String, String>,
    ) -> Result<ComponentTransformerResponse, TransformationFailedReason> {
        Err(TransformationFailedReason::HttpStatus(
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }
}

#[test_dep]
fn transformer_plugin_caller() -> Arc<dyn TransformerPluginCaller> {
    Arc::new(FailingTransformerPluginCaller)
}

#[test_dep]
fn lazy_component_service(_tracing: &Tracing) -> Arc<LazyComponentService> {
    Arc::new(LazyComponentService::new())
}

#[test_dep]
fn plugin_service(
    plugin_repo: &Arc<dyn PluginRepo>,
    library_plugin_files_service: &Arc<PluginWasmFilesService>,
    component_service: &Arc<LazyComponentService>,
) -> Arc<PluginService> {
    Arc::new(PluginService::new(
        plugin_repo.clone(),
        library_plugin_files_service.clone(),
        component_service.clone(),
    ))
}

#[test_dep]
async fn component_service(
    lazy_component_service: &Arc<LazyComponentService>,
    component_repo: &Arc<dyn ComponentRepo>,
    object_store: &Arc<dyn ComponentObjectStore>,
    component_compilation_service: &Arc<dyn ComponentCompilationService>,
    initial_component_files_service: &Arc<InitialComponentFilesService>,
    plugin_service: &Arc<PluginService>,
    plugin_wasm_files_service: &Arc<PluginWasmFilesService>,
    transformer_plugin_caller: &Arc<dyn TransformerPluginCaller>,
    limit_service: &Arc<dyn LimitService>,
    _tracing: &Tracing,
) -> Arc<dyn ComponentService> {
    lazy_component_service
        .set_implementation(ComponentServiceDefault::new(
            component_repo.clone(),
            object_store.clone(),
            component_compilation_service.clone(),
            initial_component_files_service.clone(),
            plugin_service.clone(),
            plugin_wasm_files_service.clone(),
            transformer_plugin_caller.clone(),
            limit_service.clone(),
        ))
        .await;
    lazy_component_service.clone()
}

const COMPONENT_ARCHIVE: &str = "../test-components/cli-project-yaml/data.zip";

#[test]
#[tracing::instrument]
async fn test_services(component_service: &Arc<dyn ComponentService>) {
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
            HashMap::new(),
            &test_component_owner(),
            HashMap::new(),
            vec![],
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
            HashMap::new(),
            &test_component_owner(),
            HashMap::new(),
            vec![],
        )
        .await
        .unwrap();

    let component1_result = component_service
        .get_by_version(&component1.versioned_component_id, &test_component_owner())
        .await
        .unwrap();
    assert!(component1_result.is_some());

    let component2_result = component_service
        .get_by_version(&component2.versioned_component_id, &test_component_owner())
        .await
        .unwrap();
    assert!(component2_result.is_some());
    assert_eq!(component2_result.unwrap(), component2);

    let component1_result = component_service
        .get_latest_version(
            &component1.versioned_component_id.component_id,
            &test_component_owner(),
        )
        .await
        .unwrap();
    assert!(component1_result.is_some());
    assert_eq!(component1_result.unwrap(), component1);

    let component1_result = component_service
        .get(
            &component1.versioned_component_id.component_id,
            &test_component_owner(),
        )
        .await
        .unwrap();
    assert_eq!(component1_result.len(), 1);

    // Create constraints
    let component_constraints = constraint_data::get_shopping_cart_component_constraint1(
        &test_component_owner(),
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
            &test_component_owner(),
        )
        .await
        .unwrap();

    assert!(component1_constrained.is_some());

    // Update Constraint
    let component_constraints = constraint_data::get_shopping_cart_component_constraint2(
        &test_component_owner(),
        &component1.versioned_component_id.component_id,
    );

    let component1_constrained = component_service
        .create_or_update_constraint(&component_constraints)
        .await
        .unwrap();

    assert_eq!(component1_constrained.constraints.constraints.len(), 2);

    let component1v2 = component_service
        .update(
            &component1.versioned_component_id.component_id,
            get_component_data("shopping-cart"),
            None,
            None,
            HashMap::new(),
            &test_component_owner(),
            HashMap::new(),
            vec![],
        )
        .await
        .map_err(|err| err.to_string())
        .unwrap();

    let component_search_result_1 = component_service
        .find_by_names(
            vec![
                ComponentByNameAndVersion {
                    component_name: component1.component_name.clone(),
                    version_type: VersionType::Latest,
                },
                ComponentByNameAndVersion {
                    component_name: component2.component_name.clone(),
                    version_type: VersionType::Latest,
                },
            ],
            &test_component_owner(),
        )
        .await
        .unwrap();

    assert_eq!(component_search_result_1.len(), 2);

    assert_eq!(
        component_search_result_1[0].component_name.0,
        "rust-echo-services"
    );
    assert_eq!(
        component_search_result_1[0].versioned_component_id.version,
        0
    );

    assert_eq!(
        component_search_result_1[1].component_name.0,
        "shopping-cart-services"
    );
    assert_eq!(
        component_search_result_1[1].versioned_component_id.version,
        1
    );

    let component_search_result_2 = component_service
        .find_by_names(
            vec![
                ComponentByNameAndVersion {
                    component_name: component1.component_name.clone(),
                    version_type: VersionType::Exact(0),
                },
                ComponentByNameAndVersion {
                    component_name: component2.component_name.clone(),
                    version_type: VersionType::Latest,
                },
            ],
            &test_component_owner(),
        )
        .await
        .unwrap();

    assert_eq!(component_search_result_2.len(), 2);

    assert_eq!(
        component_search_result_2[0].component_name.0,
        "rust-echo-services"
    );
    assert_eq!(
        component_search_result_2[0].versioned_component_id.version,
        0
    );

    assert_eq!(
        component_search_result_2[1].component_name.0,
        "shopping-cart-services"
    );
    assert_eq!(
        component_search_result_2[1].versioned_component_id.version,
        0
    );

    let component1_result = component_service
        .get_latest_version(
            &component1.versioned_component_id.component_id,
            &test_component_owner(),
        )
        .await
        .unwrap();
    assert!(component1_result.is_some());
    assert_eq!(component1_result.unwrap(), component1v2);

    let component1_result = component_service
        .get(
            &component1.versioned_component_id.component_id,
            &test_component_owner(),
        )
        .await
        .unwrap();
    assert_eq!(component1_result.len(), 2);

    let component1_result = component_service
        .get_owner(&component1.versioned_component_id.component_id)
        .await
        .unwrap();
    assert!(component1_result.is_some());
    assert_eq!(component1_result.unwrap(), test_component_owner());

    let component2_result = component_service
        .get_owner(&component2.versioned_component_id.component_id)
        .await
        .unwrap();
    assert!(component2_result.is_some());
    assert_eq!(component2_result.unwrap(), test_component_owner());

    let component1_result = component_service
        .download(
            &component1v2.versioned_component_id.component_id,
            Some(component1v2.versioned_component_id.version),
            &test_component_owner(),
        )
        .await
        .unwrap();
    assert!(!component1_result.is_empty());

    let component2_result = component_service
        .download(
            &component2.versioned_component_id.component_id,
            None,
            &test_component_owner(),
        )
        .await
        .unwrap();
    assert!(!component2_result.is_empty());

    let component1_result = component_service
        .download(
            &component1v2.versioned_component_id.component_id,
            Some(component1v2.versioned_component_id.version),
            &test_component_owner(),
        )
        .await;
    assert!(component1_result.is_ok());

    let component1_result = component_service
        .download(
            &component1v2.versioned_component_id.component_id,
            Some(10000000),
            &test_component_owner(),
        )
        .await;
    assert!(component1_result.is_err());

    let component2_result = component_service
        .download(
            &component1v2.versioned_component_id.component_id,
            None,
            &test_component_owner(),
        )
        .await;
    assert!(component2_result.is_ok());

    let component1_result = component_service
        .find_id_by_name(&component1.component_name, &test_component_owner())
        .await
        .unwrap();
    assert_eq!(
        component1_result,
        Some(component1.versioned_component_id.component_id.clone())
    );

    let component2_result = component_service
        .find_id_by_name(&component2.component_name, &test_component_owner())
        .await
        .unwrap();
    assert_eq!(
        component2_result,
        Some(component2.versioned_component_id.component_id.clone())
    );

    let component1_result = component_service
        .find_by_name(
            Some(component1.component_name.clone()),
            &test_component_owner(),
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
            &test_component_owner(),
        )
        .await
        .unwrap();
    assert_eq!(component2_result, vec![component2.clone()]);

    let component_result = component_service
        .find_by_name(None, &test_component_owner())
        .await
        .unwrap();

    assert!(component_result.contains(&component1));
    assert!(component_result.contains(&component1v2));
    assert!(component_result.contains(&component2));

    component_service
        .delete(
            &component1v2.versioned_component_id.component_id,
            &test_component_owner(),
        )
        .await
        .unwrap();

    let component1_result = component_service
        .get(
            &component1.versioned_component_id.component_id,
            &test_component_owner(),
        )
        .await
        .unwrap();
    assert!(component1_result.is_empty());

    let component1_result = component_service
        .download(
            &component1v2.versioned_component_id.component_id,
            Some(component1v2.versioned_component_id.version),
            &test_component_owner(),
        )
        .await;
    assert!(component1_result.is_err());
}

#[test]
#[tracing::instrument]
async fn test_initial_component_file_upload(component_service: &Arc<dyn ComponentService>) {
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
            HashMap::new(),
            &test_component_owner(),
            HashMap::new(),
            vec![],
        )
        .await
        .unwrap();

    let result = component_service
        .get_by_version(&component.versioned_component_id, &test_component_owner())
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
async fn test_initial_component_file_data_sharing(component_service: &Arc<dyn ComponentService>) {
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
            HashMap::new(),
            &test_component_owner(),
            HashMap::new(),
            vec![],
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
            HashMap::new(),
            &test_component_owner(),
            HashMap::new(),
            vec![],
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
    component_service: &Arc<dyn ComponentService>,
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
            HashMap::new(),
            &test_component_owner(),
            HashMap::new(),
            vec![],
        )
        .await
        .unwrap();

    let component_id = ComponentId::new_v4();

    let missing_function_constraint =
        constraint_data::get_random_constraint(&test_component_owner(), &component_id);

    let incompatible_constraint =
        constraint_data::get_incompatible_constraint(&test_component_owner(), &component_id);

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
            HashMap::new(),
            &test_component_owner(),
            HashMap::new(),
            vec![],
        )
        .await
        .unwrap_err()
        .to_safe_string();

    let expected_error = ComponentError::ComponentConstraintConflictError(ConflictReport {
        missing_functions: vec![FunctionName::Function(FullyQualifiedFunctionName {
            package_name: None,
            interface_name: None,
            function_name: "foo".to_string(),
        })],

        conflicting_functions: vec![ConflictingFunction {
            function: FunctionName::Function(FullyQualifiedFunctionName {
                package_name: Some(PackageName {
                    namespace: "golem".to_string(),
                    package_name: "it".to_string(),
                    version: None,
                }),
                interface_name: Some(InterfaceName {
                    name: "api".to_string(),
                    version: None,
                }),
                function_name: "initialize-cart".to_string(),
            }),
            parameter_type_conflict: Some(ParameterTypeConflict {
                existing: vec![u64()],
                new: vec![str()],
            }),
            return_type_conflict: Some(ReturnTypeConflict {
                existing: Some(str()),
                new: None,
            }),
        }],
    })
    .to_safe_string();

    assert_eq!(component_update_error, expected_error)
}

#[test]
#[tracing::instrument]
async fn test_component_oplog_process_plugin_creation(
    component_service: &Arc<dyn ComponentService>,
    plugin_service: &Arc<PluginService>,
) {
    let plugin_component_name =
        ComponentName("oplog-processor-oplog-processor-plugin-creation".to_string());
    let component_name = ComponentName("shopping-cart-oplog-processor-plugin-creation".to_string());

    // Create a component that will be used as a plugin
    let created_plugin_component = component_service
        .create(
            &ComponentId::new_v4(),
            &plugin_component_name,
            ComponentType::Durable,
            get_component_data("oplog-processor"),
            None,
            vec![],
            HashMap::new(),
            &test_component_owner(),
            HashMap::new(),
            vec![],
        )
        .await
        .unwrap();

    // create an oplog processor plugin
    let plugin_name = "oplog-processor-oplog-processor-plugin-creation";
    let plugin_version = "1";
    let plugin_priority = 0;

    let created_plugin = plugin_service
        .create_plugin(
            &test_component_owner().into(),
            PluginDefinitionCreation {
                name: plugin_name.to_string(),
                version: plugin_version.to_string(),
                description: "a plugin".to_string(),
                icon: vec![],
                homepage: "".to_string(),
                specs: PluginTypeSpecificCreation::OplogProcessor(OplogProcessorDefinition {
                    component_id: created_plugin_component.versioned_component_id.component_id,
                    component_version: created_plugin_component.versioned_component_id.version,
                }),
                scope: PluginScope::Global(Empty {}),
            },
        )
        .await
        .unwrap();

    // Create a shopping cart
    let created_component = component_service
        .create(
            &ComponentId::new_v4(),
            &component_name,
            ComponentType::Durable,
            get_component_data("shopping-cart"),
            None,
            vec![],
            HashMap::new(),
            &test_component_owner(),
            HashMap::new(),
            vec![],
        )
        .await
        .unwrap();

    // install oplog processer plugin to the shopping cart
    component_service
        .create_plugin_installation_for_component(
            &test_component_owner(),
            &created_component.versioned_component_id.component_id,
            PluginInstallationCreation {
                name: plugin_name.to_string(),
                version: plugin_version.to_string(),
                parameters: HashMap::new(),
                priority: plugin_priority,
            },
        )
        .await
        .unwrap();

    // get component and check we have an installation
    let final_component = component_service
        .get_latest_version(
            &created_component.versioned_component_id.component_id,
            &created_component.owner,
        )
        .await
        .unwrap();
    let installed_plugins = final_component.expect("no component").installed_plugins;
    assert_eq!(installed_plugins.len(), 1);
    assert!(matches!(&installed_plugins[0], PluginInstallation {
        plugin_id,
        priority,
        ..
    } if *plugin_id == created_plugin.id && *priority == plugin_priority))
}

#[test]
#[tracing::instrument]
async fn test_component_oplog_process_plugin_creation_invalid_plugin(
    component_service: &Arc<dyn ComponentService>,
    plugin_service: &Arc<PluginService>,
) {
    let plugin_component_name =
        ComponentName("oplog-processor-oplog-processor-plugin-creation-invalid-plugin".to_string());

    // Create a component that will be used as a plugin. The component _does not_ implement the required interfaces
    let created_plugin_component = component_service
        .create(
            &ComponentId::new_v4(),
            &plugin_component_name,
            ComponentType::Durable,
            get_component_data("shopping-cart"),
            None,
            vec![],
            HashMap::new(),
            &test_component_owner(),
            HashMap::new(),
            vec![],
        )
        .await
        .unwrap();

    // create an oplog processor plugin
    let plugin_name = "oplog-processor-oplog-processor-plugin-creation-invalid-plugin";
    let plugin_version = "1";

    let result = plugin_service
        .create_plugin(
            &test_component_owner().into(),
            PluginDefinitionCreation {
                name: plugin_name.to_string(),
                version: plugin_version.to_string(),
                description: "a plugin".to_string(),
                icon: vec![],
                homepage: "".to_string(),
                specs: PluginTypeSpecificCreation::OplogProcessor(OplogProcessorDefinition {
                    component_id: created_plugin_component.versioned_component_id.component_id,
                    component_version: created_plugin_component.versioned_component_id.version,
                }),
                scope: PluginScope::Global(Empty {}),
            },
        )
        .await;

    assert!(matches!(
        result,
        Err(ComponentError::InvalidOplogProcessorPlugin)
    ));
}

#[test]
#[tracing::instrument]
// happy path is tested in integration tests using a real web server.
async fn test_failing_component_transformer_plugin(
    component_service: &Arc<dyn ComponentService>,
    plugin_service: &Arc<PluginService>,
) {
    let plugin_component_name =
        ComponentName("failing-component-transformer-component".to_string());

    // Create a component that can be composed with library
    let created_component = component_service
        .create(
            &ComponentId::new_v4(),
            &plugin_component_name,
            ComponentType::Durable,
            get_component_data("shopping-cart"),
            None,
            vec![],
            HashMap::new(),
            &test_component_owner(),
            HashMap::new(),
            vec![],
        )
        .await
        .unwrap();

    // create a library plugin
    let plugin_name = "failing-component-transformer-plugin";
    let plugin_version = "1";

    plugin_service
        .create_plugin(
            &test_component_owner().into(),
            PluginDefinitionCreation {
                name: plugin_name.to_string(),
                version: plugin_version.to_string(),
                description: "a plugin".to_string(),
                icon: vec![],
                homepage: "".to_string(),
                specs: PluginTypeSpecificCreation::ComponentTransformer(
                    ComponentTransformerDefinition {
                        provided_wit_package: None,
                        json_schema: None,
                        validate_url: "http://localhost:9000/validate".to_string(),
                        transform_url: "http://localhost:9000/transform".to_string(),
                    },
                ),
                scope: PluginScope::Global(Empty {}),
            },
        )
        .await
        .unwrap();

    let result = component_service
        .create_plugin_installation_for_component(
            &test_component_owner(),
            &created_component.versioned_component_id.component_id,
            PluginInstallationCreation {
                name: plugin_name.to_string(),
                version: plugin_version.to_string(),
                parameters: HashMap::new(),
                priority: 0,
            },
        )
        .await;

    assert!(matches!(
        result,
        Err(ComponentError::TransformationFailed(_))
    ));
}

#[test]
#[tracing::instrument]
async fn test_library_plugin_creation(
    component_service: &Arc<dyn ComponentService>,
    plugin_service: &Arc<PluginService>,
) {
    let plugin_component_name = ComponentName("library-plugin-creation-app".to_string());

    // Create a component that can be composed with library
    let created_component = component_service
        .create(
            &ComponentId::new_v4(),
            &plugin_component_name,
            ComponentType::Durable,
            get_component_data("app_and_library_app"),
            None,
            vec![],
            HashMap::new(),
            &test_component_owner(),
            HashMap::new(),
            vec![],
        )
        .await
        .unwrap();

    // create a library plugin
    let plugin_name = "library-plugin-creation-library";
    let plugin_version = "1";

    let library_plugin_stream = Bytes::from(get_component_data("app_and_library_library"))
        .map_item(|i| i.map_err(widen_infallible::<String>))
        .map_error(widen_infallible::<String>)
        .erased();

    plugin_service
        .create_plugin(
            &test_component_owner().into(),
            PluginDefinitionCreation {
                name: plugin_name.to_string(),
                version: plugin_version.to_string(),
                description: "a plugin".to_string(),
                icon: vec![],
                homepage: "".to_string(),
                specs: PluginTypeSpecificCreation::Library(LibraryPluginCreation {
                    data: PluginWasmFileReference::Data(Box::new(library_plugin_stream)),
                }),
                scope: PluginScope::Global(Empty {}),
            },
        )
        .await
        .unwrap();

    component_service
        .create_plugin_installation_for_component(
            &test_component_owner(),
            &created_component.versioned_component_id.component_id,
            PluginInstallationCreation {
                name: plugin_name.to_string(),
                version: plugin_version.to_string(),
                parameters: HashMap::new(),
                priority: 0,
            },
        )
        .await
        .unwrap();

    // get component and check it now implements the old interface
    let final_component = component_service
        .get_latest_version(
            &created_component.versioned_component_id.component_id,
            &created_component.owner,
        )
        .await
        .unwrap()
        .expect("plugin not found");

    let exports = final_component.metadata.exports();

    assert_eq!(exports.len(), 1);
    assert!(matches!(
        &exports[0],
        AnalysedExport::Instance(AnalysedInstance {
            name,
            ..
        }) if name == "it:app-and-library-app/app-api"
    ));
}

#[test]
#[tracing::instrument]
async fn test_app_plugin_creation(
    component_service: &Arc<dyn ComponentService>,
    plugin_service: &Arc<PluginService>,
) {
    let plugin_component_name = ComponentName("app-plugin-creation-library".to_string());

    // Create a component that will be composed with the app plugin
    let created_component = component_service
        .create(
            &ComponentId::new_v4(),
            &plugin_component_name,
            ComponentType::Durable,
            get_component_data("app_and_library_library"),
            None,
            vec![],
            HashMap::new(),
            &test_component_owner(),
            HashMap::new(),
            vec![],
        )
        .await
        .unwrap();

    // create a library plugin
    let plugin_name = "app-plugin-creation-app";
    let plugin_version = "1";

    let app_plugin_stream = Bytes::from(get_component_data("app_and_library_app"))
        .map_item(|i| i.map_err(widen_infallible::<String>))
        .map_error(widen_infallible::<String>)
        .erased();

    plugin_service
        .create_plugin(
            &test_component_owner().into(),
            PluginDefinitionCreation {
                name: plugin_name.to_string(),
                version: plugin_version.to_string(),
                description: "a plugin".to_string(),
                icon: vec![],
                homepage: "".to_string(),
                specs: PluginTypeSpecificCreation::App(AppPluginCreation {
                    data: PluginWasmFileReference::Data(Box::new(app_plugin_stream)),
                }),
                scope: PluginScope::Global(Empty {}),
            },
        )
        .await
        .unwrap();

    component_service
        .create_plugin_installation_for_component(
            &test_component_owner(),
            &created_component.versioned_component_id.component_id,
            PluginInstallationCreation {
                name: plugin_name.to_string(),
                version: plugin_version.to_string(),
                parameters: HashMap::new(),
                priority: 0,
            },
        )
        .await
        .unwrap();

    // get component and check it now implements the new interface
    let final_component = component_service
        .get_latest_version(
            &created_component.versioned_component_id.component_id,
            &created_component.owner,
        )
        .await
        .unwrap()
        .expect("plugin not found");

    let exports = final_component.metadata.exports();

    assert_eq!(exports.len(), 1);
    assert!(matches!(
        &exports[0],
        AnalysedExport::Instance(AnalysedInstance {
            name,
            ..
        }) if name == "it:app-and-library-app/app-api"
    ));
}
