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
use anyhow::anyhow;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{BoxError, Router};
use bytes::Bytes;
use futures::stream;
use golem_common::agent_id;
use golem_common::model::component::{ComponentFilePath, ComponentFilePermissions};
use golem_common::model::worker::{FlatComponentFileSystemNode, FlatComponentFileSystemNodeKind};
use golem_common::model::{IdempotencyKey, WorkerStatus};
use golem_test_framework::dsl::{drain_connection, stderr_events, stdout_events, TestDsl};
use golem_test_framework::model::IFSEntry;
use golem_wasm::Value;
use golem_worker_executor_test_utils::{
    start, LastUniqueId, TestContext, WorkerExecutorTestDependencies,
};
use http::{HeaderMap, StatusCode};
use pretty_assertions::assert_eq;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
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
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("logging", "write-stdout-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let mut rx = executor.capture_output(&worker_id).await?;

    executor
        .invoke_and_await_agent(&component.id, &agent_id, "write_stdout", data_value!())
        .await?;

    let mut events = vec![];
    let start_time = Instant::now();
    while events.len() < 4 && start_time.elapsed() < Duration::from_secs(5) {
        if let Some(event) = rx.recv().await {
            events.push(event);
        } else {
            break;
        }
    }

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(
        stdout_events(events.into_iter()),
        vec!["Sample text written to the output\n"]
    );
    Ok(())
}

