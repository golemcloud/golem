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

use crate::common::{start, TestContext};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use assert2::{assert, check};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{BoxError, Router};
use bytes::Bytes;
use futures::stream;
use golem_common::model::oplog::WorkerError;
use golem_common::model::{
    ComponentFilePermissions, ComponentFileSystemNode, ComponentFileSystemNodeDetails,
    IdempotencyKey, WorkerStatus,
};
use golem_common::virtual_exports::http_incoming_handler::IncomingHttpRequest;
use golem_test_framework::config::TestDependencies;
use golem_test_framework::dsl::{
    drain_connection, stderr_events, stdout_events, worker_error_logs, worker_error_message,
    worker_error_underlying_error, TestDslUnsafe,
};
use golem_wasm::{IntoValueAndType, Value, ValueAndType};
use http::{HeaderMap, StatusCode};
use std::collections::HashMap;
use std::sync::atomic::AtomicU8;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use test_r::{inherit_test_dep, test};
use tokio::spawn;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_stream::StreamExt;
use tracing::{debug, info, Instrument};

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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("write-stdout").store().await;
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

    executor.check_oplog_is_queryable(&worker_id).await;

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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("write-stderr").store().await;
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

    executor.check_oplog_is_queryable(&worker_id).await;

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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("read-stdin").store().await;
    let worker_id = executor.start_worker(&component_id, "read-stdin-1").await;

    let result = executor.invoke_and_await(&worker_id, "run", vec![]).await;

    executor.check_oplog_is_queryable(&worker_id).await;

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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("clocks").store().await;
    let worker_id = executor.start_worker(&component_id, "clocks-1").await;

    let result = executor
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("file-write-read-delete").store().await;
    let mut env = HashMap::new();
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());
    let worker_id = executor
        .start_worker_with(
            &component_id,
            "file-write-read-delete-1",
            vec![],
            env,
            vec![],
        )
        .await;

    let result = executor
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_files = executor
        .add_initial_component_files(&[
            (
                "initial-file-read-write/files/foo.txt",
                "foo.txt",
                ComponentFilePermissions::ReadOnly,
            ),
            (
                "initial-file-read-write/files/baz.txt",
                "/bar/baz.txt",
                ComponentFilePermissions::ReadWrite,
            ),
        ])
        .await;

    let component_id = executor
        .component("initial-file-read-write")
        .unique()
        .with_files(&component_files)
        .store()
        .await;
    let mut env = HashMap::new();
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());
    let worker_id = executor
        .start_worker_with(
            &component_id,
            "initial-file-read-write-1",
            vec![],
            env,
            vec![],
        )
        .await;

    let result = executor
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_files = executor
        .add_initial_component_files(&[
            (
                "initial-file-read-write/files/foo.txt",
                "/foo.txt",
                ComponentFilePermissions::ReadOnly,
            ),
            (
                "initial-file-read-write/files/baz.txt",
                "/bar/baz.txt",
                ComponentFilePermissions::ReadWrite,
            ),
            (
                "initial-file-read-write/files/baz.txt",
                "/baz.txt",
                ComponentFilePermissions::ReadWrite,
            ),
        ])
        .await;

    let component_id = executor
        .component("initial-file-read-write")
        .unique()
        .with_files(&component_files)
        .store()
        .await;

    let worker_id = executor
        .start_worker(&component_id, "initial-file-read-write-1")
        .await;

    let result = executor.get_file_system_node(&worker_id, "/").await;

    let mut result = result
        .into_iter()
        .map(|e| ComponentFileSystemNode {
            last_modified: SystemTime::UNIX_EPOCH,
            ..e
        })
        .collect::<Vec<_>>();

    result.sort_by_key(|e| e.name.clone());

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

    let result = executor.get_file_system_node(&worker_id, "/bar").await;

    let mut result = result
        .into_iter()
        .map(|e| ComponentFileSystemNode {
            last_modified: SystemTime::UNIX_EPOCH,
            ..e
        })
        .collect::<Vec<_>>();

    result.sort_by_key(|e| e.name.clone());

    check!(
        result
            == vec![ComponentFileSystemNode {
                name: "baz.txt".to_string(),
                last_modified: SystemTime::UNIX_EPOCH,
                details: ComponentFileSystemNodeDetails::File {
                    permissions: ComponentFilePermissions::ReadWrite,
                    size: 4,
                }
            },]
    );

    let result = executor.get_file_system_node(&worker_id, "/baz.txt").await;

    let mut result = result
        .into_iter()
        .map(|e| ComponentFileSystemNode {
            last_modified: SystemTime::UNIX_EPOCH,
            ..e
        })
        .collect::<Vec<_>>();

    result.sort_by_key(|e| e.name.clone());

    check!(
        result
            == vec![ComponentFileSystemNode {
                name: "baz.txt".to_string(),
                last_modified: SystemTime::UNIX_EPOCH,
                details: ComponentFileSystemNodeDetails::File {
                    permissions: ComponentFilePermissions::ReadWrite,
                    size: 4,
                }
            },]
    );

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);
}

