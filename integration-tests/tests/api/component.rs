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
use assert2::{assert, check};
use golem_api_grpc::proto::golem::component::v1::{
    GetComponentRequest, GetComponentsRequest, GetLatestComponentRequest,
};
use golem_api_grpc::proto::golem::component::Component;
use golem_common::model::component_metadata::{
    DynamicLinkedInstance, DynamicLinkedWasmRpc, WasmRpcTarget,
};
use golem_common::model::{
    AccountId, ComponentFilePermissions, ComponentId, ComponentType, InitialComponentFile,
};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslUnsafe;
use std::collections::HashMap;
use test_r::{inherit_test_dep, test};
use tokio::join;
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn get_components_many_component(deps: &EnvBasedTestDependencies) {
    // Create some components
    let (counter_1_id, counter_2_id, caller_id, ephemeral_id) = join!(
        deps.component("counters").unique().store(),
        deps.component("counters").unique().store(),
        deps.component("caller")
            .unique()
            .with_dynamic_linking(&[
                (
                    "rpc:counters-client/counters-client",
                    DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                        targets: HashMap::from_iter(vec![
                            (
                                "api".to_string(),
                                WasmRpcTarget {
                                    interface_name: "rpc:counters-exports/api".to_string(),
                                    component_name: "rpc:counters".to_string(),
                                    component_type: ComponentType::Durable,
                                },
                            ),
                            (
                                "counter".to_string(),
                                WasmRpcTarget {
                                    interface_name: "rpc:counters-exports/api".to_string(),
                                    component_name: "rpc:counters".to_string(),
                                    component_type: ComponentType::Durable,
                                },
                            ),
                        ]),
                    }),
                ),
                (
                    "rpc:ephemeral-client/ephemeral-client",
                    DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                        targets: HashMap::from_iter(vec![(
                            "api".to_string(),
                            WasmRpcTarget {
                                interface_name: "rpc:ephemeral-exports/api".to_string(),
                                component_name: "rpc:ephemeral".to_string(),
                                component_type: ComponentType::Ephemeral
                            }
                        )]),
                    }),
                ),
            ])
            .store(),
        deps.component("ephemeral").unique().ephemeral().store()
    );

    let counter_1_id = common_component_id_to_str(&counter_1_id);
    let counter_2_id = common_component_id_to_str(&counter_2_id);
    let caller_id = common_component_id_to_str(&caller_id);
    let ephemeral_id = common_component_id_to_str(&ephemeral_id);

    // Get components
    let components = deps
        .component_service()
        .get_components(GetComponentsRequest {
            project_id: None,
            component_name: None,
        })
        .await
        .unwrap();

    let components = components
        .into_iter()
        .map(|component| {
            (
                grpc_component_id_to_str(
                    component
                        .versioned_component_id
                        .as_ref()
                        .unwrap()
                        .component_id
                        .as_ref()
                        .unwrap(),
                ),
                component,
            )
        })
        .collect::<HashMap<_, _>>();

    // Check that we have all the components with some meta (we check equal or more,
    // so tests can run in parallel)
    assert!(components.len() >= 4);

    let counter_1 = components.get(&counter_1_id).unwrap();
    let counter_2 = components.get(&counter_2_id).unwrap();
    let caller = components.get(&caller_id).unwrap();
    let ephemeral = components.get(&ephemeral_id).unwrap();

    check!(counter_1.component_type == Some(ComponentType::Durable as i32));
    check!(counter_2.component_type == Some(ComponentType::Durable as i32));
    check!(caller.component_type == Some(ComponentType::Durable as i32));
    check!(ephemeral.component_type == Some(ComponentType::Ephemeral as i32));

    check!(counter_1.versioned_component_id.unwrap().version == 0);
    check!(counter_2.versioned_component_id.unwrap().version == 0);
    check!(caller.versioned_component_id.unwrap().version == 0);
    check!(ephemeral.versioned_component_id.unwrap().version == 0);

    check!(counter_1.component_size > 0);
    check!(counter_1.component_size == counter_2.component_size);
    check!(caller.component_size > 0);
    check!(ephemeral.component_size > 0);

    let counter_1_meta = &counter_1.metadata.as_ref().unwrap();
    let counter_2_meta = &counter_2.metadata.as_ref().unwrap();
    let caller_meta = &caller.metadata.as_ref().unwrap();
    let ephemeral_meta = &ephemeral.metadata.as_ref().unwrap();

    check!(counter_1_meta.exports.len() > 0);
    check!(counter_2_meta.exports.len() == counter_2_meta.exports.len());
    check!(caller_meta.exports.len() > 0);
    check!(ephemeral_meta.exports.len() > 0);

    check!(counter_1_meta.dynamic_linking.len() == 0);
    check!(counter_2_meta.dynamic_linking.len() == 0);
    check!(caller_meta.dynamic_linking.len() == 2);

    check!(
        DynamicLinkedInstance::try_from(
            caller_meta
                .dynamic_linking
                .get("rpc:counters-client/counters-client")
                .unwrap()
                .clone()
        )
        .unwrap()
            == DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                targets: HashMap::from_iter(vec![
                    (
                        "api".to_string(),
                        WasmRpcTarget {
                            interface_name: "rpc:counters-exports/api".to_string(),
                            component_name: "rpc:counters".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "counter".to_string(),
                        WasmRpcTarget {
                            interface_name: "rpc:counters-exports/api".to_string(),
                            component_name: "rpc:counters".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                ]),
            }),
    );
    check!(
        DynamicLinkedInstance::try_from(
            caller_meta
                .dynamic_linking
                .get("rpc:ephemeral-client/ephemeral-client")
                .unwrap()
                .clone(),
        )
        .unwrap()
            == DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                targets: HashMap::from_iter(vec![(
                    "api".to_string(),
                    WasmRpcTarget {
                        interface_name: "rpc:ephemeral-exports/api".to_string(),
                        component_name: "rpc:ephemeral".to_string(),
                        component_type: ComponentType::Ephemeral,
                    }
                )]),
            }),
    );
    check!(ephemeral_meta.dynamic_linking.len() == 0);
}