#[test]
#[tracing::instrument]
async fn write_stderr(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("logging", "write-stderr-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let mut rx = executor.capture_output(&worker_id).await?;

    executor
        .invoke_and_await_agent(&component.id, &agent_id, "write_stderr", data_value!())
        .await?;

    let mut events = vec![];
    let start_time = Instant::now();
    while events.len() < 4 && start_time.elapsed() < Duration::from_secs(5) {
        if let Some(event) = rx.recv().await {
            events.push(event);
        } else {
            break;
        }
    }

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(
        stderr_events(events.into_iter()),
        vec!["Sample text written to the error output\n"]
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn read_stdin(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("io", "read-stdin-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component.id, &agent_id, "run", data_value!())
        .await;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert!(result.is_err()); // stdin is disabled
    Ok(())
}

#[test]
#[tracing::instrument]
async fn clocks(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("clocks", "clocks-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component.id, &agent_id, "use_std_time_apis", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let Value::Record(fields) = &result else {
        panic!("expected record, got {:?}", result)
    };
    assert_eq!(fields.len(), 3);

    let Value::F64(elapsed1) = &fields[0] else {
        panic!("expected f64")
    };
    let Value::F64(elapsed2) = &fields[1] else {
        panic!("expected f64")
    };
    let Value::String(odt) = &fields[2] else {
        panic!("expected string")
    };

    let epoch_seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let diff1 = (epoch_seconds - *elapsed1).abs();
    let parsed_odt = chrono::DateTime::parse_from_rfc3339(odt.as_str()).unwrap();
    let odt_diff = epoch_seconds - parsed_odt.timestamp() as f64;

    assert!(diff1 < 5.0);
    assert!(*elapsed2 >= 2.0);
    assert!(*elapsed2 < 3.0);
    assert!(odt_diff < 5.0);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn file_write_read_delete(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .with_env(vec![("RUST_BACKTRACE".to_string(), "full".to_string())])
        .store()
        .await?;
    let agent_id = agent_id!("file-system", "file-write-read-delete-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "run_file_write_read_delete",
            data_value!(),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(
        result,
        Value::Record(vec![
            Value::Option(None),
            Value::Option(Some(Box::new(Value::String("hello world".to_string())))),
            Value::Option(None)
        ])
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn initial_file_read_write(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "it_initial_file_system_release",
        )
        .name("golem-it:initial-file-system")
        .unique()
        .with_files(&[
            IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/foo.txt"),
                target_path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadOnly,
            },
            IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                target_path: ComponentFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadWrite,
            },
        ])
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());
    let agent_id = agent_id!("file-read-write", "initial-file-read-write-1");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, HashMap::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component.id, &agent_id, "run", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(
        result,
        Value::Tuple(vec![
            Value::Option(Some(Box::new(Value::String("foo\n".to_string())))),
            Value::Option(None),
            Value::Option(None),
            Value::Option(Some(Box::new(Value::String("baz\n".to_string())))),
            Value::Option(Some(Box::new(Value::String("hello world".to_string())))),
        ])
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn initial_file_listing_through_api(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::agent_id;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "it_initial_file_system_release",
        )
        .name("golem-it:initial-file-system")
        .unique()
        .with_files(&[
            IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/foo.txt"),
                target_path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadOnly,
            },
            IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                target_path: ComponentFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadWrite,
            },
            IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                target_path: ComponentFilePath::from_abs_str("/baz.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadWrite,
            },
        ])
        .store()
        .await?;

    let agent_id = agent_id!("file-read-write", "initial-file-listing-1");
    let worker_id = executor.start_agent(&component.id, agent_id).await?;

    let result = executor.get_file_system_node(&worker_id, "/").await?;

    let mut result = result
        .into_iter()
        .map(|e| FlatComponentFileSystemNode {
            last_modified: 0,
            ..e
        })
        .collect::<Vec<_>>();

    result.sort_by_key(|e| e.name.clone());

    assert_eq!(
        result,
        vec![
            FlatComponentFileSystemNode {
                name: "bar".to_string(),
                last_modified: 0,
                kind: FlatComponentFileSystemNodeKind::Directory,
                permissions: None,
                size: None
            },
            FlatComponentFileSystemNode {
                name: "baz.txt".to_string(),
                last_modified: 0,
                kind: FlatComponentFileSystemNodeKind::File,
                permissions: Some(ComponentFilePermissions::ReadWrite),
                size: Some(4),
            },
            FlatComponentFileSystemNode {
                name: "foo.txt".to_string(),
                last_modified: 0,
                kind: FlatComponentFileSystemNodeKind::File,
                permissions: Some(ComponentFilePermissions::ReadOnly),
                size: Some(4)
            },
        ]
    );

    let result = executor.get_file_system_node(&worker_id, "/bar").await?;

    let mut result = result
        .into_iter()
        .map(|e| FlatComponentFileSystemNode {
            last_modified: 0,
            ..e
        })
        .collect::<Vec<_>>();

    result.sort_by_key(|e| e.name.clone());

    assert_eq!(
        result,
        vec![FlatComponentFileSystemNode {
            name: "baz.txt".to_string(),
            last_modified: 0,
            kind: FlatComponentFileSystemNodeKind::File,
            permissions: Some(ComponentFilePermissions::ReadWrite),
            size: Some(4),
        },]
    );

    let result = executor
        .get_file_system_node(&worker_id, "/baz.txt")
        .await?;

    let mut result = result
        .into_iter()
        .map(|e| FlatComponentFileSystemNode {
            last_modified: 0,
            ..e
        })
        .collect::<Vec<_>>();

    result.sort_by_key(|e| e.name.clone());

    assert_eq!(
        result,
        vec![FlatComponentFileSystemNode {
            name: "baz.txt".to_string(),
            last_modified: 0,
            kind: FlatComponentFileSystemNodeKind::File,
            permissions: Some(ComponentFilePermissions::ReadWrite),
            size: Some(4),
        },]
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    Ok(())
}

#[test]
#[tracing::instrument]
async fn initial_file_reading_through_api(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "it_initial_file_system_release",
        )
        .name("golem-it:initial-file-system")
        .unique()
        .with_files(&[
            IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/foo.txt"),
                target_path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadOnly,
            },
            IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                target_path: ComponentFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadWrite,
            },
        ])
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());
    let agent_id = agent_id!("file-read-write", "initial-file-read-write-3");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, HashMap::new())
        .await?;

    // run the agent so it can update the files.
    executor
        .invoke_and_await_agent(&component.id, &agent_id, "run", data_value!())
        .await?;

    let result1 = executor.get_file_contents(&worker_id, "/foo.txt").await?;
    let result1 = std::str::from_utf8(&result1).unwrap();

    let result2 = executor
        .get_file_contents(&worker_id, "/bar/baz.txt")
        .await?;
    let result2 = std::str::from_utf8(&result2).unwrap();

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(result1, "foo\n");
    assert_eq!(result2, "hello world");

    Ok(())
}

#[test]
#[tracing::instrument]
async fn directories(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("file-system", "directories-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component.id, &agent_id, "run_directories", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let Value::Record(fields) = &result else {
        panic!("expected record, got {:?}", result)
    };
    assert_eq!(fields.len(), 4);

    assert_eq!(fields[0], Value::U32(0)); // initial number of entries
    assert_eq!(
        fields[1],
        Value::List(vec![Value::Record(vec![
            Value::String("/test".to_string()),
            Value::Bool(true)
        ])])
    ); // contents of /

    // contents of /test
    let Value::List(list) = &fields[2] else {
        panic!("expected list")
    };
    assert_eq!(
        *list,
        vec![
            Value::Record(vec![
                Value::String("/test/dir1".to_string()),
                Value::Bool(true)
            ]),
            Value::Record(vec![
                Value::String("/test/dir2".to_string()),
                Value::Bool(true)
            ]),
            Value::Record(vec![
                Value::String("/test/hello.txt".to_string()),
                Value::Bool(false)
            ]),
        ]
    );
    assert_eq!(fields[3], Value::U32(1)); // final number of entries NOTE: this should be 0 if remove_directory worked

    Ok(())
}