#[test]
#[tracing::instrument]
async fn initial_file_reading_through_api(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_files = executor
        .add_initial_component_files(&[
            (
                "initial-file-read-write/files/foo.txt",
                "/foo.txt",
                ComponentFilePermissions::ReadOnly,
            ),
            (
                "initial-file-read-write/files/baz.txt",
                "/bar/baz.txt",
                ComponentFilePermissions::ReadWrite,
            ),
        ])
        .await;

    let component_id = executor
        .component("initial-file-read-write")
        .unique()
        .with_files(&component_files)
        .store()
        .await;
    let mut env = HashMap::new();
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());
    let worker_id = executor
        .start_worker_with(
            &component_id,
            "initial-file-read-write-3",
            vec![],
            env,
            vec![],
        )
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

    executor.check_oplog_is_queryable(&worker_id).await;

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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("directories").store().await;
    let worker_id = executor.start_worker(&component_id, "directories-1").await;

    let result = executor
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("directories").store().await;
    let worker_id = executor.start_worker(&component_id, "directories-1").await;

    let result = executor
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("file-service").store().await;
    let worker_id = executor.start_worker(&component_id, "file-service-1").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{write-file}",
            vec![
                "/testfile.txt".into_value_and_type(),
                "hello world".into_value_and_type(),
            ],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{read-file}",
            vec!["/testfile.txt".into_value_and_type()],
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
async fn file_update_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = {
        let component_files = executor
            .add_initial_component_files(&[(
                "ifs-update/files/foo.txt",
                "/foo.txt",
                ComponentFilePermissions::ReadOnly,
            )])
            .await;

        executor
            .component("golem_it_ifs_update")
            .unique()
            .with_files(&component_files)
            .store()
            .await
    };

    let worker_id = executor.start_worker(&component_id, "ifs-update-1").await;

    executor
        .invoke_and_await(
            &worker_id,
            "golem-it:ifs-update-exports/golem-it-ifs-update-api.{load-file}",
            vec![],
        )
        .await
        .unwrap();

    {
        let content_before_update = executor
            .invoke_and_await(
                &worker_id,
                "golem-it:ifs-update-exports/golem-it-ifs-update-api.{get-file-content}",
                vec![],
            )
            .await
            .unwrap();

        check!(content_before_update[0] == Value::String("foo\n".to_string()));
    }

    {
        let component_files = executor
            .add_initial_component_files(&[(
                "ifs-update/files/bar.txt",
                "/foo.txt",
                ComponentFilePermissions::ReadOnly,
            )])
            .await;

        let target_version = executor
            .update_component_with_files(
                &component_id,
                "golem_it_ifs_update",
                Some(&component_files),
            )
            .await;

        executor
            .auto_update_worker(&worker_id, target_version)
            .await;
    };

    {
        let content_after_update = executor
            .invoke_and_await(
                &worker_id,
                "golem-it:ifs-update-exports/golem-it-ifs-update-api.{get-file-content}",
                vec![],
            )
            .await
            .unwrap();

        check!(content_after_update[0] == Value::String("foo\n".to_string()));
    }

    executor.simulated_crash(&worker_id).await;

    {
        let content_after_crash = executor
            .invoke_and_await(
                &worker_id,
                "golem-it:ifs-update-exports/golem-it-ifs-update-api.{get-file-content}",
                vec![],
            )
            .await
            .unwrap();

        check!(content_after_crash[0] == Value::String("foo\n".to_string()));
    }

    executor
        .invoke_and_await(
            &worker_id,
            "golem-it:ifs-update-exports/golem-it-ifs-update-api.{load-file}",
            vec![],
        )
        .await
        .unwrap();

    {
        let content_after_reload = executor
            .invoke_and_await(
                &worker_id,
                "golem-it:ifs-update-exports/golem-it-ifs-update-api.{get-file-content}",
                vec![],
            )
            .await
            .unwrap();

        check!(content_after_reload[0] == Value::String("bar\n".to_string()));
    }

    executor.simulated_crash(&worker_id).await;

    {
        let content_after_crash = executor
            .invoke_and_await(
                &worker_id,
                "golem-it:ifs-update-exports/golem-it-ifs-update-api.{get-file-content}",
                vec![],
            )
            .await
            .unwrap();

        check!(content_after_crash[0] == Value::String("bar\n".to_string()));
    }

    {
        let component_files = executor
            .add_initial_component_files(&[(
                "ifs-update/files/baz.txt",
                "/foo.txt",
                ComponentFilePermissions::ReadOnly,
            )])
            .await;

        let target_version = executor
            .update_component_with_files(
                &component_id,
                "golem_it_ifs_update",
                Some(&component_files),
            )
            .await;

        executor
            .manual_update_worker(&worker_id, target_version)
            .await;
    };

    {
        let content_after_manual_update = executor
            .invoke_and_await(
                &worker_id,
                "golem-it:ifs-update-exports/golem-it-ifs-update-api.{get-file-content}",
                vec![],
            )
            .await
            .unwrap();

        check!(content_after_manual_update[0] == Value::String("restored".to_string()));
    }

    executor
        .invoke_and_await(
            &worker_id,
            "golem-it:ifs-update-exports/golem-it-ifs-update-api.{load-file}",
            vec![],
        )
        .await
        .unwrap();

    {
        let content_after_reload = executor
            .invoke_and_await(
                &worker_id,
                "golem-it:ifs-update-exports/golem-it-ifs-update-api.{get-file-content}",
                vec![],
            )
            .await
            .unwrap();

        check!(content_after_reload[0] == Value::String("baz\n".to_string()));
    }

    executor.simulated_crash(&worker_id).await;

    {
        let content_after_crash = executor
            .invoke_and_await(
                &worker_id,
                "golem-it:ifs-update-exports/golem-it-ifs-update-api.{get-file-content}",
                vec![],
            )
            .await
            .unwrap();

        check!(content_after_crash[0] == Value::String("baz\n".to_string()));
    }

    executor.delete_worker(&worker_id).await;
}

