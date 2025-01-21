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

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::AtomicU8;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::common::{start, TestContext};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use assert2::{assert, check};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{BoxError, Router};
use bytes::Bytes;
use futures_util::stream;
use golem_common::model::{
    AccountId, ComponentFilePath, ComponentFilePermissions, ComponentFileSystemNode,
    ComponentFileSystemNodeDetails, ComponentType, IdempotencyKey, InitialComponentFile,
    WorkerStatus,
};
use golem_test_framework::dsl::{
    drain_connection, stderr_events, stdout_events, worker_error_message, TestDslUnsafe,
};
use golem_wasm_rpc::Value;
use http::{HeaderMap, StatusCode};
use tokio::spawn;
use tokio::time::Instant;
use tokio_stream::StreamExt;
use tracing::info;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn write_stdout(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("write-stdout").await;
    let worker_id = executor.start_worker(&component_id, "write-stdout-1").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let _result = executor.invoke_and_await(&worker_id, "run", vec![]).await;

    let mut events = vec![];
    let start_time = Instant::now();
    while events.len() < 2 && start_time.elapsed() < Duration::from_secs(5) {
        if let Some(event) = rx.recv().await {
            events.push(event);
        } else {
            break;
        }
    }

    drop(executor);

    check!(stdout_events(events.into_iter()) == vec!["Sample text written to the output\n"]);
}

#[test]
#[tracing::instrument]
async fn write_stderr(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("write-stderr").await;
    let worker_id = executor.start_worker(&component_id, "write-stderr-1").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let _result = executor.invoke_and_await(&worker_id, "run", vec![]).await;

    let mut events = vec![];
    let start_time = Instant::now();
    while events.len() < 2 && start_time.elapsed() < Duration::from_secs(5) {
        if let Some(event) = rx.recv().await {
            events.push(event);
        } else {
            break;
        }
    }

    drop(executor);

    check!(stderr_events(events.into_iter()) == vec!["Sample text written to the error output\n"]);
}

#[test]
#[tracing::instrument]
async fn read_stdin(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("read-stdin").await;
    let worker_id = executor.start_worker(&component_id, "read-stdin-1").await;

    let result = executor.invoke_and_await(&worker_id, "run", vec![]).await;

    drop(executor);

    assert!(result.is_err()); // stdin is disabled
}

#[test]
#[tracing::instrument]
async fn clocks(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("clocks").await;
    let worker_id = executor.start_worker(&component_id, "clocks-1").await;

    let result = executor
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(result.len() == 1);
    let Value::Tuple(tuple) = &result[0] else {
        panic!("expected tuple")
    };
    check!(tuple.len() == 3);

    let Value::F64(elapsed1) = &tuple[0] else {
        panic!("expected f64")
    };
    let Value::F64(elapsed2) = &tuple[1] else {
        panic!("expected f64")
    };
    let Value::String(odt) = &tuple[2] else {
        panic!("expected string")
    };

    let epoch_seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let diff1 = (epoch_seconds - *elapsed1).abs();
    let parsed_odt = chrono::DateTime::parse_from_rfc3339(odt.as_str()).unwrap();
    let odt_diff = epoch_seconds - parsed_odt.timestamp() as f64;

    check!(diff1 < 5.0);
    check!(*elapsed2 >= 2.0);
    check!(*elapsed2 < 3.0);
    check!(odt_diff < 5.0);
}

#[test]
#[tracing::instrument]
async fn file_write_read_delete(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("file-write-read-delete").await;
    let mut env = HashMap::new();
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());
    let worker_id = executor
        .start_worker_with(&component_id, "file-write-read-delete-1", vec![], env)
        .await;

    let result = executor
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(
        result
            == vec![Value::Tuple(vec![
                Value::Option(None),
                Value::Option(Some(Box::new(Value::String("hello world".to_string())))),
                Value::Option(None)
            ])]
    );
}

#[test]
#[tracing::instrument]
async fn initial_file_read_write(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let account_id = AccountId {
        value: "test-account".to_string(),
    };

    let file1_key = executor
        .add_initial_component_file(
            &account_id,
            PathBuf::from("initial-file-read-write/files/foo.txt").as_path(),
        )
        .await;
    let file2_key = executor
        .add_initial_component_file(
            &account_id,
            PathBuf::from("initial-file-read-write/files/baz.txt").as_path(),
        )
        .await;

    let component_files: Vec<InitialComponentFile> = vec![
        InitialComponentFile {
            key: file1_key,
            path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
            permissions: ComponentFilePermissions::ReadOnly,
        },
        InitialComponentFile {
            key: file2_key,
            path: ComponentFilePath::from_abs_str("/bar/baz.txt").unwrap(),
            permissions: ComponentFilePermissions::ReadWrite,
        },
    ];

    let component_id = executor
        .store_unique_component_with_files(
            "initial-file-read-write",
            ComponentType::Durable,
            &component_files,
        )
        .await;
    let mut env = HashMap::new();
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());
    let worker_id = executor
        .start_worker_with(&component_id, "initial-file-read-write-1", vec![], env)
        .await;

    let result = executor
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(
        result
            == vec![Value::Tuple(vec![
                Value::Option(Some(Box::new(Value::String("foo\n".to_string())))),
                Value::Option(None),
                Value::Option(None),
                Value::Option(Some(Box::new(Value::String("baz\n".to_string())))),
                Value::Option(Some(Box::new(Value::String("hello world".to_string())))),
            ])]
    );
}