#[test]
#[tracing::instrument]
async fn directories_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("file-system", "directories-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component.id, &agent_id, "run_directories", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    // NOTE: if the directory listing would not be stable, replay would fail with divergence error

    let metadata = executor
        .wait_for_status(&worker_id, WorkerStatus::Idle, Duration::from_secs(5))
        .await?;

    assert_eq!(metadata.status, WorkerStatus::Idle);

    let Value::Record(fields) = &result else {
        panic!("expected record, got {:?}", result)
    };
    assert_eq!(fields.len(), 4);

    assert_eq!(fields[0], Value::U32(0)); // initial number of entries
    assert_eq!(
        fields[1],
        Value::List(vec![Value::Record(vec![
            Value::String("/test".to_string()),
            Value::Bool(true)
        ])])
    ); // contents of /

    // contents of /test
    let Value::List(list) = &fields[2] else {
        panic!("expected list")
    };
    assert_eq!(
        *list,
        vec![
            Value::Record(vec![
                Value::String("/test/dir1".to_string()),
                Value::Bool(true)
            ]),
            Value::Record(vec![
                Value::String("/test/dir2".to_string()),
                Value::Bool(true)
            ]),
            Value::Record(vec![
                Value::String("/test/hello.txt".to_string()),
                Value::Bool(false)
            ]),
        ]
    );
    assert_eq!(fields[3], Value::U32(1)); // final number of entries NOTE: this should be 0 if remove_directory worked

    Ok(())
}

#[test]
#[tracing::instrument]
async fn file_write_read(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("file-system", "file-service-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "write_file",
            data_value!("/testfile.txt", "hello world"),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let result = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "read_file",
            data_value!("/testfile.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(
        result,
        Value::Result(Ok(Some(Box::new(Value::String("hello world".to_string())))))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn file_update_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "it_initial_file_system_release",
        )
        .name("golem-it:initial-file-system")
        .unique()
        .with_files(&[IFSEntry {
            source_path: PathBuf::from("initial-file-system/files/foo.txt"),
            target_path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
            permissions: ComponentFilePermissions::ReadOnly,
        }])
        .store()
        .await?;

    let agent_id = agent_id!("ifs-update", "ifs-update-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(&component.id, &agent_id, "load_file", data_value!())
        .await?;

    {
        let content_before_update = executor
            .invoke_and_await_agent(&component.id, &agent_id, "get_file_content", data_value!())
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value"))?;

        assert_eq!(content_before_update, Value::String("foo\n".to_string()));
    }

    {
        let updated_component = executor
            .update_component_with_files(
                &component.id,
                "it_initial_file_system_release",
                vec![IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/bar.txt"),
                    target_path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
                    permissions: ComponentFilePermissions::ReadOnly,
                }],
            )
            .await?;

        executor
            .auto_update_worker(&worker_id, updated_component.revision, false)
            .await?;
    };

    {
        let content_after_update = executor
            .invoke_and_await_agent(&component.id, &agent_id, "get_file_content", data_value!())
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value"))?;

        assert_eq!(content_after_update, Value::String("foo\n".to_string()));
    }

    executor.simulated_crash(&worker_id).await?;

    {
        let content_after_crash = executor
            .invoke_and_await_agent(&component.id, &agent_id, "get_file_content", data_value!())
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value"))?;

        assert_eq!(content_after_crash, Value::String("foo\n".to_string()));
    }

    executor
        .invoke_and_await_agent(&component.id, &agent_id, "load_file", data_value!())
        .await?;

    {
        let content_after_reload = executor
            .invoke_and_await_agent(&component.id, &agent_id, "get_file_content", data_value!())
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value"))?;

        assert_eq!(content_after_reload, Value::String("bar\n".to_string()));
    }

    executor.simulated_crash(&worker_id).await?;

    {
        let content_after_crash = executor
            .invoke_and_await_agent(&component.id, &agent_id, "get_file_content", data_value!())
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value"))?;

        assert_eq!(content_after_crash, Value::String("bar\n".to_string()));
    }

    {
        let updated_component = executor
            .update_component_with_files(
                &component.id,
                "it_initial_file_system_release",
                vec![IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                    target_path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
                    permissions: ComponentFilePermissions::ReadOnly,
                }],
            )
            .await?;

        executor
            .manual_update_worker(&worker_id, updated_component.revision, false)
            .await?;
    };

    {
        let content_after_manual_update = executor
            .invoke_and_await_agent(&component.id, &agent_id, "get_file_content", data_value!())
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value"))?;

        assert_eq!(
            content_after_manual_update,
            Value::String("restored".to_string())
        );
    }

    executor
        .invoke_and_await_agent(&component.id, &agent_id, "load_file", data_value!())
        .await?;

    {
        let content_after_reload = executor
            .invoke_and_await_agent(&component.id, &agent_id, "get_file_content", data_value!())
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value"))?;

        assert_eq!(content_after_reload, Value::String("baz\n".to_string()));
    }

    executor.simulated_crash(&worker_id).await?;

    {
        let content_after_crash = executor
            .invoke_and_await_agent(&component.id, &agent_id, "get_file_content", data_value!())
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value"))?;

        assert_eq!(content_after_crash, Value::String("baz\n".to_string()));
    }

    executor.delete_worker(&worker_id).await?;

    Ok(())
}