#[test]
#[tracing::instrument]
async fn file_update_in_the_middle_of_exported_function(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let (sender, mut latch) = tokio::sync::mpsc::channel::<()>(1);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = {
        let sender = Arc::new(sender);
        let first_request = Arc::new(tokio::sync::Mutex::new(true));
        let start_server = async {
            let route = Router::new().route(
                "/",
                get(async move || {
                    sender.send(()).await.unwrap();
                    let mut first_request = first_request.lock().await;
                    if *first_request {
                        (*first_request) = false;
                        tokio::time::sleep(Duration::from_secs(600)).await;
                    }
                    "Hello, World!".to_string()
                }),
            );

            axum::serve(listener, route).await.unwrap();
        };

        spawn(start_server.in_current_span())
    };

    let component_id = {
        let component_files = executor
            .add_initial_component_files(&[(
                "ifs-update-inside-exported-function/files/foo.txt",
                "/foo.txt",
                ComponentFilePermissions::ReadOnly,
            )])
            .await;

        let env = vec![
            ("PORT".to_string(), host_http_port.to_string()),
            ("RUST_BACKTRACE".to_string(), "full".to_string()),
        ];

        executor
            .component("golem_it_ifs_update_inside_exported_function")
            .unique()
            .with_files(&component_files)
            .with_env(env)
            .store()
            .await
    };

    let worker_id = executor.start_worker(&component_id, "ifs-update-1").await;

    let idempotency_key = IdempotencyKey::fresh();

    executor
        .invoke_with_key(
            &worker_id,
            &idempotency_key,
            "golem-it:ifs-update-inside-exported-function-exports/golem-it-ifs-update-inside-exported-function-api.{run}",
            vec![],
        )
        .await
        .unwrap();

    latch.recv().await.expect("channel should produce value");

    {
        let component_files = executor
            .add_initial_component_files(&[(
                "ifs-update-inside-exported-function/files/bar.txt",
                "/foo.txt",
                ComponentFilePermissions::ReadOnly,
            )])
            .await;

        let target_version = executor
            .update_component_with_files(
                &component_id,
                "golem_it_ifs_update_inside_exported_function",
                Some(&component_files),
            )
            .await;

        executor
            .auto_update_worker(&worker_id, target_version)
            .await;
    };

    {
        let result = executor
            .invoke_and_await_with_key(
                &worker_id,
                &idempotency_key,
                "golem-it:ifs-update-inside-exported-function-exports/golem-it-ifs-update-inside-exported-function-api.{run}",
                vec![],
            )
            .await
            .unwrap();

        check!(
            result[0]
                == Value::Tuple(vec![
                    Value::String("foo\n".to_string()),
                    Value::String("bar\n".to_string())
                ])
        );
    }

    http_server.abort();
}

