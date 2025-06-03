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

use test_r::{inherit_test_dep, test};

use crate::common::{start, TestContext};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use assert2::check;
use golem_common::model::component_metadata::{
    DynamicLinkedGrpc, DynamicLinkedInstance, GrpcMetadata, GrpcTarget,
};
use golem_common::model::ComponentType;
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_rpc::Value;
use std::collections::HashMap;
use std::path::Path;
use tracing::info;
use std::thread::sleep;
use std::time::Duration;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

// I am unary rpc
#[test]
#[tracing::instrument]
async fn grpc_unary_rpc(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let root: String = std::env::var("WORKSPACE_DIR").expect("root directory");

    let (_, package_name, fds) = wit_carpenter::from_grpc(
        &[Path::new(&format!(
            "{}/test-components/grpc-example1_app/components-rust/rpc-grpc-api-caller/proto/index.proto",
            root
        ))],
        &[Path::new(&format!(
            "{}/test-components/grpc-example1_app/components-rust/rpc-grpc-api-caller/proto",
            root
        ))],
        None,
    );

    let api_caller_component_id = executor
        .component("rpc_grpc_api_caller")
        .with_dynamic_linking(&[(
            "rpc-grpc:grpcbin-client/grpcbin-world-client",
            DynamicLinkedInstance::Grpc(DynamicLinkedGrpc {
                targets: HashMap::from_iter(vec![
                    (
                        "g-r-p-c-bin".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "dummy-server-stream-resource-server-streaming".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "g-r-p-c-bin-resource-unary".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "dummy-client-stream-resource-client-streaming".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "dummy-bi-stream-resource-bidirectional-streaming".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "grpcbin".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/grpcbin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                ]),
                metadata: GrpcMetadata {
                    file_descriptor_set: fds,
                    package_name,
                },
            }),
        )])
        .store()
        .await;

    let mut env = HashMap::new();
    env.insert("RPC_GRPC_SECRET_TOKEN".to_string(), "secret".to_string());

    env.insert(
        "RPC_GRPC_URL".to_string(),
        "http://localhost:50051".to_string(),
    );

    let api_caller_worker_id = executor
        .start_worker_with(&api_caller_component_id, "rpc_grpc_api_caller", vec![], env)
        .await;

    let _ = executor.log_output(&api_caller_worker_id).await;

    let result = executor
        .invoke_and_await(
            &api_caller_worker_id,
            "rpc-grpc:api-caller-exports/rpc-grpc-api-caller-api.{unary-rpc}",
            vec![],
        )
        .await;

    sleep(Duration::new(20, 0));
    executor
        .check_oplog_is_queryable(&api_caller_worker_id)
        .await;

    drop(executor);

    info!("result: {:?}", result);
    check!(result.is_ok());

    let value = &result.unwrap()[0];
    check!(
        *value == Value::Record(
        vec![ Value::String("Good, received messaged : DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }".to_string()), ]
        )
    );
}

#[test]
#[tracing::instrument]
async fn grpc_unary_rpc_unavailable(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let root: String = std::env::var("WORKSPACE_DIR").expect("root directory");

    let (_, package_name, fds) = wit_carpenter::from_grpc(
        &[Path::new(&format!(
            "{}/test-components/grpc-example1_app/components-rust/rpc-grpc-api-caller/proto/index.proto",
            root
        ))],
        &[Path::new(&format!(
            "{}/test-components/grpc-example1_app/components-rust/rpc-grpc-api-caller/proto",
            root
        ))],
        None,
    );

    let api_caller_component_id = executor
        .component("rpc_grpc_api_caller")
        .with_dynamic_linking(&[(
            "rpc-grpc:grpcbin-client/grpcbin-world-client",
            DynamicLinkedInstance::Grpc(DynamicLinkedGrpc {
                targets: HashMap::from_iter(vec![
                    (
                        "g-r-p-c-bin".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "dummy-server-stream-resource-server-streaming".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "g-r-p-c-bin-resource-unary".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "dummy-client-stream-resource-client-streaming".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "dummy-bi-stream-resource-bidirectional-streaming".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "grpcbin".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/grpcbin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                ]),
                metadata: GrpcMetadata {
                    file_descriptor_set: fds,
                    package_name,
                },
            }),
        )])
        .store()
        .await;

    let mut env = HashMap::new();
    env.insert("RPC_GRPC_SECRET_TOKEN".to_string(), "secret".to_string());

    env.insert(
        "RPC_GRPC_URL".to_string(),
        "http://localhost:50052".to_string(), // unavailable
    );

    let api_caller_worker_id = executor
        .start_worker_with(&api_caller_component_id, "rpc_grpc_api_caller", vec![], env)
        .await;

    let _ = executor.log_output(&api_caller_worker_id).await;

    let result = executor
        .invoke_and_await(
            &api_caller_worker_id,
            "rpc-grpc:api-caller-exports/rpc-grpc-api-caller-api.{unary-rpc}",
            vec![],
        )
        .await;

    executor
        .check_oplog_is_queryable(&api_caller_worker_id)
        .await;

    drop(executor);

    info!("result: {:?}", result);
    check!(result.is_ok());

    let value = &result.unwrap()[0];

    let flag = match value {
        Value::Record(v) => {
            if let Some(Value::String(s)) = v.get(0) {
                s.contains("Unable to establish the connection to :")
            } else {
                false
            }
        }
        _ => false,
    };

    check!(flag);
}

#[test]
#[tracing::instrument]
async fn grpc_server_streaming_rpc(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let root: String = std::env::var("WORKSPACE_DIR").expect("root directory");

    let (_, package_name, fds) = wit_carpenter::from_grpc(
        &[Path::new(&format!(
            "{}/test-components/grpc-example1_app/components-rust/rpc-grpc-api-caller/proto/index.proto",
            root
        ))],
        &[Path::new(&format!(
            "{}/test-components/grpc-example1_app/components-rust/rpc-grpc-api-caller/proto",
            root
        ))],
        None,
    );

    let api_caller_component_id = executor
        .component("rpc_grpc_api_caller")
        .with_dynamic_linking(&[(
            "rpc-grpc:grpcbin-client/grpcbin-world-client",
            DynamicLinkedInstance::Grpc(DynamicLinkedGrpc {
                targets: HashMap::from_iter(vec![
                    (
                        "g-r-p-c-bin".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "dummy-server-stream-resource-server-streaming".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "g-r-p-c-bin-resource-unary".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "dummy-client-stream-resource-client-streaming".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "dummy-bi-stream-resource-bidirectional-streaming".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "grpcbin".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/grpcbin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                ]),
                metadata: GrpcMetadata {
                    file_descriptor_set: fds,
                    package_name,
                },
            }),
        )])
        .store()
        .await;

    let mut env = HashMap::new();
    env.insert("RPC_GRPC_SECRET_TOKEN".to_string(), "secret".to_string());

    env.insert(
        "RPC_GRPC_URL".to_string(),
        "http://localhost:50051".to_string(),
    );

    let api_caller_worker_id = executor
        .start_worker_with(&api_caller_component_id, "rpc_grpc_api_caller", vec![], env)
        .await;

    let _ = executor.log_output(&api_caller_worker_id).await;

    let result = executor
        .invoke_and_await(
            &api_caller_worker_id,
            "rpc-grpc:api-caller-exports/rpc-grpc-api-caller-api.{server-streaming-rpc}",
            vec![],
        )
        .await;

    executor
        .check_oplog_is_queryable(&api_caller_worker_id)
        .await;

    drop(executor);

    info!("result: {:?}", result);
    check!(result.is_ok());

    let value = &result.unwrap()[0];
    check!(
        *value == Value::Record(
        vec![ Value::String("Good, received 10 messages : [DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }]".to_string()), ]
        )
    );
}

#[test]
#[tracing::instrument]
async fn grpc_client_streaming_rpc(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let root: String = std::env::var("WORKSPACE_DIR").expect("root directory");

    let (_, package_name, fds) = wit_carpenter::from_grpc(
        &[Path::new(&format!(
            "{}/test-components/grpc-example1_app/components-rust/rpc-grpc-api-caller/proto/index.proto",
            root
        ))],
        &[Path::new(&format!(
            "{}/test-components/grpc-example1_app/components-rust/rpc-grpc-api-caller/proto",
            root
        ))],
        None,
    );

    let api_caller_component_id = executor
        .component("rpc_grpc_api_caller")
        .with_dynamic_linking(&[(
            "rpc-grpc:grpcbin-client/grpcbin-world-client",
            DynamicLinkedInstance::Grpc(DynamicLinkedGrpc {
                targets: HashMap::from_iter(vec![
                    (
                        "g-r-p-c-bin".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "dummy-server-stream-resource-server-streaming".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "g-r-p-c-bin-resource-unary".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "dummy-client-stream-resource-client-streaming".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "dummy-bi-stream-resource-bidirectional-streaming".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "grpcbin".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/grpcbin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                ]),
                metadata: GrpcMetadata {
                    file_descriptor_set: fds,
                    package_name,
                },
            }),
        )])
        .store()
        .await;

    let mut env = HashMap::new();
    env.insert("RPC_GRPC_SECRET_TOKEN".to_string(), "secret".to_string());

    env.insert(
        "RPC_GRPC_URL".to_string(),
        "http://localhost:50051".to_string(),
    );

    let api_caller_worker_id = executor
        .start_worker_with(&api_caller_component_id, "rpc_grpc_api_caller", vec![], env)
        .await;

    let _ = executor.log_output(&api_caller_worker_id).await;

    let result = executor
        .invoke_and_await(
            &api_caller_worker_id,
            "rpc-grpc:api-caller-exports/rpc-grpc-api-caller-api.{client-streaming-rpc}",
            vec![],
        )
        .await;

    executor
        .check_oplog_is_queryable(&api_caller_worker_id)
        .await;

    drop(executor);

    info!("result: {:?}", result);
    check!(result.is_ok());

    let value = &result.unwrap()[0];
    check!(
        *value
            == Value::Record(vec![Value::String(
                "Good, client streaming was demostrated well enough".to_string()
            )])
    );
}

#[test]
#[tracing::instrument]
async fn grpc_bidirectional_streaming_rpc(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let root: String = std::env::var("WORKSPACE_DIR").expect("root directory");

    let (_, package_name, fds) = wit_carpenter::from_grpc(
        &[Path::new(&format!(
            "{}/test-components/grpc-example1_app/components-rust/rpc-grpc-api-caller/proto/index.proto",
            root
        ))],
        &[Path::new(&format!(
            "{}/test-components/grpc-example1_app/components-rust/rpc-grpc-api-caller/proto",
            root
        ))],
        None,
    );

    let api_caller_component_id = executor
        .component("rpc_grpc_api_caller")
        .with_dynamic_linking(&[(
            "rpc-grpc:grpcbin-client/grpcbin-world-client",
            DynamicLinkedInstance::Grpc(DynamicLinkedGrpc {
                targets: HashMap::from_iter(vec![
                    (
                        "g-r-p-c-bin".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "dummy-server-stream-resource-server-streaming".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "g-r-p-c-bin-resource-unary".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "dummy-client-stream-resource-client-streaming".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "dummy-bi-stream-resource-bidirectional-streaming".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "grpcbin".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/grpcbin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                ]),
                metadata: GrpcMetadata {
                    file_descriptor_set: fds,
                    package_name,
                },
            }),
        )])
        .store()
        .await;

    let mut env = HashMap::new();
    env.insert("RPC_GRPC_SECRET_TOKEN".to_string(), "secret".to_string());

    env.insert(
        "RPC_GRPC_URL".to_string(),
        "http://localhost:50051".to_string(),
    );

    let api_caller_worker_id = executor
        .start_worker_with(&api_caller_component_id, "rpc_grpc_api_caller", vec![], env)
        .await;

    let _ = executor.log_output(&api_caller_worker_id).await;

    let result = executor
        .invoke_and_await(
            &api_caller_worker_id,
            "rpc-grpc:api-caller-exports/rpc-grpc-api-caller-api.{bidirectional-streaming-rpc}",
            vec![],
        )
        .await;

    executor
        .check_oplog_is_queryable(&api_caller_worker_id)
        .await;

    drop(executor);

    info!("result: {:?}", result);
    check!(result.is_ok());

    let value = &result.unwrap()[0];
    check!(
        *value
            == Value::Record(vec![Value::String(
                "Good, bi directional streaming was demostrated well enough".to_string()
            ),])
    );
}

#[test]
#[tracing::instrument]
async fn durabile_grpc_unary_rpc(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);

    let executor = start(deps, &context).await.unwrap();

    let root: String = std::env::var("WORKSPACE_DIR").expect("root directory");

    let (_, package_name, fds) = wit_carpenter::from_grpc(
        &[Path::new(&format!(
            "{}/test-components/grpc-example1_app/components-rust/rpc-grpc-api-caller/proto/index.proto",
            root
        ))],
        &[Path::new(&format!(
            "{}/test-components/grpc-example1_app/components-rust/rpc-grpc-api-caller/proto",
            root
        ))],
        None,
    );

    let api_caller_component_id = executor
        .component("rpc_grpc_api_caller")
        .with_dynamic_linking(&[(
            "rpc-grpc:grpcbin-client/grpcbin-world-client",
            DynamicLinkedInstance::Grpc(DynamicLinkedGrpc {
                targets: HashMap::from_iter(vec![
                    (
                        "g-r-p-c-bin".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "dummy-server-stream-resource-server-streaming".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "g-r-p-c-bin-resource-unary".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "dummy-client-stream-resource-client-streaming".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "dummy-bi-stream-resource-bidirectional-streaming".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                    (
                        "grpcbin".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/grpcbin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Durable,
                        },
                    ),
                ]),
                metadata: GrpcMetadata {
                    file_descriptor_set: fds,
                    package_name,
                },
            }),
        )])
        .store()
        .await;

    let mut env = HashMap::new();
    env.insert("RPC_GRPC_SECRET_TOKEN".to_string(), "secret".to_string());

    env.insert(
        "RPC_GRPC_URL".to_string(),
        "http://localhost:50051".to_string(),
    );

    let api_caller_worker_id = executor
        .start_worker_with(&api_caller_component_id, "rpc_grpc_api_caller", vec![], env)
        .await;

    let _ = executor.log_output(&api_caller_worker_id).await;

    let result = executor
        .invoke_and_await(
            &api_caller_worker_id,
            "rpc-grpc:api-caller-exports/rpc-grpc-api-caller-api.{unary-rpc}",
            vec![],
        )
        .await;

    executor
        .check_oplog_is_queryable(&api_caller_worker_id)
        .await;

    drop(executor);

    info!("result: {:?}", result);
    check!(result.is_ok());

    let value = &result.unwrap()[0];
    check!(
        *value == Value::Record(
        vec![ Value::String("Good, received 10 messages : [DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }]".to_string()), ]
        )
    );
}