#[test]
#[tracing::instrument]
async fn initial_file_listing_through_api(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let account_id = AccountId {
        value: "test-account".to_string(),
    };

    let file1_key = executor
        .add_initial_component_file(
            &account_id,
            PathBuf::from("initial-file-read-write/files/foo.txt").as_path(),
        )
        .await;
    let file2_key = executor
        .add_initial_component_file(
            &account_id,
            PathBuf::from("initial-file-read-write/files/baz.txt").as_path(),
        )
        .await;

    let component_files: Vec<InitialComponentFile> = vec![
        InitialComponentFile {
            key: file1_key,
            path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
            permissions: ComponentFilePermissions::ReadOnly,
        },
        InitialComponentFile {
            key: file2_key.clone(),
            path: ComponentFilePath::from_abs_str("/bar/baz.txt").unwrap(),
            permissions: ComponentFilePermissions::ReadWrite,
        },
        InitialComponentFile {
            key: file2_key,
            path: ComponentFilePath::from_abs_str("/baz.txt").unwrap(),
            permissions: ComponentFilePermissions::ReadWrite,
        },
    ];

    let component_id = executor
        .store_unique_component_with_files(
            "initial-file-read-write",
            ComponentType::Durable,
            &component_files,
        )
        .await;
    let mut env = HashMap::new();
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());
    let worker_id = executor
        .start_worker_with(&component_id, "initial-file-read-write-2", vec![], env)
        .await;

    let result = executor.list_directory(&worker_id, "/").await;

    let mut result = result
        .into_iter()
        .map(|e| ComponentFileSystemNode {
            last_modified: SystemTime::UNIX_EPOCH,
            ..e
        })
        .collect::<Vec<_>>();

    result.sort_by_key(|e| e.name.clone());

    drop(executor);

    check!(
        result
            == vec![
                ComponentFileSystemNode {
                    name: "bar".to_string(),
                    last_modified: SystemTime::UNIX_EPOCH,
                    details: ComponentFileSystemNodeDetails::Directory
                },
                ComponentFileSystemNode {
                    name: "baz.txt".to_string(),
                    last_modified: SystemTime::UNIX_EPOCH,
                    details: ComponentFileSystemNodeDetails::File {
                        permissions: ComponentFilePermissions::ReadWrite,
                        size: 4,
                    }
                },
                ComponentFileSystemNode {
                    name: "foo.txt".to_string(),
                    last_modified: SystemTime::UNIX_EPOCH,
                    details: ComponentFileSystemNodeDetails::File {
                        permissions: ComponentFilePermissions::ReadOnly,
                        size: 4,
                    }
                },
            ]
    );
}

#[test]
#[tracing::instrument]
async fn initial_file_reading_through_api(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let account_id = AccountId {
        value: "test-account".to_string(),
    };

    let file1_key = executor
        .add_initial_component_file(
            &account_id,
            PathBuf::from("initial-file-read-write/files/foo.txt").as_path(),
        )
        .await;
    let file2_key = executor
        .add_initial_component_file(
            &account_id,
            PathBuf::from("initial-file-read-write/files/baz.txt").as_path(),
        )
        .await;

    let component_files: Vec<InitialComponentFile> = vec![
        InitialComponentFile {
            key: file1_key,
            path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
            permissions: ComponentFilePermissions::ReadOnly,
        },
        InitialComponentFile {
            key: file2_key.clone(),
            path: ComponentFilePath::from_abs_str("/bar/baz.txt").unwrap(),
            permissions: ComponentFilePermissions::ReadWrite,
        },
    ];

    let component_id = executor
        .store_unique_component_with_files(
            "initial-file-read-write",
            ComponentType::Durable,
            &component_files,
        )
        .await;
    let mut env = HashMap::new();
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());
    let worker_id = executor
        .start_worker_with(&component_id, "initial-file-read-write-3", vec![], env)
        .await;

    // run the worker so it can update the files.
    executor
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    let result1 = executor.get_file_contents(&worker_id, "/foo.txt").await;
    let result1 = std::str::from_utf8(&result1).unwrap();

    let result2 = executor.get_file_contents(&worker_id, "/bar/baz.txt").await;
    let result2 = std::str::from_utf8(&result2).unwrap();

    drop(executor);

    check!(result1 == "foo\n");
    check!(result2 == "hello world");
}