#[test]
#[tracing::instrument]
async fn environment_service(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("environment-service").store().await;
    let args = vec!["test-arg".to_string()];
    let mut env = HashMap::new();
    env.insert("TEST_ENV".to_string(), "test-value".to_string());
    let worker_id = executor
        .start_worker_with(&component_id, "environment-service-1", args, env, vec![])
        .await;

    let args_result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-arguments}", vec![])
        .await
        .unwrap();

    let env_result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-environment}", vec![])
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

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
                    Value::String("GOLEM_AGENT_ID".to_string()),
                    Value::String("environment-service-1".to_string())
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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = spawn(
        async move {
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

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component_id = executor.component("http-client").store().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "http-client-2", vec![], env, vec![])
        .await;
    let rx = executor.capture_output(&worker_id).await;

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api.{send-request}", vec![])
        .await
        .expect("first send-request failed");

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);
    drop(rx);

    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;
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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let (signal_tx, mut signal_rx) = tokio::sync::mpsc::unbounded_channel();
    let idempotency_keys = Arc::new(Mutex::new(Vec::new()));
    let idempotency_keys_clone = idempotency_keys.clone();

    let http_server = spawn(
        async move {
            let route = Router::new().route(
                "/big-byte-array",
                get(move |headers: HeaderMap| async move {
                    let idempotency_key = headers
                        .get("idempotency-key")
                        .map(|h| h.to_str().unwrap().to_string());
                    if let Some(key) = idempotency_key {
                        let mut keys = idempotency_keys_clone.lock().unwrap();
                        keys.push(key);
                    }
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

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component_id = executor.component("http-client-2").store().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "http-client-2", vec![], env, vec![])
        .await;
    let rx = executor.capture_output_with_termination(&worker_id).await;

    let key = IdempotencyKey::fresh();

    let executor_clone = executor.clone();
    let worker_id_clone = worker_id.clone();
    let key_clone = key.clone();
    let _handle = spawn(
        async move {
            let _ = executor_clone
                .invoke_and_await_with_key(
                    &worker_id_clone,
                    &key_clone,
                    "golem:it/api.{slow-body-stream}",
                    vec![],
                )
                .await;
        }
        .in_current_span(),
    );

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

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

    http_server.abort();

    check!(result == Ok(vec![Value::U64(100 * 1024)]));

    let idempotency_keys = idempotency_keys.lock().unwrap();
    check!(idempotency_keys.len() == 2);
    check!(idempotency_keys[0] == idempotency_keys[1]);
}

#[test]
#[tracing::instrument]
async fn http_client_interrupting_response_stream_async(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let (signal_tx, mut signal_rx) = tokio::sync::mpsc::unbounded_channel();
    let idempotency_keys = Arc::new(Mutex::new(Vec::new()));
    let idempotency_keys_clone = idempotency_keys.clone();

    let http_server = spawn(
        async move {
            let route = Router::new().route(
                "/big-byte-array",
                get(move |headers: HeaderMap| async move {
                    let idempotency_key = headers
                        .get("idempotency-key")
                        .map(|h| h.to_str().unwrap().to_string());
                    if let Some(key) = idempotency_key {
                        let mut keys = idempotency_keys_clone.lock().unwrap();
                        keys.push(key);
                    }
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

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component_id = executor.component("http-client-3").store().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "http-client-2-async", vec![], env, vec![])
        .await;
    let rx = executor.capture_output_with_termination(&worker_id).await;

    let key = IdempotencyKey::fresh();

    let executor_clone = executor.clone();
    let worker_id_clone = worker_id.clone();
    let key_clone = key.clone();
    let _handle = spawn(
        async move {
            let _ = executor_clone
                .invoke_and_await_with_key(
                    &worker_id_clone,
                    &key_clone,
                    "golem:it/api.{slow-body-stream}",
                    vec![],
                )
                .await;
        }
        .in_current_span(),
    );

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

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

    http_server.abort();

    check!(result == Ok(vec![Value::U64(100 * 1024)]));

    let idempotency_keys = idempotency_keys.lock().unwrap();
    check!(idempotency_keys.len() == 2);
    check!(idempotency_keys[0] == idempotency_keys[1]);
}

#[test]
#[tracing::instrument]
async fn sleep(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("clock-service").store().await;
    let worker_id = executor
        .start_worker(&component_id, "clock-service-1")
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{sleep}",
            vec![10u64.into_value_and_type()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let start = Instant::now();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{sleep}",
            vec![0u64.into_value_and_type()],
        )
        .await
        .unwrap();
    let duration = start.elapsed();

    check!(duration.as_secs() < 2);
}

#[test]
#[tracing::instrument]
async fn sleep_less_than_suspend_threshold(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("clock-service").store().await;
    let worker_id = executor
        .start_worker(&component_id, "clock-service-2")
        .await;

    let start = Instant::now();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{sleep}",
            vec![1u64.into_value_and_type()],
        )
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{healthcheck}", vec![])
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    check!(duration.as_secs() >= 1);
    check!(result == vec![Value::Bool(true)]);
}

#[test]
#[tracing::instrument]
async fn sleep_longer_than_suspend_threshold(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("clock-service").store().await;
    let worker_id = executor
        .start_worker(&component_id, "clock-service-3")
        .await;

    let start = Instant::now();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{sleep}",
            vec![12u64.into_value_and_type()],
        )
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{healthcheck}", vec![])
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    check!(duration.as_secs() >= 12);
    check!(result == vec![Value::Bool(true)]);
}

async fn simulated_slow_request_server(delay: Duration) -> (u16, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = spawn(
        async move {
            let route = Router::new().route(
                "/simulated-slow-request",
                get(move || async move {
                    tokio::time::sleep(delay).await;
                    "slow response".to_string()
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    (host_http_port, http_server)
}

#[test]
#[tracing::instrument]
async fn sleep_less_than_suspend_threshold_while_awaiting_response(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let (port, server) = simulated_slow_request_server(Duration::from_secs(10)).await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let component_id = executor.component("clock-service").store().await;
    let worker_id = executor
        .start_worker_with(&component_id, "clock-service-4", vec![], env, vec![])
        .await;

    let start = Instant::now();
    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{sleep-during-request}",
            vec![2u64.into_value_and_type()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    server.abort();
    drop(executor);

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    check!(duration.as_secs() >= 2);
    check!(duration.as_secs() < 10);
    check!(result == vec![Value::String("Timeout".to_string())]);
}

#[test]
#[tracing::instrument]
async fn sleep_longer_than_suspend_threshold_while_awaiting_response(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let (port, server) = simulated_slow_request_server(Duration::from_secs(5)).await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let component_id = executor.component("clock-service").store().await;
    let worker_id = executor
        .start_worker_with(&component_id, "clock-service-5", vec![], env, vec![])
        .await;

    let start = Instant::now();
    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{sleep-during-request}",
            vec![30u64.into_value_and_type()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    server.abort();
    drop(executor);

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    check!(duration.as_secs() >= 5);
    check!(duration.as_secs() < 30);
    check!(result == vec![Value::String("slow response".to_string())]);
}

#[test]
#[tracing::instrument]
async fn sleep_longer_than_suspend_threshold_while_awaiting_response_2(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let (port, server) = simulated_slow_request_server(Duration::from_secs(30)).await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let component_id = executor.component("clock-service").store().await;
    let worker_id = executor
        .start_worker_with(&component_id, "clock-service-6", vec![], env, vec![])
        .await;

    let start = Instant::now();
    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{sleep-during-request}",
            vec![15u64.into_value_and_type()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    server.abort();
    drop(executor);

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    check!(duration.as_secs() >= 15);
    check!(duration.as_secs() < 30);
    check!(result == vec![Value::String("Timeout".to_string())]);
}

#[test]
#[tracing::instrument]
async fn sleep_and_awaiting_parallel_responses(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let (port, server) = simulated_slow_request_server(Duration::from_secs(2)).await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let component_id = executor.component("clock-service").store().await;
    let worker_id = executor
        .start_worker_with(&component_id, "clock-service-7", vec![], env, vec![])
        .await;

    let start = Instant::now();
    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{sleep-during-parallel-requests}",
            vec![20u64.into_value_and_type()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);
    server.abort();

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    info!("Restarting worker...");
    let executor = crate::common::start(deps, &context)
        .await
        .unwrap()
        .into_admin()
        .await;
    info!("Worker restarted");

    let healthcheck_result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{healthcheck}", vec![])
        .await
        .unwrap();

    // server.abort();

    check!(duration.as_secs() >= 10);
    check!(duration.as_secs() < 20);
    check!(result == vec![Value::String("Ok(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\n".to_string())]);
    check!(healthcheck_result == vec![Value::Bool(true)]);
}

#[test]
#[tracing::instrument]
async fn sleep_below_threshold_between_http_responses(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let (port, server) = simulated_slow_request_server(Duration::from_secs(1)).await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let component_id = executor.component("clock-service").store().await;
    let worker_id = executor
        .start_worker_with(&component_id, "clock-service-8", vec![], env, vec![])
        .await;

    let _ = executor.log_output(&worker_id).await;

    let start = Instant::now();
    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{sleep-between-requests}",
            vec![1u64.into_value_and_type(), 5u64.into_value_and_type()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    server.abort();
    drop(executor);
    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    info!("Restarting worker...");
    let executor = crate::common::start(deps, &context)
        .await
        .unwrap()
        .into_admin()
        .await;
    info!("Worker restarted");

    let healthcheck_result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{healthcheck}", vec![])
        .await
        .unwrap();

    check!(duration.as_secs() >= 10);
    check!(result == vec![Value::String("Ok(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\n".to_string())]);
    check!(healthcheck_result == vec![Value::Bool(true)]);
}

#[test]
#[tracing::instrument]
async fn sleep_above_threshold_between_http_responses(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let (port, server) = simulated_slow_request_server(Duration::from_secs(1)).await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let component_id = executor.component("clock-service").store().await;
    let worker_id = executor
        .start_worker_with(&component_id, "clock-service-9", vec![], env, vec![])
        .await;

    let start = Instant::now();
    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{sleep-between-requests}",
            vec![12u64.into_value_and_type(), 2u64.into_value_and_type()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    server.abort();
    drop(executor);
    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    info!("Restarting worker...");
    let executor = crate::common::start(deps, &context)
        .await
        .unwrap()
        .into_admin()
        .await;
    info!("Worker restarted");

    let healthcheck_result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{healthcheck}", vec![])
        .await
        .unwrap();

    check!(duration.as_secs() >= 14);
    check!(
        result
            == vec![Value::String(
                "Ok(\"slow response\")\nOk(\"slow response\")\n".to_string()
            )]
    );
    check!(healthcheck_result == vec![Value::Bool(true)]);
}

#[test]
#[tracing::instrument]
async fn resuming_sleep(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("clock-service").store().await;
    let worker_id = executor
        .start_worker(&component_id, "clock-service-2")
        .await;

    let executor_clone = executor.clone();
    let worker_id_clone = worker_id.clone();
    let fiber = spawn(
        async move {
            executor_clone
                .invoke_and_await(
                    &worker_id_clone,
                    "golem:it/api.{sleep}",
                    vec![10u64.into_value_and_type()],
                )
                .await
                .unwrap();
        }
        .in_current_span(),
    );

    tokio::time::sleep(Duration::from_secs(5)).await;

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);
    let _ = fiber.await;

    info!("Restarting worker...");

    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    info!("Worker restarted");

    let start = Instant::now();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{sleep}",
            vec![10u64.into_value_and_type()],
        )
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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("failing-component").store().await;
    let worker_id = executor
        .start_worker(&component_id, "failing-worker-1")
        .await;

    let result1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:component/api.{add}",
            vec![5u64.into_value_and_type()],
        )
        .await;

    let result2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:component/api.{add}",
            vec![50u64.into_value_and_type()],
        )
        .await;

    let result3 = executor
        .invoke_and_await(&worker_id, "golem:component/api.{get}", vec![])
        .await;

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

    check!(result1.is_ok());
    check!(result2.is_err());
    check!(result3.is_err());

    let result2_err = result2.err().unwrap();
    assert_eq!(worker_error_message(&result2_err), "Invocation failed");
    assert!(
        matches!(worker_error_underlying_error(&result2_err), Some(WorkerError::Unknown(error)) if error.starts_with("error while executing at wasm backtrace:") && error.contains("failing_component.wasm!golem:component/api#add"))
    );
    assert_eq!(worker_error_logs(&result2_err), Some("error log message\n\nthread '<unnamed>' panicked at src/lib.rs:31:17:\nvalue is too large\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n".to_string()));
    let result3_err = result3.err().unwrap();
    assert_eq!(
        worker_error_message(&result3_err),
        "Previous invocation failed"
    );
    assert!(
        matches!(worker_error_underlying_error(&result3_err), Some(WorkerError::Unknown(error)) if error.starts_with("error while executing at wasm backtrace:") && error.contains("failing_component.wasm!golem:component/api#add"))
    );
    assert_eq!(worker_error_logs(&result3_err), Some("error log message\n\nthread '<unnamed>' panicked at src/lib.rs:31:17:\nvalue is too large\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n".to_string()));
}

#[test]
#[tracing::instrument]
async fn file_service_write_direct(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("file-service").store().await;
    let worker_id = executor.start_worker(&component_id, "file-service-2").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{write-file-direct}",
            vec![
                "testfile.txt".into_value_and_type(),
                "hello world".into_value_and_type(),
            ],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{read-file}",
            vec!["/testfile.txt".into_value_and_type()],
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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("file-service").store().await;
    let worker_id = executor.start_worker(&component_id, "file-service-3").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{write-file-direct}",
            vec![
                "testfile.txt".into_value_and_type(),
                "hello world".into_value_and_type(),
            ],
        )
        .await
        .unwrap();
    let times1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-file-info}",
            vec!["/testfile.txt".into_value_and_type()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let times2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-file-info}",
            vec!["/testfile.txt".into_value_and_type()],
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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("file-service").store().await;
    let worker_id = executor.start_worker(&component_id, "file-service-4").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-directory}",
            vec!["/test".into_value_and_type()],
        )
        .await
        .unwrap();
    let times1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec!["/".into_value_and_type()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let times2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec!["/".into_value_and_type()],
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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("file-service").store().await;
    let worker_id = executor.start_worker(&component_id, "file-service-5").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{write-file}",
            vec![
                "/testfile.txt".into_value_and_type(),
                "hello world".into_value_and_type(),
            ],
        )
        .await
        .unwrap();

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-link}",
            vec![
                "/testfile.txt".into_value_and_type(),
                "/link.txt".into_value_and_type(),
            ],
        )
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{read-file}",
            vec!["/link.txt".into_value_and_type()],
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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("file-service").store().await;
    let worker_id = executor.start_worker(&component_id, "file-service-6").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-directory}",
            vec!["/test".into_value_and_type()],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-directory}",
            vec!["/test2".into_value_and_type()],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{write-file}",
            vec![
                "/test/testfile.txt".into_value_and_type(),
                "hello world".into_value_and_type(),
            ],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-link}",
            vec![
                "/test/testfile.txt".into_value_and_type(),
                "/test2/link.txt".into_value_and_type(),
            ],
        )
        .await
        .unwrap();

    let times_file_1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec!["/test2/link.txt".into_value_and_type()],
        )
        .await
        .unwrap();
    let times_dir_1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec!["/test2".into_value_and_type()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let times_dir_2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec!["/test2".into_value_and_type()],
        )
        .await
        .unwrap();
    let times_file_2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec!["/test2/link.txt".into_value_and_type()],
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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("file-service").store().await;
    let worker_id = executor.start_worker(&component_id, "file-service-7").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-directory}",
            vec!["/test".into_value_and_type()],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-directory}",
            vec!["/test/a".into_value_and_type()],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{remove-directory}",
            vec!["/test/a".into_value_and_type()],
        )
        .await
        .unwrap();
    let times1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec!["/test".into_value_and_type()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let times2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec!["/test".into_value_and_type()],
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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("file-service").store().await;
    let worker_id = executor.start_worker(&component_id, "file-service-8").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-directory}",
            vec!["/test".into_value_and_type()],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-directory}",
            vec!["/test2".into_value_and_type()],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{write-file-direct}",
            vec![
                "test/testfile.txt".into_value_and_type(),
                "hello world".into_value_and_type(),
            ],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-sym-link}",
            vec![
                "../test/testfile.txt".into_value_and_type(),
                "/test2/link.txt".into_value_and_type(),
            ],
        )
        .await
        .unwrap();

    let times_file_1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec!["/test2/link.txt".into_value_and_type()],
        )
        .await
        .unwrap();
    let times_dir_1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec!["/test2".into_value_and_type()],
        )
        .await
        .unwrap();

    drop(executor);

    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let times_dir_2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec!["/test2".into_value_and_type()],
        )
        .await
        .unwrap();
    let times_file_2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec!["/test2/link.txt".into_value_and_type()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("file-service").store().await;
    let worker_id = executor.start_worker(&component_id, "file-service-9").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-directory}",
            vec!["/test".into_value_and_type()],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-directory}",
            vec!["/test2".into_value_and_type()],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{write-file}",
            vec![
                "/test/testfile.txt".into_value_and_type(),
                "hello world".into_value_and_type(),
            ],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{rename-file}",
            vec![
                "/test/testfile.txt".into_value_and_type(),
                "/test2/link.txt".into_value_and_type(),
            ],
        )
        .await
        .unwrap();

    let times_srcdir_1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec!["/test".into_value_and_type()],
        )
        .await
        .unwrap();
    let times_destdir_1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec!["/test2".into_value_and_type()],
        )
        .await
        .unwrap();
    let times_file_1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec!["/test2/link.txt".into_value_and_type()],
        )
        .await
        .unwrap();

    drop(executor);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let times_srcdir_2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec!["/test".into_value_and_type()],
        )
        .await
        .unwrap();
    let times_destdir_2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec!["/test2".into_value_and_type()],
        )
        .await
        .unwrap();
    let times_file_2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec!["/test2/link.txt".into_value_and_type()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;
    drop(executor);

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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("file-service").store().await;
    let worker_id = executor
        .start_worker(&component_id, "file-service-10")
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-directory}",
            vec!["/test".into_value_and_type()],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{write-file}",
            vec![
                "/test/testfile.txt".into_value_and_type(),
                "hello world".into_value_and_type(),
            ],
        )
        .await
        .unwrap();
    let info1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec!["/test/testfile.txt".into_value_and_type()],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{remove-file}",
            vec!["/test/testfile.txt".into_value_and_type()],
        )
        .await
        .unwrap();
    let info2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec!["/test/testfile.txt".into_value_and_type()],
        )
        .await
        .unwrap();
    let times1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec!["/test".into_value_and_type()],
        )
        .await
        .unwrap();

    drop(executor);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let times2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-info}",
            vec!["/test".into_value_and_type()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;
    drop(executor);

    println!("{:?}", info1);
    println!("{:?}", info2);

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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("file-service").store().await;
    let worker_id = executor.start_worker(&component_id, "file-service-3").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{write-file}",
            vec![
                "/testfile.txt".into_value_and_type(),
                "hello world".into_value_and_type(),
            ],
        )
        .await
        .unwrap();
    let times1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-file-info}",
            vec!["/testfile.txt".into_value_and_type()],
        )
        .await
        .unwrap();

    drop(executor);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let times2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-file-info}",
            vec!["/testfile.txt".into_value_and_type()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("file-service").store().await;
    let worker_id = executor.start_worker(&component_id, "file-service-3").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{write-file-direct}",
            vec![
                "testfile.txt".into_value_and_type(),
                "hello world".into_value_and_type(),
            ],
        )
        .await
        .unwrap();
    let hash1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{hash}",
            vec!["testfile.txt".into_value_and_type()],
        )
        .await
        .unwrap();

    drop(executor);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let hash2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{hash}",
            vec!["testfile.txt".into_value_and_type()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("networking").store().await;
    let worker_id = executor
        .start_worker(&component_id, "ip-address-resolve-1")
        .await;

    let result1 = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get}", vec![])
        .await
        .unwrap();

    drop(executor);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    // If the recovery succeeds, that means that the replayed IP address resolution produced the same result as expected

    let result2 = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get}", vec![])
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor
        .component("wasi-http-incoming-request-handler")
        .store()
        .await;
    let worker_id = executor
        .start_worker(&component_id, "wasi-http-incoming-request-handler-1")
        .await;

    let args = ValueAndType {
        value: Value::Record(vec![
            Value::Variant {
                case_idx: 0,
                case_value: None,
            },
            Value::Variant {
                case_idx: 0,
                case_value: None,
            },
            Value::String("localhost:8000".to_string()),
            Value::String("/".to_string()),
            Value::List(vec![]),
            Value::Option(None),
        ]),
        typ: IncomingHttpRequest::analysed_type(),
    };

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:http/incoming-handler.{handle}",
            vec![args],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor
        .component("wasi-http-incoming-request-handler-echo")
        .store()
        .await;

    let worker_id = executor
        .start_worker(&component_id, "wasi-http-incoming-request-handler-echo-1")
        .await;

    let args = ValueAndType {
        value: Value::Record(vec![
            Value::Variant {
                case_idx: 2,
                case_value: None,
            },
            Value::Variant {
                case_idx: 0,
                case_value: None,
            },
            Value::String("localhost:8000".to_string()),
            Value::String("/foo?bar=baz".to_string()),
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
        ]),
        typ: IncomingHttpRequest::analysed_type(),
    };

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:http/incoming-handler.{handle}",
            vec![args],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor
        .component("wasi-http-incoming-request-handler-state")
        .store()
        .await;

    let worker_id = executor
        .start_worker(&component_id, "wasi-http-incoming-request-handler-state-1")
        .await;

    let args_put = ValueAndType {
        value: Value::Record(vec![
            Value::Variant {
                case_idx: 3,
                case_value: None,
            },
            Value::Variant {
                case_idx: 0,
                case_value: None,
            },
            Value::String("localhost:8000".to_string()),
            Value::String("/".to_string()),
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
        ]),
        typ: IncomingHttpRequest::analysed_type(),
    };

    let args_get = ValueAndType {
        value: Value::Record(vec![
            Value::Variant {
                case_idx: 0,
                case_value: None,
            },
            Value::Variant {
                case_idx: 0,
                case_value: None,
            },
            Value::String("localhost:8000".to_string()),
            Value::String("/".to_string()),
            Value::List(vec![]),
            Value::Option(None),
        ]),
        typ: IncomingHttpRequest::analysed_type(),
    };

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
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let result3 = executor
        .invoke_and_await(
            &worker_id,
            "golem:http/incoming-handler.{handle}",
            vec![args_get.clone()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

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

#[test]
#[tracing::instrument]
async fn wasi_config_initial_worker_config(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("golem_it_wasi_config").store().await;

    let worker_id = executor
        .start_worker_with(
            &component_id,
            "worker-1",
            Vec::new(),
            HashMap::new(),
            vec![
                ("k1".to_string(), "v1".to_string()),
                ("k2".to_string(), "v2".to_string()),
            ],
        )
        .await;

    {
        // get existing key

        let result = executor
            .invoke_and_await(
                &worker_id,
                "golem-it:wasi-config-exports/golem-it-wasi-config-api.{get}",
                vec!["k1".into_value_and_type()],
            )
            .await
            .unwrap();

        assert_eq!(
            result,
            vec![Value::Option(Some(Box::new(Value::String(
                "v1".to_string()
            ))))]
        )
    }

    {
        // get non-existent key

        let result = executor
            .invoke_and_await(
                &worker_id,
                "golem-it:wasi-config-exports/golem-it-wasi-config-api.{get}",
                vec!["k3".into_value_and_type()],
            )
            .await
            .unwrap();

        assert_eq!(result, vec![Value::Option(None)])
    }

    {
        // get all keys

        let result = executor
            .invoke_and_await(
                &worker_id,
                "golem-it:wasi-config-exports/golem-it-wasi-config-api.{get-all}",
                vec![],
            )
            .await
            .unwrap();

        assert_eq!(
            result,
            vec![Value::List(vec![
                Value::Tuple(vec![
                    Value::String("k1".to_string()),
                    Value::String("v1".to_string())
                ]),
                Value::Tuple(vec![
                    Value::String("k2".to_string()),
                    Value::String("v2".to_string())
                ])
            ])]
        )
    }
}