#[test]
#[tracing::instrument]
async fn get_components_many_versions(deps: &EnvBasedTestDependencies) {
    // Create component
    let (component_id, component_name) = deps
        .component("counters")
        .unique()
        .store_and_get_name()
        .await;

    // Search for the component by name
    let components = deps
        .component_service()
        .get_components(GetComponentsRequest {
            project_id: None,
            component_name: Some(component_name.0.clone()),
        })
        .await
        .unwrap();

    // Check that we only have 1 version of the component
    assert!(components.len() == 1);
    check_versioned_id(&components[0], &component_id, 0);

    // Update component two times
    deps.update_component(&component_id, "counters").await;
    deps.update_component(&component_id, "counters").await;

    // Search for the component by name again
    let components = deps
        .component_service()
        .get_components(GetComponentsRequest {
            project_id: None,
            component_name: Some(component_name.0.clone()),
        })
        .await
        .unwrap();

    // Check that we have all versions of the component
    assert!(components.len() == 3);
    check_versioned_id(&components[0], &component_id, 0);
    check_versioned_id(&components[1], &component_id, 1);
    check_versioned_id(&components[2], &component_id, 2);
}

#[test]
#[tracing::instrument]
async fn get_component_latest_version(deps: &EnvBasedTestDependencies) {
    // Create component
    let (component_id, _) = deps
        .component("counters")
        .unique()
        .store_and_get_name()
        .await;

    // Update component three times
    deps.update_component(&component_id, "counters").await;
    deps.update_component(&component_id, "counters").await;
    deps.update_component(&component_id, "counters").await;

    // Get all versions
    let result = deps
        .component_service()
        .get_latest_component_metadata(GetLatestComponentRequest {
            component_id: Some(component_id.clone().into()),
        })
        .await
        .unwrap();

    // Check metadata version
    check!(result.versioned_component_id.unwrap().version == 3);
}