#[test]
#[tracing::instrument]
async fn directories(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("directories").await;
    let worker_id = executor.start_worker(&component_id, "directories-1").await;

    let result = executor
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    drop(executor);

    let Value::Tuple(tuple) = &result[0] else {
        panic!("expected tuple")
    };
    check!(tuple.len() == 4); //  tuple<u32, list<tuple<string, bool>>, list<tuple<string, bool>>, u32>;

    check!(tuple[0] == Value::U32(0)); // initial number of entries
    check!(
        tuple[1]
            == Value::List(vec![Value::Tuple(vec![
                Value::String("/test".to_string()),
                Value::Bool(true)
            ])])
    ); // contents of /

    // contents of /test
    let Value::List(list) = &tuple[2] else {
        panic!("expected list")
    };
    check!(
        *list
            == vec![
                Value::Tuple(vec![
                    Value::String("/test/dir1".to_string()),
                    Value::Bool(true)
                ]),
                Value::Tuple(vec![
                    Value::String("/test/dir2".to_string()),
                    Value::Bool(true)
                ]),
                Value::Tuple(vec![
                    Value::String("/test/hello.txt".to_string()),
                    Value::Bool(false)
                ]),
            ]
    );
    check!(tuple[3] == Value::U32(1)); // final number of entries NOTE: this should be 0 if remove_directory worked
}

#[test]
#[tracing::instrument]
async fn directories_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("directories").await;
    let worker_id = executor.start_worker(&component_id, "directories-1").await;

    let result = executor
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    drop(executor);
    let executor = start(deps, &context).await.unwrap();

    // NOTE: if the directory listing would not be stable, replay would fail with divergence error

    let metadata = executor
        .wait_for_status(&worker_id, WorkerStatus::Idle, Duration::from_secs(5))
        .await;

    check!(metadata.last_known_status.status == WorkerStatus::Idle);

    let Value::Tuple(tuple) = &result[0] else {
        panic!("expected tuple")
    };
    check!(tuple.len() == 4); //  tuple<u32, list<tuple<string, bool>>, list<tuple<string, bool>>, u32>;

    check!(tuple[0] == Value::U32(0)); // initial number of entries
    check!(
        tuple[1]
            == Value::List(vec![Value::Tuple(vec![
                Value::String("/test".to_string()),
                Value::Bool(true)
            ])])
    ); // contents of /

    // contents of /test
    let Value::List(list) = &tuple[2] else {
        panic!("expected list")
    };
    check!(
        *list
            == vec![
                Value::Tuple(vec![
                    Value::String("/test/dir1".to_string()),
                    Value::Bool(true)
                ]),
                Value::Tuple(vec![
                    Value::String("/test/dir2".to_string()),
                    Value::Bool(true)
                ]),
                Value::Tuple(vec![
                    Value::String("/test/hello.txt".to_string()),
                    Value::Bool(false)
                ]),
            ]
    );
    check!(tuple[3] == Value::U32(1)); // final number of entries NOTE: this should be 0 if remove_directory worked
}

#[test]
#[tracing::instrument]
async fn file_write_read(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("file-service").await;
    let worker_id = executor.start_worker(&component_id, "file-service-1").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{write-file}",
            vec![
                Value::String("/testfile.txt".to_string()),
                Value::String("hello world".to_string()),
            ],
        )
        .await
        .unwrap();

    drop(executor);
    let executor = start(deps, &context).await.unwrap();

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{read-file}",
            vec![Value::String("/testfile.txt".to_string())],
        )
        .await
        .unwrap();

    check!(
        result
            == vec![Value::Result(Ok(Some(Box::new(Value::String(
                "hello world".to_string()
            )))))]
    );
}

#[test]
#[tracing::instrument]
async fn http_client(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let host_http_port = context.host_http_port();
    let http_server = spawn(async move {
        let route = Router::new().route(
            "/",
            post(move |headers: HeaderMap, body: Bytes| async move {
                let header = headers.get("X-Test").unwrap().to_str().unwrap();
                let body = String::from_utf8(body.to_vec()).unwrap();
                format!("response is {header} {body}")
            }),
        );

        let listener = tokio::net::TcpListener::bind(
            format!("0.0.0.0:{}", host_http_port)
                .parse::<SocketAddr>()
                .unwrap(),
        )
        .await
        .unwrap();
        axum::serve(listener, route).await.unwrap();
    });

    let component_id = executor.store_component("http-client").await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "http-client-1", vec![], env)
        .await;
    let rx = executor.capture_output(&worker_id).await;

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{run}", vec![])
        .await;

    drop(executor);
    drop(rx);
    http_server.abort();

    check!(
        result
            == Ok(vec![Value::String(
                "200 response is test-header test-body".to_string()
            )])
    );
}

