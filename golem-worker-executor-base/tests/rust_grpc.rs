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
use std::thread::sleep;
use std::time::Duration;
use tracing::info;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn grpc_unary_rpc(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    sleep(Duration::new(20, 0));
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let (_, package_name, fds) = wit_carpenter::from_grpc(
        Path::new("/home/balaram/golem-fork/test-components/rpc-grpc-proto/components-rust/rpc-grpc-api-caller/proto"),
        None
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
                            component_type: ComponentType::Ephemeral,
                        },
                    ),
                    (
                        "dummy-server-stream-resource-server-streaming".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/g-r-p-c-bin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Ephemeral,
                        },
                    ),
                    (
                        "grpcbin".to_string(),
                        GrpcTarget {
                            interface_name: "rpc-grpc:grpcbin/grpcbin".to_string(),
                            component_name: "rpc-grpc:grpcbin".to_string(),
                            component_type: ComponentType::Ephemeral,
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

    let api_caller_worker_id = executor
        .start_worker_with(
            &api_caller_component_id,
            "rpc_grpc_api_caller",
            vec![],
            HashMap::new(),
        )
        .await;

    let _ = executor.log_output(&api_caller_worker_id).await;

    let index_result = executor
        .invoke_and_await(
            &api_caller_worker_id,
            "rpc-grpc:api-caller-exports/rpc-grpc-api-caller-api.{hello}",
            vec![],
        )
        .await;

    drop(executor);

    info!("result: {:?}", index_result);
    check!(index_result.is_ok());

    let value = &index_result.unwrap()[0];
    check!(
        *value == Value::Record(
        vec![ Value::String("Good, received 10 messages : [DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }, DummyMessage { f-string: Some(\"Hello, World!\"), f-strings: [\"one\", \"two\"], f-int32: Some(42), f-int32s: [1, 2, 3], f-enum: Some(DummyMessageEnum::Enum1), f-enums: [DummyMessageEnum::Enum1, DummyMessageEnum::Enum1], f-sub: Some(DummyMessageSub { f-string: Some(\"Hello, World!\") }), f-subs: [DummyMessageSub { f-string: Some(\"Hello, World!\") }, DummyMessageSub { f-string: Some(\"Hello, World!\") }], f-bool: Some(true), f-bools: [true, false, true], f-int64: Some(9876543210), f-int64s: [100, 200, 300], f-bytes: Some([222, 173, 190, 239]), f-bytess: [[170, 187], [204, 221]], f-float: Some(3.14), f-floats: [1.1, 2.2, 3.3] }]".to_string()), ]
        )
    );
}