#[test]
#[tracing::instrument]
async fn file_update_in_the_middle_of_exported_function(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

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

    let component = executor
        .component(
            &context.default_environment_id,
            "it_initial_file_system_release",
        )
        .name("golem-it:initial-file-system")
        .unique()
        .with_files(&[IFSEntry {
            source_path: PathBuf::from("initial-file-system/files/foo.txt"),
            target_path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
            permissions: ComponentFilePermissions::ReadOnly,
        }])
        .with_env(vec![
            ("PORT".to_string(), host_http_port.to_string()),
            ("RUST_BACKTRACE".to_string(), "full".to_string()),
        ])
        .store()
        .await?;

    let agent_id = agent_id!("ifs-update-inside-exported-function", "ifs-update-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let idempotency_key = IdempotencyKey::fresh();

    executor
        .invoke_agent_with_key(
            &component.id,
            &agent_id,
            &idempotency_key,
            "run",
            data_value!(),
        )
        .await?;

    latch.recv().await.expect("channel should produce value");

    {
        let updated_component = executor
            .update_component_with_files(
                &component.id,
                "it_initial_file_system_release",
                vec![IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/bar.txt"),
                    target_path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
                    permissions: ComponentFilePermissions::ReadOnly,
                }],
            )
            .await?;

        executor
            .auto_update_worker(&worker_id, updated_component.revision, false)
            .await?;
    };

    {
        let result = executor
            .invoke_and_await_agent_with_key(
                &component.id,
                &agent_id,
                &idempotency_key,
                "run",
                data_value!(),
            )
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value"))?;

        assert_eq!(
            result,
            Value::Tuple(vec![
                Value::String("foo\n".to_string()),
                Value::String("bar\n".to_string())
            ])
        );
    }

    http_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
async fn environment_variables(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("environment", "environment-service-1");
    let mut env = HashMap::new();
    env.insert("TEST_ENV".to_string(), "test-value".to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, HashMap::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get_environment", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let worker_name = agent_id.to_string();
    assert_eq!(
        result,
        Value::Result(Ok(Some(Box::new(Value::List(vec![
            Value::Tuple(vec![
                Value::String("TEST_ENV".to_string()),
                Value::String("test-value".to_string())
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_AGENT_ID".to_string()),
                Value::String(worker_name.clone())
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_WORKER_NAME".to_string()),
                Value::String(worker_name)
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_COMPONENT_ID".to_string()),
                Value::String(component.id.to_string())
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_COMPONENT_REVISION".to_string()),
                Value::String("0".to_string())
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_AGENT_TYPE".to_string()),
                Value::String("Environment".to_string())
            ])
        ])))))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_client_response_persisted_between_invocations(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

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

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_http_tests_release",
        )
        .name("golem-it:http-tests")
        .store()
        .await?;
    let agent_id = agent_id!("http-client");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, HashMap::new())
        .await?;
    let rx = executor.capture_output(&worker_id).await?;

    executor
        .invoke_and_await_agent(&component.id, &agent_id, "send_request", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    drop(rx);
    let executor = start(deps, &context).await?;
    let _rx = executor.capture_output(&worker_id).await?;

    let result = executor
        .invoke_and_await_agent(&component.id, &agent_id, "process_response", data_value!())
        .await?;

    http_server.abort();

    assert_eq!(result, data_value!("200 response is test-header test-body"));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_client_interrupting_response_stream(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

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

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_http_tests_release",
        )
        .name("golem-it:http-tests")
        .store()
        .await?;
    let agent_id = agent_id!("http-client2");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, HashMap::new())
        .await?;
    let (rx, _abort_capture) = executor.capture_output_with_termination(&worker_id).await?;

    let key = IdempotencyKey::fresh();

    let executor_clone = executor.clone();
    let component_id_clone = component.id;
    let agent_id_clone = agent_id.clone();
    let key_clone = key.clone();
    let _handle = spawn(
        async move {
            let _ = executor_clone
                .invoke_and_await_agent_with_key(
                    &component_id_clone,
                    &agent_id_clone,
                    &key_clone,
                    "slow_body_stream",
                    data_value!(),
                )
                .await;
        }
        .in_current_span(),
    );

    signal_rx.recv().await.unwrap();

    executor.interrupt(&worker_id).await?; // Potential "body stream was interrupted" error

    drain_connection(rx).await;

    executor.resume(&worker_id, false).await?;

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(5))
        .await?;

    executor.log_output(&worker_id).await?;

    let result = executor
        .invoke_and_await_agent_with_key(
            &component.id,
            &agent_id,
            &key,
            "slow_body_stream",
            data_value!(),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    assert_eq!(result, data_value!(100u64 * 1024u64));

    let idempotency_keys = idempotency_keys.lock().unwrap();
    assert_eq!(idempotency_keys.len(), 2);
    assert_eq!(idempotency_keys[0], idempotency_keys[1]);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_client_interrupting_response_stream_async(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

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

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_http_tests_release",
        )
        .name("golem-it:http-tests")
        .store()
        .await?;
    let agent_id = agent_id!("http-client3");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, HashMap::new())
        .await?;
    let (rx, _abort_capture) = executor.capture_output_with_termination(&worker_id).await?;

    let key = IdempotencyKey::fresh();

    let executor_clone = executor.clone();
    let component_id_clone = component.id;
    let agent_id_clone = agent_id.clone();
    let key_clone = key.clone();
    let _handle = spawn(
        async move {
            let _ = executor_clone
                .invoke_and_await_agent_with_key(
                    &component_id_clone,
                    &agent_id_clone,
                    &key_clone,
                    "slow_body_stream",
                    data_value!(),
                )
                .await;
        }
        .in_current_span(),
    );

    signal_rx.recv().await.unwrap();

    executor.interrupt(&worker_id).await?; // Potential "body stream was interrupted" error

    drain_connection(rx).await;

    executor.resume(&worker_id, false).await?;

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(5))
        .await?;
    executor.log_output(&worker_id).await?;

    let result = executor
        .invoke_and_await_agent_with_key(
            &component.id,
            &agent_id,
            &key,
            "slow_body_stream",
            data_value!(),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    assert_eq!(result, data_value!(100u64 * 1024u64));

    let idempotency_keys = idempotency_keys.lock().unwrap();
    assert_eq!(idempotency_keys.len(), 2);
    assert_eq!(idempotency_keys[0], idempotency_keys[1]);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn sleep(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("clock", "clock-service-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(&component.id, &agent_id, "sleep", data_value!(10u64))
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let component_id = component.id;
    drop(executor);
    let executor = start(deps, &context).await?;

    let start = Instant::now();
    executor
        .invoke_and_await_agent(&component_id, &agent_id, "sleep", data_value!(0u64))
        .await?;
    let duration = start.elapsed();

    assert!(duration.as_secs() < 2);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn sleep_less_than_suspend_threshold(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("clock", "clock-service-2");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let start = Instant::now();
    executor
        .invoke_and_await_agent(&component.id, &agent_id, "sleep", data_value!(1u64))
        .await?;

    let result = executor
        .invoke_and_await_agent(&component.id, &agent_id, "healthcheck", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    assert!(duration.as_secs() >= 1);
    assert_eq!(result, Value::Bool(true));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn sleep_longer_than_suspend_threshold(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("clock", "clock-service-3");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let start = Instant::now();
    executor
        .invoke_and_await_agent(&component.id, &agent_id, "sleep", data_value!(12u64))
        .await?;

    let result = executor
        .invoke_and_await_agent(&component.id, &agent_id, "healthcheck", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    assert!(duration.as_secs() >= 12);
    assert_eq!(result, Value::Bool(true));

    Ok(())
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
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let (port, server) = simulated_slow_request_server(Duration::from_secs(10)).await;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .with_env(vec![("PORT".to_string(), port.to_string())])
        .store()
        .await?;
    let agent_id = agent_id!("clock", "clock-service-4");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let start = Instant::now();
    let result = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "sleep_during_request",
            data_value!(2u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    server.abort();
    drop(executor);

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    assert!(duration.as_secs() >= 2);
    assert!(duration.as_secs() < 10);
    assert_eq!(result, Value::String("Timeout".to_string()));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn sleep_longer_than_suspend_threshold_while_awaiting_response(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let (port, server) = simulated_slow_request_server(Duration::from_secs(5)).await;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .with_env(vec![("PORT".to_string(), port.to_string())])
        .store()
        .await?;
    let agent_id = agent_id!("clock", "clock-service-5");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let start = Instant::now();
    let result = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "sleep_during_request",
            data_value!(30u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    server.abort();
    drop(executor);

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    assert!(duration.as_secs() >= 5);
    assert!(duration.as_secs() < 30);
    assert_eq!(result, Value::String("slow response".to_string()));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn sleep_longer_than_suspend_threshold_while_awaiting_response_2(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let (port, server) = simulated_slow_request_server(Duration::from_secs(30)).await;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .with_env(vec![("PORT".to_string(), port.to_string())])
        .store()
        .await?;
    let agent_id = agent_id!("clock", "clock-service-6");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let start = Instant::now();
    let result = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "sleep_during_request",
            data_value!(15u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    server.abort();
    drop(executor);

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    assert!(duration.as_secs() >= 15);
    assert!(duration.as_secs() < 30);
    assert_eq!(result, Value::String("Timeout".to_string()));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn sleep_and_awaiting_parallel_responses(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let (port, server) = simulated_slow_request_server(Duration::from_secs(2)).await;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .with_env(vec![("PORT".to_string(), port.to_string())])
        .store()
        .await?;
    let agent_id = agent_id!("clock", "clock-service-7");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let start = Instant::now();
    let result = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "sleep_during_parallel_requests",
            data_value!(20u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let component_id = component.id;
    drop(executor);
    server.abort();

    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    info!("Restarting worker...");
    let executor = golem_worker_executor_test_utils::start(deps, &context).await?;
    info!("Worker restarted");

    let healthcheck_result = executor
        .invoke_and_await_agent(&component_id, &agent_id, "healthcheck", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert!(duration.as_secs() >= 10);
    assert!(duration.as_secs() < 20);
    assert_eq!(result, Value::String("Ok(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\n".to_string()));
    assert_eq!(healthcheck_result, Value::Bool(true));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn sleep_below_threshold_between_http_responses(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let (port, server) = simulated_slow_request_server(Duration::from_secs(1)).await;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .with_env(vec![("PORT".to_string(), port.to_string())])
        .store()
        .await?;
    let agent_id = agent_id!("clock", "clock-service-8");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor.log_output(&worker_id).await?;

    let start = Instant::now();
    let result = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "sleep_between_requests",
            data_value!(1u64, 5u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    server.abort();
    let component_id = component.id;
    drop(executor);
    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    info!("Restarting worker...");
    let executor = golem_worker_executor_test_utils::start(deps, &context).await?;
    info!("Worker restarted");

    let healthcheck_result = executor
        .invoke_and_await_agent(&component_id, &agent_id, "healthcheck", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert!(duration.as_secs() >= 10);
    assert_eq!(result, Value::String("Ok(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\nOk(\"slow response\")\n".to_string()));
    assert_eq!(healthcheck_result, Value::Bool(true));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn sleep_above_threshold_between_http_responses(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let (port, server) = simulated_slow_request_server(Duration::from_secs(1)).await;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .with_env(vec![("PORT".to_string(), port.to_string())])
        .store()
        .await?;
    let agent_id = agent_id!("clock", "clock-service-9");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let start = Instant::now();
    let result = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "sleep_between_requests",
            data_value!(12u64, 2u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    server.abort();
    let component_id = component.id;
    drop(executor);
    let duration = start.elapsed();
    debug!("duration: {:?}", duration);

    info!("Restarting worker...");
    let executor = golem_worker_executor_test_utils::start(deps, &context).await?;
    info!("Worker restarted");

    let healthcheck_result = executor
        .invoke_and_await_agent(&component_id, &agent_id, "healthcheck", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert!(duration.as_secs() >= 14);
    assert_eq!(
        result,
        Value::String("Ok(\"slow response\")\nOk(\"slow response\")\n".to_string())
    );
    assert_eq!(healthcheck_result, Value::Bool(true));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn resuming_sleep(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("clock", "clock-service-2");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let executor_clone = executor.clone();
    let component_id_clone = component.id;
    let agent_id_clone = agent_id.clone();
    let fiber = spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(
                    &component_id_clone,
                    &agent_id_clone,
                    "sleep",
                    data_value!(10u64),
                )
                .await
        }
        .in_current_span(),
    );

    tokio::time::sleep(Duration::from_secs(5)).await;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let component_id = component.id;
    drop(executor);
    fiber.await??;

    info!("Restarting worker...");

    let executor = start(deps, &context).await?;

    info!("Worker restarted");

    let start = Instant::now();
    executor
        .invoke_and_await_agent(&component_id, &agent_id, "sleep", data_value!(10u64))
        .await?;
    let duration = start.elapsed();

    assert!(duration.as_secs() < 20);
    assert!(duration.as_secs() >= 10);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn failing_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::data_value;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;
    let agent_id = agent_id!("failing-counter", "failing-worker-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(&component.id, &agent_id, "add", data_value!(5u64))
        .await?;

    let result2 = executor
        .invoke_and_await_agent(&component.id, &agent_id, "add", data_value!(50u64))
        .await;

    let result3 = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get", data_value!())
        .await;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert!(result2.is_err());
    assert!(result3.is_err());

    let err2 = format!("{}", result2.err().unwrap());
    assert!(
        err2.contains("error log message"),
        "Expected 'error log message' in error: {err2}"
    );
    assert!(
        err2.contains("value is too large"),
        "Expected 'value is too large' in error: {err2}"
    );

    let err3 = format!("{}", result3.err().unwrap());
    assert!(
        err3.contains("error log message"),
        "Expected 'error log message' in error: {err3}"
    );
    assert!(
        err3.contains("value is too large"),
        "Expected 'value is too large' in error: {err3}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn file_service_write_direct(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("file-system", "file-service-2");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "write_file_direct",
            data_value!("testfile.txt", "hello world"),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let result = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "read_file",
            data_value!("/testfile.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(
        result,
        Value::Result(Ok(Some(Box::new(Value::String("hello world".to_string())))))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn filesystem_write_replay_restores_file_times(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("file-system", "file-service-3");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "write_file_direct",
            data_value!("testfile.txt", "hello world"),
        )
        .await?;

    let times1 = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "get_file_info",
            data_value!("/testfile.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let times2 = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "get_file_info",
            data_value!("/testfile.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(times1, times2);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn filesystem_create_dir_replay_restores_file_times(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("file-system", "file-service-4");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "create_directory",
            data_value!("/test"),
        )
        .await?;

    let times1 = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get_info", data_value!("/"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let times2 = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get_info", data_value!("/"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(times1, times2);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn file_hard_link(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("file-system", "file-service-5");
    let _worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "write_file",
            data_value!("/testfile.txt", "hello world"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "create_link",
            data_value!("/testfile.txt", "/link.txt"),
        )
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "read_file",
            data_value!("/link.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(
        result,
        Value::Result(Ok(Some(Box::new(Value::String("hello world".to_string())))))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn filesystem_link_replay_restores_file_times(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("file-system", "file-service-6");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "create_directory",
            data_value!("/test"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "create_directory",
            data_value!("/test2"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "write_file",
            data_value!("/test/testfile.txt", "hello world"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "create_link",
            data_value!("/test/testfile.txt", "/test2/link.txt"),
        )
        .await?;

    let times_file_1 = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "get_info",
            data_value!("/test2/link.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let times_dir_1 = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get_info", data_value!("/test2"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let times_dir_2 = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get_info", data_value!("/test2"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let times_file_2 = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "get_info",
            data_value!("/test2/link.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(times_dir_1, times_dir_2);
    assert_eq!(times_file_1, times_file_2);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn filesystem_remove_dir_replay_restores_file_times(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("file-system", "file-service-7");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "create_directory",
            data_value!("/test"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "create_directory",
            data_value!("/test/a"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "remove_directory",
            data_value!("/test/a"),
        )
        .await?;

    let times1 = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get_info", data_value!("/test"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let times2 = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get_info", data_value!("/test"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(times1, times2);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn filesystem_symlink_replay_restores_file_times(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("file-system", "file-service-8");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "create_directory",
            data_value!("/test"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "create_directory",
            data_value!("/test2"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "write_file_direct",
            data_value!("test/testfile.txt", "hello world"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "create_sym_link",
            data_value!("../test/testfile.txt", "/test2/link.txt"),
        )
        .await?;

    let times_file_1 = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "get_info",
            data_value!("/test2/link.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let times_dir_1 = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get_info", data_value!("/test2"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let times_dir_2 = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get_info", data_value!("/test2"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let times_file_2 = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "get_info",
            data_value!("/test2/link.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(times_dir_1, times_dir_2);
    assert_eq!(times_file_1, times_file_2);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn filesystem_rename_replay_restores_file_times(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("file-system", "file-service-9");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "create_directory",
            data_value!("/test"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "create_directory",
            data_value!("/test2"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "write_file",
            data_value!("/test/testfile.txt", "hello world"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "rename_file",
            data_value!("/test/testfile.txt", "/test2/link.txt"),
        )
        .await?;

    let times_srcdir_1 = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get_info", data_value!("/test"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let times_destdir_1 = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get_info", data_value!("/test2"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let times_file_1 = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "get_info",
            data_value!("/test2/link.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let times_srcdir_2 = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get_info", data_value!("/test"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let times_destdir_2 = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get_info", data_value!("/test2"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let times_file_2 = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "get_info",
            data_value!("/test2/link.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(times_srcdir_1, times_srcdir_2);
    assert_eq!(times_destdir_1, times_destdir_2);
    assert_eq!(times_file_1, times_file_2);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn filesystem_remove_file_replay_restores_file_times(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("file-system", "file-service-10");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "create_directory",
            data_value!("/test"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "write_file",
            data_value!("/test/testfile.txt", "hello world"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "get_info",
            data_value!("/test/testfile.txt"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "remove_file",
            data_value!("/test/testfile.txt"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "get_info",
            data_value!("/test/testfile.txt"),
        )
        .await?;

    let times1 = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get_info", data_value!("/test"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let times2 = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get_info", data_value!("/test"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(times1, times2);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn filesystem_write_via_stream_replay_restores_file_times(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("file-system", "file-service-3");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "write_file",
            data_value!("/testfile.txt", "hello world"),
        )
        .await?;

    let times1 = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "get_file_info",
            data_value!("/testfile.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let times2 = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "get_file_info",
            data_value!("/testfile.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(times1, times2);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn filesystem_metadata_hash(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("file-system", "file-service-3");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "write_file_direct",
            data_value!("testfile.txt", "hello world"),
        )
        .await?;

    let hash1 = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "hash",
            data_value!("testfile.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let hash2 = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "hash",
            data_value!("testfile.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(hash1, hash2);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn ip_address_resolve(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("networking", "ip-address-resolve-1");
    let component_id = component.id;
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result1 = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    drop(executor);
    let executor = start(deps, &context).await?;

    // If the recovery succeeds, that means that the replayed IP address resolution produced the same result as expected

    let result2 = executor
        .invoke_and_await_agent(&component_id, &agent_id, "get", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Result 2 is a fresh resolution which is not guaranteed to return the same addresses (or the same order) but we can expect
    // that it could resolve golem.cloud to at least one address.
    let Value::List(entries1) = &result1 else {
        panic!("expected list, got {:?}", result1)
    };
    let Value::List(entries2) = &result2 else {
        panic!("expected list, got {:?}", result2)
    };
    assert!(!entries1.is_empty());
    assert!(!entries2.is_empty());

    Ok(())
}

#[test]
#[tracing::instrument]
async fn wasi_config_initial_worker_config(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("wasi-config", "worker-1");

    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::new(),
            HashMap::from_iter(vec![
                ("k1".to_string(), "v1".to_string()),
                ("k2".to_string(), "v2".to_string()),
            ]),
        )
        .await?;

    {
        // get existing key

        let result = executor
            .invoke_and_await_agent(&component.id, &agent_id, "get", data_value!("k1"))
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value"))?;

        assert_eq!(
            result,
            Value::Option(Some(Box::new(Value::String("v1".to_string()))))
        )
    }

    {
        // get non-existent key

        let result = executor
            .invoke_and_await_agent(&component.id, &agent_id, "get", data_value!("k3"))
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value"))?;

        assert_eq!(result, Value::Option(None))
    }

    {
        // get all keys

        let result = executor
            .invoke_and_await_agent(&component.id, &agent_id, "get_all", data_value!())
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value"))?;

        assert_eq!(
            result,
            Value::List(vec![
                Value::Tuple(vec![
                    Value::String("k1".to_string()),
                    Value::String("v1".to_string())
                ]),
                Value::Tuple(vec![
                    Value::String("k2".to_string()),
                    Value::String("v2".to_string())
                ])
            ])
        )
    }

    executor.check_oplog_is_queryable(&worker_id).await?;

    Ok(())
}

#[test]
#[tracing::instrument]
async fn wasi_config_component_update(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_id, data_value};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .with_wasi_config_vars(vec![
            ("k1".to_string(), "v0".to_string()),
            ("k3".to_string(), "v3".to_string()),
        ])
        .store()
        .await?;

    let agent_id = agent_id!("wasi-config", "worker-1");

    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::new(),
            HashMap::from_iter(vec![
                ("k1".to_string(), "v1".to_string()),
                ("k2".to_string(), "v2".to_string()),
            ]),
        )
        .await?;

    {
        let result = executor
            .invoke_and_await_agent(&component.id, &agent_id, "get_all", data_value!())
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value"))?;

        assert_eq!(
            result,
            Value::List(vec![
                Value::Tuple(vec![
                    Value::String("k1".to_string()),
                    Value::String("v1".to_string())
                ]),
                Value::Tuple(vec![
                    Value::String("k2".to_string()),
                    Value::String("v2".to_string())
                ]),
                Value::Tuple(vec![
                    Value::String("k3".to_string()),
                    Value::String("v3".to_string())
                ]),
            ])
        )
    }

    let updated_component = executor
        .update_component_with(
            &component.id,
            component.revision,
            None,
            Vec::new(),
            Vec::new(),
            None,
            None,
            Some(BTreeMap::from_iter(vec![
                ("k1".to_string(), "v2".to_string()),
                ("k3".to_string(), "v4".to_string()),
                ("k4".to_string(), "v4".to_string()),
            ])),
        )
        .await?;

    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;

    {
        let result = executor
            .invoke_and_await_agent(&component.id, &agent_id, "get_all", data_value!())
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value"))?;

        assert_eq!(
            result,
            Value::List(vec![
                Value::Tuple(vec![
                    Value::String("k1".to_string()),
                    Value::String("v1".to_string())
                ]),
                Value::Tuple(vec![
                    Value::String("k2".to_string()),
                    Value::String("v2".to_string())
                ]),
                Value::Tuple(vec![
                    Value::String("k3".to_string()),
                    Value::String("v4".to_string())
                ]),
                Value::Tuple(vec![
                    Value::String("k4".to_string()),
                    Value::String("v4".to_string())
                ]),
            ])
        )
    }

    executor.check_oplog_is_queryable(&worker_id).await?;

    Ok(())
}