#[test]
#[tracing::instrument]
async fn http_client_using_reqwest(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();
    let captured_body: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let captured_body_clone = captured_body.clone();
    let host_http_port = context.host_http_port();
    let http_server = spawn(async move {
        let route = Router::new().route(
            "/post-example",
            post(move |headers: HeaderMap, body: Bytes| async move {
                let header = headers
                    .get("X-Test")
                    .map(|h| h.to_str().unwrap().to_string())
                    .unwrap_or("no X-Test header".to_string());
                let body = String::from_utf8(body.to_vec()).unwrap();
                {
                    let mut capture = captured_body_clone.lock().unwrap();
                    *capture = Some(body.clone());
                }
                format!(
                    "{{ \"percentage\" : 0.25, \"message\": \"response message {}\" }}",
                    header
                )
            }),
        );

        let listener = tokio::net::TcpListener::bind(
            format!("0.0.0.0:{}", host_http_port)
                .parse::<SocketAddr>()
                .unwrap(),
        )
        .await
        .unwrap();
        axum::serve(listener, route).await.unwrap();
    });

    let component_id = executor.store_component("http-client-2").await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "http-client-reqwest-1", vec![], env)
        .await;

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{run}", vec![])
        .await
        .unwrap();
    let captured_body = captured_body.lock().unwrap().clone().unwrap();

    drop(executor);
    http_server.abort();

    check!(result == vec![Value::String("200 ExampleResponse { percentage: 0.25, message: Some(\"response message Golem\") }".to_string())]);
    check!(
        captured_body
            == "{\"name\":\"Something\",\"amount\":42,\"comments\":[\"Hello\",\"World\"]}"
                .to_string()
    );
}

#[test]
#[tracing::instrument]
async fn environment_service(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("environment-service").await;
    let args = vec!["test-arg".to_string()];
    let mut env = HashMap::new();
    env.insert("TEST_ENV".to_string(), "test-value".to_string());
    let worker_id = executor
        .start_worker_with(&component_id, "environment-service-1", args, env)
        .await;

    let args_result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-arguments}", vec![])
        .await
        .unwrap();

    let env_result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-environment}", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(
        args_result
            == vec![Value::Result(Ok(Some(Box::new(Value::List(vec![
                Value::String("test-arg".to_string())
            ])))))]
    );
    check!(
        env_result
            == vec![Value::Result(Ok(Some(Box::new(Value::List(vec![
                Value::Tuple(vec![
                    Value::String("TEST_ENV".to_string()),
                    Value::String("test-value".to_string())
                ]),
                Value::Tuple(vec![
                    Value::String("GOLEM_WORKER_NAME".to_string()),
                    Value::String("environment-service-1".to_string())
                ]),
                Value::Tuple(vec![
                    Value::String("GOLEM_COMPONENT_ID".to_string()),
                    Value::String(component_id.to_string())
                ]),
                Value::Tuple(vec![
                    Value::String("GOLEM_COMPONENT_VERSION".to_string()),
                    Value::String("0".to_string())
                ]),
            ])))))]
    );
}

#[test]
#[tracing::instrument]
async fn http_client_response_persisted_between_invocations(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();
    let host_http_port = context.host_http_port();

    let http_server = spawn(async move {
        let call_count = Arc::new(AtomicU8::new(0));

        let route = Router::new().route(
            "/",
            post(move |headers: HeaderMap, body: Bytes| async move {
                let header = headers.get("X-Test").unwrap().to_str().unwrap();
                let body = String::from_utf8(body.to_vec()).unwrap();
                let old_count = call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                match old_count {
                    0 => (StatusCode::OK, format!("response is {header} {body}")),
                    _ => (StatusCode::NOT_FOUND, "".to_string()),
                }
            }),
        );

        let listener = tokio::net::TcpListener::bind(
            format!("0.0.0.0:{}", host_http_port)
                .parse::<SocketAddr>()
                .unwrap(),
        )
        .await
        .unwrap();
        axum::serve(listener, route).await.unwrap();
    });

    let component_id = executor.store_component("http-client").await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "http-client-2", vec![], env)
        .await;
    let rx = executor.capture_output(&worker_id).await;

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api.{send-request}", vec![])
        .await
        .expect("first send-request failed");

    drop(executor);
    drop(rx);

    let executor = start(deps, &context).await.unwrap();
    let _rx = executor.capture_output(&worker_id).await;

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{process-response}", vec![])
        .await;

    http_server.abort();

    check!(
        result
            == Ok(vec![Value::String(
                "200 response is test-header test-body".to_string()
            )])
    );
}

#[test]
#[tracing::instrument]
async fn http_client_interrupting_response_stream(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();
    let host_http_port = context.host_http_port();

    let (signal_tx, mut signal_rx) = tokio::sync::mpsc::unbounded_channel();

    let http_server = spawn(async move {
        let route = Router::new().route(
            "/big-byte-array",
            get(move || async move {
                let stream = stream::iter(0..100)
                    .throttle(Duration::from_millis(20))
                    .map(move |i| {
                        if i == 50 {
                            signal_tx.send(()).unwrap();
                        }
                        Ok::<Bytes, BoxError>(Bytes::from(vec![0; 1024]))
                    });

                Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/octet-stream")
                    .body(axum::body::Body::from_stream(stream))
                    .unwrap()
            }),
        );

        let listener = tokio::net::TcpListener::bind(
            format!("0.0.0.0:{}", host_http_port)
                .parse::<SocketAddr>()
                .unwrap(),
        )
        .await
        .unwrap();
        axum::serve(listener, route).await.unwrap();
    });

    let component_id = executor.store_component("http-client-2").await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "http-client-2", vec![], env)
        .await;
    let rx = executor.capture_output_with_termination(&worker_id).await;

    let key = IdempotencyKey::fresh();

    let executor_clone = executor.clone();
    let worker_id_clone = worker_id.clone();
    let key_clone = key.clone();
    let _handle = spawn(async move {
        let _ = executor_clone
            .invoke_and_await_with_key(
                &worker_id_clone,
                &key_clone,
                "golem:it/api.{slow-body-stream}",
                vec![],
            )
            .await;
    });

    signal_rx.recv().await.unwrap();

    executor.interrupt(&worker_id).await; // Potential "body stream was interrupted" error

    let _ = drain_connection(rx).await;

    executor.resume(&worker_id, false).await;

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(5))
        .await;
    executor.log_output(&worker_id).await;

    let result = executor
        .invoke_and_await_with_key(&worker_id, &key, "golem:it/api.{slow-body-stream}", vec![])
        .await;

    drop(executor);

    http_server.abort();

    check!(result == Ok(vec![Value::U64(100 * 1024)]));
}