#[test]
#[tracing::instrument]
async fn get_component_metadata_all_versions(deps: &EnvBasedTestDependencies) {
    // Create component
    let (component_id, component_name) = deps
        .component("counters")
        .unique()
        .store_and_get_name()
        .await;

    // Update component a few times while changing type, ifs, dynamic link
    let files = deps
        .add_initial_component_files(
            &AccountId::placeholder(),
            &[
                (
                    "initial-file-read-write/files/foo.txt",
                    "/test-file-readonly",
                    ComponentFilePermissions::ReadOnly,
                ),
                (
                    "initial-file-read-write/files/baz.txt",
                    "/test-file-readwrite",
                    ComponentFilePermissions::ReadOnly,
                ),
            ],
        )
        .await;

    let link = (
        "dummy:dummy/dummy".to_string(),
        DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
            targets: HashMap::from_iter(vec![(
                "dummy".to_string(),
                WasmRpcTarget {
                    interface_name: "dummy:dummy/dummy-x".to_string(),
                    component_name: "dummy:dummy".to_string(),
                    component_type: ComponentType::Durable,
                },
            )]),
        }),
    );
    deps.component_service()
        .update_component(
            &component_id,
            &deps.component_directory().join("counters.wasm"),
            ComponentType::Durable,
            Some(&files),
            None,
            &HashMap::new(),
        )
        .await
        .unwrap();

    deps.component_service()
        .update_component(
            &component_id,
            &deps.component_directory().join("counters.wasm"),
            ComponentType::Ephemeral,
            None,
            None,
            &HashMap::new(),
        )
        .await
        .unwrap();

    deps.component_service()
        .update_component(
            &component_id,
            &deps.component_directory().join("counters.wasm"),
            ComponentType::Durable,
            None,
            Some(&HashMap::from([link.clone()])),
            &HashMap::new(),
        )
        .await
        .unwrap();

    // Get all versions
    let result = deps
        .component_service()
        .get_component_metadata_all_versions(GetComponentRequest {
            component_id: Some(component_id.clone().into()),
        })
        .await
        .unwrap();

    // Check metadata
    check!(result.len() == 4);

    let component_id_str = common_component_id_to_str(&component_id);
    for (idx, component) in result.iter().enumerate() {
        check!(
            component.versioned_component_id.unwrap().version == idx as u64,
            "{idx}"
        );

        check!(
            grpc_component_id_to_str(
                &component
                    .versioned_component_id
                    .unwrap()
                    .component_id
                    .unwrap()
            ) == component_id_str,
            "{idx}"
        );

        check!(component.component_name == component_name.0, "{idx}");

        match idx {
            idx if idx >= 1 => {
                assert!(component.files.len() == 2, "{idx}");

                check!(
                    InitialComponentFile::try_from(component.files[0].clone()).unwrap()
                        == files[0].1,
                    "{idx}"
                );
                check!(
                    InitialComponentFile::try_from(component.files[1].clone()).unwrap()
                        == files[1].1,
                    "{idx}"
                );
            }
            _ => {
                check!(component.files.is_empty(), "{idx}");
            }
        }

        match idx {
            2 => {
                check!(
                    component.component_type == Some(ComponentType::Ephemeral as i32),
                    "{idx}"
                );
            }
            _ => {
                check!(
                    component.component_type == Some(ComponentType::Durable as i32),
                    "{idx}"
                );
            }
        }

        match idx {
            3 => {
                let dynamic_linking = component
                    .metadata
                    .as_ref()
                    .unwrap()
                    .dynamic_linking
                    .get(&link.0)
                    .unwrap();

                check!(link.1 == DynamicLinkedInstance::try_from(dynamic_linking.clone()).unwrap());
            }
            _ => {
                check!(
                    component
                        .metadata
                        .as_ref()
                        .unwrap()
                        .dynamic_linking
                        .is_empty(),
                    "{idx}"
                );
            }
        }
    }
}

fn common_component_id_to_str(component_id: &ComponentId) -> String {
    component_id.to_string()
}

fn grpc_component_id_to_str(
    component_id: &golem_api_grpc::proto::golem::component::ComponentId,
) -> String {
    Uuid::from(component_id.value.unwrap()).to_string()
}

fn check_versioned_id(
    component: &Component,
    expected_component_id: &ComponentId,
    expected_version: u64,
) {
    check!(component.versioned_component_id.unwrap().version == expected_version);
    let returned_component_id = grpc_component_id_to_str(
        &component
            .versioned_component_id
            .unwrap()
            .component_id
            .unwrap(),
    );
    let expected_component_id = common_component_id_to_str(expected_component_id);
    check!(returned_component_id == expected_component_id);
}