#[test]
#[tracing::instrument]
async fn sleep(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("clock-service").await;
    let worker_id = executor
        .start_worker(&component_id, "clock-service-1")
        .await;

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api.{sleep}", vec![Value::U64(10)])
        .await
        .unwrap();

    drop(executor);
    let executor = start(deps, &context).await.unwrap();

    let start = Instant::now();
    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api.{sleep}", vec![Value::U64(0)])
        .await
        .unwrap();
    let duration = start.elapsed();

    check!(duration.as_secs() < 2);
}

#[test]
#[tracing::instrument]
async fn resuming_sleep(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("clock-service").await;
    let worker_id = executor
        .start_worker(&component_id, "clock-service-2")
        .await;

    let executor_clone = executor.clone();
    let worker_id_clone = worker_id.clone();
    let fiber = spawn(async move {
        executor_clone
            .invoke_and_await(
                &worker_id_clone,
                "golem:it/api.{sleep}",
                vec![Value::U64(10)],
            )
            .await
            .unwrap();
    });

    tokio::time::sleep(Duration::from_secs(5)).await;

    drop(executor);
    let _ = fiber.await;

    info!("Restarting worker...");

    let executor = start(deps, &context).await.unwrap();

    info!("Worker restarted");

    let start = Instant::now();
    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api.{sleep}", vec![Value::U64(10)])
        .await
        .unwrap();
    let duration = start.elapsed();

    check!(duration.as_secs() < 20);
    check!(duration.as_secs() >= 10);
}

#[test]
#[tracing::instrument]
async fn failing_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("failing-component").await;
    let worker_id = executor
        .start_worker(&component_id, "failing-worker-1")
        .await;

    let result1 = executor
        .invoke_and_await(&worker_id, "golem:component/api.{add}", vec![Value::U64(5)])
        .await;

    let result2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:component/api.{add}",
            vec![Value::U64(50)],
        )
        .await;

    let result3 = executor
        .invoke_and_await(&worker_id, "golem:component/api.{get}", vec![])
        .await;

    drop(executor);

    check!(result1.is_ok());
    check!(result2.is_err());
    check!(result3.is_err());
    check!(worker_error_message(&result2.clone().err().unwrap())
        .starts_with("Runtime error: error while executing at wasm backtrace:"));
    check!(worker_error_message(&result2.err().unwrap())
        .contains("failing_component.wasm!golem:component/api#add"));
    check!(worker_error_message(&result3.err().unwrap()).starts_with("Previous invocation failed"));
}

#[test]
#[tracing::instrument]
async fn file_service_write_direct(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("file-service").await;
    let worker_id = executor.start_worker(&component_id, "file-service-2").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{write-file-direct}",
            vec![
                Value::String("testfile.txt".to_string()),
                Value::String("hello world".to_string()),
            ],
        )
        .await
        .unwrap();

    drop(executor);
    let executor = start(deps, &context).await.unwrap();

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{read-file}",
            vec![Value::String("/testfile.txt".to_string())],
        )
        .await
        .unwrap();

    check!(
        result
            == vec![Value::Result(Ok(Some(Box::new(Value::String(
                "hello world".to_string()
            )))))]
    );
}

#[test]
#[tracing::instrument]
async fn filesystem_write_replay_restores_file_times(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("file-service").await;
    let worker_id = executor.start_worker(&component_id, "file-service-3").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{write-file-direct}",
            vec![
                Value::String("testfile.txt".to_string()),
                Value::String("hello world".to_string()),
            ],
        )
        .await
        .unwrap();
    let times1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-file-info}",
            vec![Value::String("/testfile.txt".to_string())],
        )
        .await
        .unwrap();

    drop(executor);
    let executor = start(deps, &context).await.unwrap();

    let times2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-file-info}",
            vec![Value::String("/testfile.txt".to_string())],
        )
        .await
        .unwrap();

    check!(times1 == times2);
}

#[test]
#[tracing::instrument]
async fn filesystem_create_dir_replay_restores_file_times(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("file-service").await;
    let worker_id = executor.start_worker(&component_id, "file-service-4").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-directory}",
            vec![Value::String("/test".to_string())],
        )
        .await
        .unwrap();
    let times1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec![Value::String("/".to_string())],
        )
        .await
        .unwrap();

    drop(executor);
    let executor = start(deps, &context).await.unwrap();

    let times2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec![Value::String("/".to_string())],
        )
        .await
        .unwrap();

    check!(times1 == times2);
}

#[test]
#[tracing::instrument]
async fn file_hard_link(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("file-service").await;
    let worker_id = executor.start_worker(&component_id, "file-service-5").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{write-file}",
            vec![
                Value::String("/testfile.txt".to_string()),
                Value::String("hello world".to_string()),
            ],
        )
        .await
        .unwrap();

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-link}",
            vec![
                Value::String("/testfile.txt".to_string()),
                Value::String("/link.txt".to_string()),
            ],
        )
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{read-file}",
            vec![Value::String("/link.txt".to_string())],
        )
        .await
        .unwrap();

    check!(
        result
            == vec![Value::Result(Ok(Some(Box::new(Value::String(
                "hello world".to_string()
            )))))]
    );
}

#[test]
#[tracing::instrument]
async fn filesystem_link_replay_restores_file_times(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("file-service").await;
    let worker_id = executor.start_worker(&component_id, "file-service-6").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-directory}",
            vec![Value::String("/test".to_string())],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-directory}",
            vec![Value::String("/test2".to_string())],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{write-file}",
            vec![
                Value::String("/test/testfile.txt".to_string()),
                Value::String("hello world".to_string()),
            ],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-link}",
            vec![
                Value::String("/test/testfile.txt".to_string()),
                Value::String("/test2/link.txt".to_string()),
            ],
        )
        .await
        .unwrap();

    let times_file_1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec![Value::String("/test2/link.txt".to_string())],
        )
        .await
        .unwrap();
    let times_dir_1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec![Value::String("/test2".to_string())],
        )
        .await
        .unwrap();

    drop(executor);
    let executor = start(deps, &context).await.unwrap();

    let times_dir_2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec![Value::String("/test2".to_string())],
        )
        .await
        .unwrap();
    let times_file_2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec![Value::String("/test2/link.txt".to_string())],
        )
        .await
        .unwrap();

    check!(times_dir_1 == times_dir_2);
    check!(times_file_1 == times_file_2);
}

#[test]
#[tracing::instrument]
async fn filesystem_remove_dir_replay_restores_file_times(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("file-service").await;
    let worker_id = executor.start_worker(&component_id, "file-service-7").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-directory}",
            vec![Value::String("/test".to_string())],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-directory}",
            vec![Value::String("/test/a".to_string())],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{remove-directory}",
            vec![Value::String("/test/a".to_string())],
        )
        .await
        .unwrap();
    let times1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec![Value::String("/test".to_string())],
        )
        .await
        .unwrap();

    drop(executor);
    let executor = start(deps, &context).await.unwrap();

    let times2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec![Value::String("/test".to_string())],
        )
        .await
        .unwrap();

    check!(times1 == times2);
}

#[test]
#[tracing::instrument]
async fn filesystem_symlink_replay_restores_file_times(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("file-service").await;
    let worker_id = executor.start_worker(&component_id, "file-service-8").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-directory}",
            vec![Value::String("/test".to_string())],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-directory}",
            vec![Value::String("/test2".to_string())],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{write-file-direct}",
            vec![
                Value::String("test/testfile.txt".to_string()),
                Value::String("hello world".to_string()),
            ],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-sym-link}",
            vec![
                Value::String("../test/testfile.txt".to_string()),
                Value::String("/test2/link.txt".to_string()),
            ],
        )
        .await
        .unwrap();

    let times_file_1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec![Value::String("/test2/link.txt".to_string())],
        )
        .await
        .unwrap();
    let times_dir_1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec![Value::String("/test2".to_string())],
        )
        .await
        .unwrap();

    drop(executor);

    let executor = start(deps, &context).await.unwrap();

    let times_dir_2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec![Value::String("/test2".to_string())],
        )
        .await
        .unwrap();
    let times_file_2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec![Value::String("/test2/link.txt".to_string())],
        )
        .await
        .unwrap();

    check!(times_dir_1 == times_dir_2);
    check!(times_file_1 == times_file_2);
}

#[test]
#[tracing::instrument]
async fn filesystem_rename_replay_restores_file_times(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("file-service").await;
    let worker_id = executor.start_worker(&component_id, "file-service-9").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-directory}",
            vec![Value::String("/test".to_string())],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-directory}",
            vec![Value::String("/test2".to_string())],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{write-file}",
            vec![
                Value::String("/test/testfile.txt".to_string()),
                Value::String("hello world".to_string()),
            ],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{rename-file}",
            vec![
                Value::String("/test/testfile.txt".to_string()),
                Value::String("/test2/link.txt".to_string()),
            ],
        )
        .await
        .unwrap();

    let times_srcdir_1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec![Value::String("/test".to_string())],
        )
        .await
        .unwrap();
    let times_destdir_1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec![Value::String("/test2".to_string())],
        )
        .await
        .unwrap();
    let times_file_1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec![Value::String("/test2/link.txt".to_string())],
        )
        .await
        .unwrap();

    drop(executor);
    let executor = start(deps, &context).await.unwrap();

    let times_srcdir_2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec![Value::String("/test".to_string())],
        )
        .await
        .unwrap();
    let times_destdir_2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec![Value::String("/test2".to_string())],
        )
        .await
        .unwrap();
    let times_file_2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec![Value::String("/test2/link.txt".to_string())],
        )
        .await
        .unwrap();

    check!(times_srcdir_1 == times_srcdir_2);
    check!(times_destdir_1 == times_destdir_2);
    check!(times_file_1 == times_file_2);
}

#[test]
#[tracing::instrument]
async fn filesystem_remove_file_replay_restores_file_times(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("file-service").await;
    let worker_id = executor
        .start_worker(&component_id, "file-service-10")
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-directory}",
            vec![Value::String("/test".to_string())],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{write-file}",
            vec![
                Value::String("/test/testfile.txt".to_string()),
                Value::String("hello world".to_string()),
            ],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{remove-file}",
            vec![Value::String("/test/testfile.txt".to_string())],
        )
        .await
        .unwrap();
    let times1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec![Value::String("/test".to_string())],
        )
        .await
        .unwrap();

    drop(executor);
    let executor = start(deps, &context).await.unwrap();

    let times2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec![Value::String("/test".to_string())],
        )
        .await
        .unwrap();

    check!(times1 == times2);
}

#[test]
#[tracing::instrument]
async fn filesystem_write_via_stream_replay_restores_file_times(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("file-service").await;
    let worker_id = executor.start_worker(&component_id, "file-service-3").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{write-file}",
            vec![
                Value::String("/testfile.txt".to_string()),
                Value::String("hello world".to_string()),
            ],
        )
        .await
        .unwrap();
    let times1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-file-info}",
            vec![Value::String("/testfile.txt".to_string())],
        )
        .await
        .unwrap();

    drop(executor);
    let executor = start(deps, &context).await.unwrap();

    let times2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-file-info}",
            vec![Value::String("/testfile.txt".to_string())],
        )
        .await
        .unwrap();

    check!(times1 == times2);
}

#[test]
#[tracing::instrument]
async fn filesystem_metadata_hash(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("file-service").await;
    let worker_id = executor.start_worker(&component_id, "file-service-3").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{write-file-direct}",
            vec![
                Value::String("testfile.txt".to_string()),
                Value::String("hello world".to_string()),
            ],
        )
        .await
        .unwrap();
    let hash1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{hash}",
            vec![Value::String("testfile.txt".to_string())],
        )
        .await
        .unwrap();

    drop(executor);
    let executor = start(deps, &context).await.unwrap();

    let hash2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{hash}",
            vec![Value::String("testfile.txt".to_string())],
        )
        .await
        .unwrap();

    check!(hash1 == hash2);
}

#[test]
#[tracing::instrument]
async fn ip_address_resolve(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("networking").await;
    let worker_id = executor
        .start_worker(&component_id, "ip-address-resolve-1")
        .await;

    let result1 = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get}", vec![])
        .await
        .unwrap();

    drop(executor);
    let executor = start(deps, &context).await.unwrap();

    // If the recovery succeeds, that means that the replayed IP address resolution produced the same result as expected

    let result2 = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get}", vec![])
        .await
        .unwrap();

    // Result 2 is a fresh resolution which is not guaranteed to return the same addresses (or the same order) but we can expect
    // that it could resolve golem.cloud to at least one address.
    check!(result1.len() > 0);
    check!(result2.len() > 0);
}

#[test]
#[tracing::instrument]
async fn wasi_incoming_request_handler(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor
        .store_component("wasi-http-incoming-request-handler")
        .await;
    let worker_id = executor
        .start_worker(&component_id, "wasi-http-incoming-request-handler-1")
        .await;

    let args: Value = Value::Record(vec![
        Value::String("http://localhost:8000".to_string()),
        Value::Variant {
            case_idx: 0,
            case_value: None,
        },
        Value::List(vec![]),
        Value::Option(None),
    ]);

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:http/incoming-handler.{handle}",
            vec![args],
        )
        .await
        .unwrap();

    drop(executor);

    check!(result.len() == 1);
    check!(
        result[0]
            == Value::Record(vec![
                Value::U16(200),
                Value::List(vec![]),
                Value::Option(None)
            ])
    );
}

#[test]
#[tracing::instrument]
async fn wasi_incoming_request_handler_echo(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor
        .store_component("wasi-http-incoming-request-handler-echo")
        .await;

    let worker_id = executor
        .start_worker(&component_id, "wasi-http-incoming-request-handler-echo-1")
        .await;

    let args: Value = Value::Record(vec![
        Value::String("http://localhost:8000/foo?bar=baz".to_string()),
        Value::Variant {
            case_idx: 2,
            case_value: None,
        },
        Value::List(vec![Value::Tuple(vec![
            Value::String("test-header".to_string()),
            Value::List(
                "foobar"
                    .to_string()
                    .into_bytes()
                    .into_iter()
                    .map(Value::U8)
                    .collect(),
            ),
        ])]),
        Value::Option(Some(Box::new(Value::Record(vec![
            Value::List(
                "test-body"
                    .to_string()
                    .into_bytes()
                    .into_iter()
                    .map(Value::U8)
                    .collect(),
            ),
            Value::Option(Some(Box::new(Value::List(vec![Value::Tuple(vec![
                Value::String("test-trailer".to_string()),
                Value::List(
                    "barfoo"
                        .to_string()
                        .into_bytes()
                        .into_iter()
                        .map(Value::U8)
                        .collect(),
                ),
            ])])))),
        ])))),
    ]);

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:http/incoming-handler.{handle}",
            vec![args],
        )
        .await
        .unwrap();

    drop(executor);

    check!(result.len() == 1);
    check!(
        result[0]
            == Value::Record(vec![
                Value::U16(200),
                Value::List(vec![
                    Value::Tuple(vec![
                        Value::String("echo-test-header".to_string()),
                        Value::List(
                            "foobar"
                                .to_string()
                                .into_bytes()
                                .into_iter()
                                .map(Value::U8)
                                .collect()
                        )
                    ]),
                    Value::Tuple(vec![
                        Value::String("x-location".to_string()),
                        Value::List(
                            "http://localhost:8000/foo?bar=baz"
                                .to_string()
                                .into_bytes()
                                .into_iter()
                                .map(Value::U8)
                                .collect()
                        )
                    ]),
                    Value::Tuple(vec![
                        Value::String("x-method".to_string()),
                        Value::List(
                            "POST"
                                .to_string()
                                .into_bytes()
                                .into_iter()
                                .map(Value::U8)
                                .collect()
                        )
                    ])
                ]),
                Value::Option(Some(Box::new(Value::Record(vec![
                    Value::List(
                        "test-body"
                            .to_string()
                            .into_bytes()
                            .into_iter()
                            .map(Value::U8)
                            .collect()
                    ),
                    Value::Option(Some(Box::new(Value::List(vec![Value::Tuple(vec![
                        Value::String("echo-test-trailer".to_string()),
                        Value::List(
                            "barfoo"
                                .to_string()
                                .into_bytes()
                                .into_iter()
                                .map(Value::U8)
                                .collect()
                        )
                    ])]),)))
                ]))))
            ])
    );
}

#[test]
#[tracing::instrument]
async fn wasi_incoming_request_handler_state(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor
        .store_component("wasi-http-incoming-request-handler-state")
        .await;

    let worker_id = executor
        .start_worker(&component_id, "wasi-http-incoming-request-handler-state-1")
        .await;

    let args_put: Value = Value::Record(vec![
        Value::String("http://localhost:8000".to_string()),
        Value::Variant {
            case_idx: 3,
            case_value: None,
        },
        Value::List(vec![]),
        Value::Option(Some(Box::new(Value::Record(vec![
            Value::List(
                "1".to_string()
                    .into_bytes()
                    .into_iter()
                    .map(Value::U8)
                    .collect(),
            ),
            Value::Option(None),
        ])))),
    ]);

    let args_get: Value = Value::Record(vec![
        Value::String("http://localhost:8000".to_string()),
        Value::Variant {
            case_idx: 0,
            case_value: None,
        },
        Value::List(vec![]),
        Value::Option(None),
    ]);

    let result1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:http/incoming-handler.{handle}",
            vec![args_put],
        )
        .await
        .unwrap();

    let result2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:http/incoming-handler.{handle}",
            vec![args_get.clone()],
        )
        .await
        .unwrap();

    drop(executor);

    check!(result1.len() == 1);
    check!(
        result1[0]
            == Value::Record(vec![
                Value::U16(200),
                Value::List(vec![]),
                Value::Option(None)
            ])
    );

    check!(result2.len() == 1);
    check!(
        result2[0]
            == Value::Record(vec![
                Value::U16(200),
                Value::List(vec![]),
                Value::Option(Some(Box::new(Value::Record(vec![
                    Value::List(
                        "1".to_string()
                            .into_bytes()
                            .into_iter()
                            .map(Value::U8)
                            .collect()
                    ),
                    Value::Option(None)
                ]))))
            ])
    );

    // restart executor and check whether we are restoring the state
    let executor = start(deps, &context).await.unwrap();

    let result3 = executor
        .invoke_and_await(
            &worker_id,
            "golem:http/incoming-handler.{handle}",
            vec![args_get.clone()],
        )
        .await
        .unwrap();

    drop(executor);

    check!(result3.len() == 1);
    check!(
        result3[0]
            == Value::Record(vec![
                Value::U16(200),
                Value::List(vec![]),
                Value::Option(Some(Box::new(Value::Record(vec![
                    Value::List(
                        "1".to_string()
                            .into_bytes()
                            .into_iter()
                            .map(Value::U8)
                            .collect()
                    ),
                    Value::Option(None)
                ]))))
            ])
    );
}
